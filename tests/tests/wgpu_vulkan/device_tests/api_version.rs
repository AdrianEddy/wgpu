//! The Vulkan version an instance was created for (`VkApplicationInfo::apiVersion`): the instance
//! reads it back, and its physical devices are capped at it whatever their drivers support. On an
//! instance created for Vulkan 1.0, they use no functionality of a later core version, nor an
//! extension whose dependencies a 1.0 device cannot meet.

use ash::vk;
use wgpu_hal::api::Vulkan;

/// Vulkan `version` as `major.minor.patch`.
fn dotted(version: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(version),
        vk::api_version_minor(version),
        vk::api_version_patch(version)
    )
}

/// wgpu-hal creates its own instance for Vulkan 1.3 where the loader supports 1.1 or later, else
/// for 1.0, and the instance reads that version back, not the loader's: its adapters are capped at
/// it.
#[test]
fn wgpu_hals_own_instance_reads_the_version_it_was_created_for() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    // SAFETY: Loading the Vulkan library has no requirements.
    let Ok(entry) = (unsafe { ash::Entry::load() }) else {
        println!("skipped: no Vulkan");
        return;
    };
    // SAFETY: No requirements.
    let loader = unsafe { entry.try_enumerate_instance_version() }
        .unwrap()
        .unwrap_or(vk::API_VERSION_1_0);
    let created_for = if loader < vk::API_VERSION_1_1 {
        vk::API_VERSION_1_0
    } else {
        vk::API_VERSION_1_3
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // SAFETY: Only read.
    let Some(hal) = (unsafe { instance.as_hal::<Vulkan>() }) else {
        println!("skipped: no Vulkan instance");
        return;
    };
    let shared = hal.shared_instance();
    println!(
        "loader {}, instance created for {}",
        dotted(loader),
        dotted(shared.instance_api_version())
    );
    assert_eq!(shared.instance_api_version(), created_for);
    for adapter in super::adapters(&instance) {
        // SAFETY: Only read.
        let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
        let own = hal.physical_device_capabilities().properties().api_version;
        let effective = shared.effective_device_version(own);
        println!(
            "{:?}: its own version {}, effective {}",
            adapter.get_info().name,
            dotted(own),
            dotted(effective)
        );
        assert!(effective <= created_for);
    }
}

/// On each adapter of an instance created for Vulkan 1.0, with and without
/// `VK_KHR_get_physical_device_properties2`, under the validation layer: the adapter is capped at
/// 1.0, and a device opens, renders a pass, copies a buffer and waits for it, without a
/// validation error. Before, wgpu-hal took the driver's version: timeline semaphores, Vulkan 1.1's
/// negative viewport height and 1.1 to 1.3 structures on a device that may use none of them.
#[test]
fn an_instance_created_for_vulkan_1_0_works_without_a_validation_error() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    for with_properties2 in [true, false] {
        let Some((instance, messenger)) = super::instance_created_for(
            wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG,
            Some(vk::API_VERSION_1_0),
            with_properties2,
        ) else {
            return;
        };
        let Some(messenger) = messenger else {
            println!("skipped: no validation layer");
            return;
        };
        for adapter in super::adapters(&instance) {
            let name = adapter.get_info().name;
            {
                // SAFETY: Only read.
                let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
                let own = hal.physical_device_capabilities().properties().api_version;
                let effective = hal.shared_instance().effective_device_version(own);
                println!(
                    "{name:?}: its own version {}, effective {}",
                    dotted(own),
                    dotted(effective)
                );
                assert!(effective < vk::API_VERSION_1_1, "{name:?}: not capped");
            }
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                    .unwrap();
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 4,
                    height: 4,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
                texture_binding_view_dimension: None,
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: None,
                    },
                },
                texture.size(),
            );
            queue.submit([encoder.finish()]);
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, |result| result.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            assert_eq!(
                buffer.slice(..4).get_mapped_range().unwrap()[..],
                [0, 255, 0, 255]
            );
            println!("{name:?}, properties2 extension {with_properties2}: a pass rendered");
        }
        let messages = messenger.take();
        drop(messenger);
        assert!(
            messages.is_empty(),
            "properties2 extension {with_properties2}: {messages:#?}"
        );
    }
}
