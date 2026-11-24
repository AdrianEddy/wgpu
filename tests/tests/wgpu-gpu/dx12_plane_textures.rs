use wgpu::*;
use wgpu_test::{
    apply, gpu_test, FailureCase, GpuTestConfiguration, GpuTestInitializer, TestParameters,
};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.push(DX12_PLANE_TEXTURES);
}

/// Each plane of a two-layer NV12 resource, wrapped as a single-plane texture with
/// `dx12::Texture::with_plane_slice`, is written and read through its wrapper alone, one layer
/// at a time. Each wrapper's barriers must name its own plane's subresources only: the D3D12
/// debug layer checks every barrier's states against the ones it tracks per subresource.
#[apply(gpu_test!)]
static DX12_PLANE_TEXTURES: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(Features::TEXTURE_FORMAT_NV12)
            .skip(FailureCase::backend(Backends::all() - Backends::DX12)),
    )
    .run_async(|ctx| async move {
        match ctx.adapter_info.backend {
            #[cfg(windows)]
            Backend::Dx12 => windows_impl::plane_textures(&ctx).await,
            other => unreachable!(
                "test is configured to skip all backends except DX12, but ran on {other:?}"
            ),
        }
    });

#[cfg(windows)]
mod windows_impl {
    use wgpu::hal::dx12;

    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;
    const LAYERS: u32 = 2;

    /// A plane's bytes in `layer`, `bytes_per_texel` a texel: different in every plane and
    /// layer, and asymmetric under a transpose or a mirror.
    fn contents(plane: u32, layer: u32, width: u32, height: u32, bytes_per_texel: u32) -> Vec<u8> {
        (0..height)
            .flat_map(|y| {
                (0..width * bytes_per_texel).map(move |x| {
                    (x * 3 + y * 101 + plane * 37 + layer * 59) as u8 ^ (plane as u8 * 0x5a)
                })
            })
            .collect()
    }

    pub async fn plane_textures(ctx: &wgpu_test::TestingContext) {
        let nv12 = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nv12"),
            size: wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // SAFETY: Only read; the clone holds a reference of its own.
        let resource = unsafe { nv12.as_hal::<dx12::Api>().unwrap().raw_resource().clone() };

        // Luma, then chroma: (plane, format, width, height, bytes a texel).
        let planes = [
            (0, wgpu::TextureFormat::R8Unorm, WIDTH, HEIGHT, 1),
            (1, wgpu::TextureFormat::Rg8Unorm, WIDTH / 2, HEIGHT / 2, 2),
        ];
        let wrappers = planes.map(|(plane, format, width, height, _)| {
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: LAYERS,
            };
            // SAFETY: The resource has one mip level, one sample and `LAYERS` layers, and this
            // plane of it has `format`'s texels and `size`.
            let hal = unsafe {
                dx12::Device::texture_from_raw(
                    resource.clone(),
                    format,
                    wgpu::TextureDimension::D2,
                    size,
                    1,
                    1,
                    None,
                )
            }
            .with_plane_slice(plane);
            // SAFETY: Created from this device's resource, as described, and never used: its
            // state is `COMMON`, which `UNINITIALIZED` stands for. Every layer is written below
            // before it is read.
            unsafe {
                ctx.device.create_texture_from_hal::<dx12::Api>(
                    hal,
                    &wgpu::TextureDescriptor {
                        label: Some("nv12 plane"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::UNINITIALIZED,
                    true,
                )
            }
        });

        // Each plane's layers are written one at a time, so that a barrier covers a part of the
        // wrapper, and the two planes' writes interleave, so that their subresources' states
        // differ when the other plane's barriers are recorded.
        for layer in 0..LAYERS {
            for (&(plane, _, width, height, bytes_per_texel), wrapper) in
                planes.iter().zip(&wrappers)
            {
                ctx.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: wrapper,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &contents(plane, layer, width, height, bytes_per_texel),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * bytes_per_texel),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            ctx.queue.submit(None);
        }

        // Read back in the other order: the last layer first, chroma before luma.
        let mut readbacks = Vec::new();
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for layer in (0..LAYERS).rev() {
            for (&(plane, _, width, height, bytes_per_texel), wrapper) in
                planes.iter().zip(&wrappers).rev()
            {
                // 256 bytes a row in both planes, the copy alignment.
                let bytes_per_row = width * bytes_per_texel;
                let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: u64::from(bytes_per_row * height),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: wrapper,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(bytes_per_row),
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                readbacks.push((
                    plane,
                    layer,
                    contents(plane, layer, width, height, bytes_per_texel),
                    buffer,
                ));
            }
        }
        ctx.queue.submit([encoder.finish()]);

        for (_, _, _, buffer) in &readbacks {
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.unwrap());
        }
        ctx.async_poll(wgpu::PollType::wait_indefinitely())
            .await
            .unwrap();
        for (plane, layer, expected, buffer) in &readbacks {
            assert!(
                buffer.slice(..).get_mapped_range().unwrap()[..] == expected[..],
                "plane {plane}, layer {layer}: the contents differ"
            );
        }
    }
}
