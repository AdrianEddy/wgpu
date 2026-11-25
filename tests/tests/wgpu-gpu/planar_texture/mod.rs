//! Tests for nv12 texture creation and sampling.

use wgpu_test::{
    apply, fail, gpu_test, valid, GpuTestConfiguration, GpuTestInitializer, TestParameters,
    TestingContext,
};

pub fn all_tests(tests: &mut Vec<GpuTestInitializer>) {
    tests.extend([
        NV12_TEXTURE_CREATION_SAMPLING,
        P010_TEXTURE_CREATION_SAMPLING,
        NV12_TEXTURE_RENDERING,
        NV12_TEXTURE_COPYING,
        P010_TEXTURE_COPYING,
        NV12_PLANE_TO_SINGLE_PLANE_COPY,
        NV12_TEXTURE_STORAGE,
        P010_TEXTURE_STORAGE,
    ]);
}

// Helper function to test planar texture creation and sampling.
fn test_planar_texture_creation_sampling(
    ctx: &TestingContext,
    y_view: &wgpu::TextureView,
    uv_view: &wgpu::TextureView,
) {
    let target_format = wgpu::TextureFormat::Bgra8UnormSrgb;

    let shader = ctx
        .device
        .create_shader_module(wgpu::include_wgsl!("planar_texture_sampling.wgsl"));
    let pipeline = ctx
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("planar texture pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: Some(wgpu::IndexFormat::Uint32),
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

    let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
        min_filter: wgpu::FilterMode::Linear,
        mag_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(y_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(uv_view),
            },
        ],
    });

    let target_tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: y_view.texture().size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            ops: wgpu::Operations::default(),
            resolve_target: None,
            view: &target_view,
            depth_slice: None,
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    rpass.set_pipeline(&pipeline);
    rpass.set_bind_group(0, &bind_group, &[]);
    rpass.draw(0..4, 0..1);
    drop(rpass);
    ctx.queue.submit([encoder.finish()]);
}

