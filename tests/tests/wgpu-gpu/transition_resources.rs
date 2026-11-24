use wgpu_test::{apply, gpu_test, GpuTestConfiguration, GpuTestInitializer, TestParameters};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.push(TRANSITION_RESOURCES);
    vec.push(TRANSITION_TO_AND_FROM_NO_USAGE);
}

#[apply(gpu_test!)]
static TRANSITION_RESOURCES: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default().enable_noop())
    .run_sync(|ctx| {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 32,
                height: 32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
            texture_binding_view_dimension: None,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        encoder.transition_resources(
            std::iter::empty(),
            [wgpu::TextureTransition {
                texture: &texture,
                selector: None,
                state: wgpu::TextureUses::COLOR_TARGET,
            }]
            .into_iter(),
        );

        ctx.queue.submit([encoder.finish()]);
    });

/// Transitions into and out of no usage keep the contents and pass validation. On Vulkan, each is
/// the only texture barrier of its submission, so its usages alone give its stage masks, which
/// must not be empty without `synchronization2` (VUID-vkCmdPipelineBarrier-srcStageMask-03937).
#[apply(gpu_test!)]
static TRANSITION_TO_AND_FROM_NO_USAGE: GpuTestConfiguration = GpuTestConfiguration::new()
    .run_async(|ctx| async move {
        // 256 bytes a row, the copy alignment.
        let size = wgpu::Extent3d {
            width: 64,
            height: 4,
            depth_or_array_layers: 1,
        };
        let bytes_per_row = 4 * size.width;
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
            texture_binding_view_dimension: None,
        });
        let contents = (0..bytes_per_row * size.height)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        ctx.queue.write_texture(
            texture.as_image_copy(),
            &contents,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
            size,
        );

        // From the write's usage into no usage.
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.transition_resources(
            std::iter::empty(),
            [wgpu::TextureTransition {
                texture: &texture,
                selector: None,
                state: wgpu::TextureUses::empty(),
            }]
            .into_iter(),
        );
        ctx.queue.submit([encoder.finish()]);

        // Out of no usage into a copy's.
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(bytes_per_row * size.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: None,
                },
            },
            size,
        );
        ctx.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
        ctx.async_poll(wgpu::PollType::wait_indefinitely())
            .await
            .unwrap();
        assert!(
            slice.get_mapped_range().unwrap()[..] == contents[..],
            "the contents changed"
        );
    });