// Helper function to test rendering onto planar texture.
fn test_planar_texture_rendering(
    ctx: &TestingContext,
    (y_view, y_format): (&wgpu::TextureView, wgpu::TextureFormat),
    (uv_view, uv_format): (&wgpu::TextureView, wgpu::TextureFormat),
) {
    let shader = ctx
        .device
        .create_shader_module(wgpu::include_wgsl!("planar_texture_rendering.wgsl"));
    let y_pipeline = ctx
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("y plane pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_y_main"),
                compilation_options: Default::default(),
                targets: &[Some(y_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: Some(wgpu::IndexFormat::Uint32),
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

    let uv_pipeline = ctx
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("uv plane pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_uv_main"),
                compilation_options: Default::default(),
                targets: &[Some(uv_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: Some(wgpu::IndexFormat::Uint32),
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                ops: wgpu::Operations::default(),
                resolve_target: None,
                view: y_view,
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rpass.set_pipeline(&y_pipeline);
        rpass.draw(0..3, 0..1);
    }
    {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                ops: wgpu::Operations::default(),
                resolve_target: None,
                view: uv_view,
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rpass.set_pipeline(&uv_pipeline);
        rpass.draw(0..3, 0..1);
    }

    ctx.queue.submit([encoder.finish()]);
}

// Helper function to check that a planar texture takes `STORAGE_BINDING` only
// where the adapter's features for its format and its planes' formats allow it,
// and that where they do, a compute shader's stores through each plane's
// storage view land in that plane.
async fn test_planar_texture_storage(
    ctx: &TestingContext,
    format: wgpu::TextureFormat,
    // Each plane's format and its WGSL storage format.
    planes: [(wgpu::TextureFormat, &str); 2],
) {
    const WIDTH: u32 = 256;
    const HEIGHT: u32 = 128;
    const CHROMA_WIDTH: u32 = WIDTH / 2;
    const CHROMA_HEIGHT: u32 = HEIGHT / 2;

    let allows_storage = |format| {
        ctx.adapter
            .get_texture_format_features(format)
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING)
    };
    let desc = wgpu::TextureDescriptor {
        label: Some("planar storage"),
        dimension: wgpu::TextureDimension::D2,
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        format,
        usage: wgpu::TextureUsages::STORAGE_BINDING,
        mip_level_count: 1,
        sample_count: 1,
        view_formats: &[],
    };
    if !(allows_storage(format) && planes.iter().all(|&(plane, _)| allows_storage(plane))) {
        fail(
            &ctx.device,
            || ctx.device.create_texture(&desc),
            Some(&format!(
                "Texture usages TextureUsages(STORAGE_BINDING) are not allowed on a texture of type {format:?}"
            )),
        );
        return;
    }

    let texture = valid(&ctx.device, || {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            ..desc
        })
    });

    // The stores stay within each plane's own extent: the chroma plane is half
    // the luma plane in each axis.
    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("planar storage"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "@group(0) @binding(0) var luma: texture_storage_2d<{luma}, write>;
                     @group(0) @binding(1) var chroma: texture_storage_2d<{chroma}, write>;

                     fn pattern(x: u32, y: u32, channel: u32) -> f32 {{
                         return f32((x * 3u + y * 101u + channel * 37u) & 255u) / 255.0;
                     }}

                     @compute @workgroup_size(8, 8)
                     fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
                         let x = id.x;
                         let y = id.y;
                         textureStore(luma, id.xy, vec4(pattern(x, y, 0u), 0.0, 0.0, 1.0));
                         if x < {CHROMA_WIDTH}u && y < {CHROMA_HEIGHT}u {{
                             let uv = vec4(pattern(x, y, 1u), pattern(x, y, 2u), 0.0, 1.0);
                             textureStore(chroma, id.xy, uv);
                         }}
                     }}",
                    luma = planes[0].1,
                    chroma = planes[1].1,
                )
                .into(),
            ),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("planar storage"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let views = [wgpu::TextureAspect::Plane0, wgpu::TextureAspect::Plane1]
        .into_iter()
        .zip(planes)
        .map(|(aspect, (plane_format, _))| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(plane_format),
                aspect,
                usage: Some(wgpu::TextureUsages::STORAGE_BINDING),
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&views[0]),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&views[1]),
            },
        ],
    });

    // (aspect, width, height, the shader's channels, bytes a texel). Every row
    // is a multiple of 256 bytes.
    let plane_layouts = [
        (wgpu::TextureAspect::Plane0, WIDTH, HEIGHT, &[0][..]),
        (
            wgpu::TextureAspect::Plane1,
            CHROMA_WIDTH,
            CHROMA_HEIGHT,
            &[1, 2][..],
        ),
    ]
    .into_iter()
    .zip(planes)
    .map(|((aspect, width, height, channels), (plane_format, _))| {
        let bytes_per_texel = plane_format.block_copy_size(None).unwrap();
        (aspect, width, height, channels, bytes_per_texel)
    })
    .collect::<Vec<_>>();
    let readbacks = plane_layouts
        .iter()
        .map(|&(_, width, height, _, bytes_per_texel)| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(width * height * bytes_per_texel),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
        .collect::<Vec<_>>();

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(WIDTH / 8, HEIGHT / 8, 1);
    }
    for (&(aspect, width, height, _, bytes_per_texel), readback) in
        plane_layouts.iter().zip(&readbacks)
    {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * bytes_per_texel),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }
    ctx.queue.submit([encoder.finish()]);

    for readback in &readbacks {
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    }
    ctx.async_poll(wgpu::PollType::wait_indefinitely())
        .await
        .unwrap();

    for (&(aspect, width, height, channels, bytes_per_texel), readback) in
        plane_layouts.iter().zip(&readbacks)
    {
        // The shader's pattern, as each unorm channel stores it: 8 bits a channel
        // hold it as it is, 16 bits as `p * 257` (`p / 255 * 65535`).
        let bytes_per_channel = bytes_per_texel / channels.len() as u32;
        let expected = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| channels.iter().map(move |&c| (x, y, c)))
            .flat_map(|(x, y, c)| {
                let p = (x * 3 + y * 101 + c * 37) & 255;
                match bytes_per_channel {
                    1 => vec![p as u8],
                    2 => ((p * 257) as u16).to_le_bytes().to_vec(),
                    _ => unreachable!(),
                }
            })
            .collect::<Vec<u8>>();
        let data = readback.slice(..).get_mapped_range().unwrap().to_vec();
        assert!(
            data == expected,
            "{format:?} {aspect:?}: the stores did not land in the plane"
        );
    }
}

/// Ensures that creation and sampling of an NV12 format texture works as
/// expected.
#[apply(gpu_test!)]
static NV12_TEXTURE_CREATION_SAMPLING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(wgpu::Features::TEXTURE_FORMAT_NV12)
            .enable_noop(),
    )
    .run_sync(|ctx| {
        // Deliberately non-square so a width/height swap is caught. Both
        // dimensions stay even, as required by NV12/P010 (chroma is half-res).
        let size = wgpu::Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        };
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });
        let y_view = tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::R8Unorm),
            aspect: wgpu::TextureAspect::Plane0,
            ..Default::default()
        });
        let uv_view = tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rg8Unorm),
            aspect: wgpu::TextureAspect::Plane1,
            ..Default::default()
        });

        test_planar_texture_creation_sampling(&ctx, &y_view, &uv_view);
    });

/// Ensures that creation and sampling of a P010 format texture works as
/// expected.
#[apply(gpu_test!)]
static P010_TEXTURE_CREATION_SAMPLING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(
                wgpu::Features::TEXTURE_FORMAT_P010 | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
            )
            .enable_noop(),
    )
    .run_sync(|ctx| {
        // Deliberately non-square so a width/height swap is caught. Both
        // dimensions stay even, as required by NV12/P010 (chroma is half-res).
        let size = wgpu::Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        };
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::P010,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });
        let y_view = tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::R16Unorm),
            aspect: wgpu::TextureAspect::Plane0,
            ..Default::default()
        });
        let uv_view = tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rg16Unorm),
            aspect: wgpu::TextureAspect::Plane1,
            ..Default::default()
        });

        test_planar_texture_creation_sampling(&ctx, &y_view, &uv_view);
    });

/// Ensures that rendering on to NV12 format texture works as expected.
#[apply(gpu_test!)]
static NV12_TEXTURE_RENDERING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(wgpu::Features::TEXTURE_FORMAT_NV12)
            .enable_noop(),
    )
    .run_sync(|ctx| {
        // Deliberately non-square so a width/height swap is caught. Both
        // dimensions stay even, as required by NV12/P010 (chroma is half-res).
        let size = wgpu::Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        };
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });
        let y_view = tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::R8Unorm),
            aspect: wgpu::TextureAspect::Plane0,
            ..Default::default()
        });
        let uv_view = tex.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rg8Unorm),
            aspect: wgpu::TextureAspect::Plane1,
            ..Default::default()
        });

        test_planar_texture_rendering(
            &ctx,
            (&y_view, wgpu::TextureFormat::R8Unorm),
            (&uv_view, wgpu::TextureFormat::Rg8Unorm),
        );
    });

/// Ensures that copying NV12 texture to NV12 texture works as expected
#[apply(gpu_test!)]
static NV12_TEXTURE_COPYING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(wgpu::Features::TEXTURE_FORMAT_NV12)
            .enable_noop(),
    )
    .run_sync(|ctx| {
        // Deliberately non-square so a width/height swap is caught. Both
        // dimensions stay even, as required by NV12/P010 (chroma is half-res).
        let size = wgpu::Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        };
        let input_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::COPY_SRC,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });
        let output_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::COPY_DST,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });

        let mut command_encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        command_encoder.copy_texture_to_texture(
            input_texture.as_image_copy(),
            output_texture.as_image_copy(),
            size,
        );
        ctx.queue.submit([command_encoder.finish()]);
    });

/// Ensures that copying a single plane of an NV12 source into a matching
/// single-plane destination (Plane0 → R8Unorm, Plane1 → Rg8Unorm) round-trips
/// byte-for-byte. Exercises the planar→single-plane copy-compatibility
/// extension in `copy_texture_to_texture`.
#[apply(gpu_test!)]
static NV12_PLANE_TO_SINGLE_PLANE_COPY: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default().features(wgpu::Features::TEXTURE_FORMAT_NV12))
    .run_async(|ctx| async move {
        // Width chosen so that bytes-per-row is 256-aligned for both planes:
        //   luma   R8Unorm:  256 px * 1 byte/px = 256
        //   chroma Rg8Unorm: 128 px * 2 byte/px = 256
        // Height is deliberately different from width so a swap is caught.
        const WIDTH: u32 = 256;
        const HEIGHT: u32 = 128;
        let luma_size = wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        };
        let chroma_size = wgpu::Extent3d {
            width: WIDTH / 2,
            height: HEIGHT / 2,
            depth_or_array_layers: 1,
        };

        let nv12 = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("nv12 src"),
            dimension: wgpu::TextureDimension::D2,
            size: luma_size,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });

        // Distinct patterns per plane so a swap or plane-0-fallback would fail
        // the assertion at the end. Each pattern depends on both `x` and `y`
        // with different per-axis weights, so it is asymmetric under transpose,
        // horizontal mirror, and vertical mirror — a row/column mix-up changes
        // the bytes and fails the comparison.
        let luma_bytes: Vec<u8> = (0..HEIGHT)
            .flat_map(|y| {
                (0..WIDTH).map(move |x| x.wrapping_mul(3).wrapping_add(y.wrapping_mul(101)) as u8)
            })
            .collect();
        let chroma_bytes: Vec<u8> = (0..HEIGHT / 2)
            .flat_map(|y| {
                (0..WIDTH / 2).flat_map(move |x| {
                    let r = x.wrapping_mul(7).wrapping_add(y.wrapping_mul(53)) as u8;
                    let g = (x.wrapping_mul(29).wrapping_add(y.wrapping_mul(13)) ^ 0xA5) as u8;
                    [r, g]
                })
            })
            .collect();

        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &nv12,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::Plane0,
            },
            &luma_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH),
                rows_per_image: Some(HEIGHT),
            },
            luma_size,
        );
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &nv12,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::Plane1,
            },
            &chroma_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(WIDTH / 2 * 2),
                rows_per_image: Some(HEIGHT / 2),
            },
            chroma_size,
        );

        let r8 = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("r8 dst"),
            dimension: wgpu::TextureDimension::D2,
            size: luma_size,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });
        let rg8 = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rg8 dst"),
            dimension: wgpu::TextureDimension::D2,
            size: chroma_size,
            format: wgpu::TextureFormat::Rg8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });

        let r8_readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: luma_bytes.len() as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rg8_readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: chroma_bytes.len() as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

        // The path under test.
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &nv12,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::Plane0,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &r8,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            luma_size,
        );
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &nv12,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::Plane1,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &rg8,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            chroma_size,
        );

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &r8,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &r8_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH),
                    rows_per_image: Some(HEIGHT),
                },
            },
            luma_size,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &rg8,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &rg8_readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH / 2 * 2),
                    rows_per_image: Some(HEIGHT / 2),
                },
            },
            chroma_size,
        );

        ctx.queue.submit([encoder.finish()]);

        let r8_slice = r8_readback.slice(..);
        r8_slice.map_async(wgpu::MapMode::Read, |_| ());
        let rg8_slice = rg8_readback.slice(..);
        rg8_slice.map_async(wgpu::MapMode::Read, |_| ());
        ctx.async_poll(wgpu::PollType::wait_indefinitely())
            .await
            .unwrap();

        let r8_data: Vec<u8> = r8_slice.get_mapped_range().unwrap().to_vec();
        let rg8_data: Vec<u8> = rg8_slice.get_mapped_range().unwrap().to_vec();
        assert_eq!(r8_data, luma_bytes, "luma plane mismatch");
        assert_eq!(rg8_data, chroma_bytes, "chroma plane mismatch");
    });

/// Ensures that copying P010 texture to P010 texture works as expected
#[apply(gpu_test!)]
static P010_TEXTURE_COPYING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(
                wgpu::Features::TEXTURE_FORMAT_P010 | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
            )
            .enable_noop(),
    )
    .run_sync(|ctx| {
        // Deliberately non-square so a width/height swap is caught. Both
        // dimensions stay even, as required by NV12/P010 (chroma is half-res).
        let size = wgpu::Extent3d {
            width: 256,
            height: 128,
            depth_or_array_layers: 1,
        };
        let input_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::P010,
            usage: wgpu::TextureUsages::COPY_SRC,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });
        let output_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            dimension: wgpu::TextureDimension::D2,
            size,
            format: wgpu::TextureFormat::P010,
            usage: wgpu::TextureUsages::COPY_DST,
            mip_level_count: 1,
            sample_count: 1,
            view_formats: &[],
        });

        let mut command_encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        command_encoder.copy_texture_to_texture(
            input_texture.as_image_copy(),
            output_texture.as_image_copy(),
            size,
        );
        ctx.queue.submit([command_encoder.finish()]);
    });

/// Ensures that an NV12 texture takes `STORAGE_BINDING` only where the
/// adapter's format features allow it, and that stores through its plane views
/// land where they do.
#[apply(gpu_test!)]
static NV12_TEXTURE_STORAGE: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(
                wgpu::Features::TEXTURE_FORMAT_NV12
                    | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            )
            .limits(wgpu::Limits::downlevel_defaults())
            .downlevel_flags(wgpu::DownlevelFlags::COMPUTE_SHADERS),
    )
    .run_async(|ctx| async move {
        test_planar_texture_storage(
            &ctx,
            wgpu::TextureFormat::NV12,
            [
                (wgpu::TextureFormat::R8Unorm, "r8unorm"),
                (wgpu::TextureFormat::Rg8Unorm, "rg8unorm"),
            ],
        )
        .await;
    });

/// Ensures that a P010 texture takes `STORAGE_BINDING` only where the
/// adapter's format features allow it, and that stores through its plane views
/// land where they do.
#[apply(gpu_test!)]
static P010_TEXTURE_STORAGE: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(
                wgpu::Features::TEXTURE_FORMAT_P010
                    | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
                    | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            )
            .limits(wgpu::Limits::downlevel_defaults())
            .downlevel_flags(wgpu::DownlevelFlags::COMPUTE_SHADERS),
    )
    .run_async(|ctx| async move {
        test_planar_texture_storage(
            &ctx,
            wgpu::TextureFormat::P010,
            [
                (wgpu::TextureFormat::R16Unorm, "r16unorm"),
                (wgpu::TextureFormat::Rg16Unorm, "rg16unorm"),
            ],
        )
        .await;
    });
