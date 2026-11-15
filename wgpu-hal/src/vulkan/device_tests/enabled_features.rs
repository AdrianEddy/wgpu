//! A device that wgpu-hal did not create gives `device_from_raw` the features it was created
//! with, and wgpu-hal takes its paths from them: where the device lacks a feature that wgpu-hal
//! would use, it takes the path that does without it, and it refuses a device that lacks a core
//! feature behind the adapter's downlevel flags.

use alloc::{string::String, sync::Arc, vec::Vec};
use core::{ffi::CStr, panic::AssertUnwindSafe, time::Duration};

use ash::vk;

use super::super::{call_log, Fence};
use super::injected::InstrumentedLock;
use crate::vulkan::call_log::Kind;
use crate::{Device as _, Queue as _, Surface as _};

/// The core features of `adapter`'s downlevel flags that it offers, which `device_from_raw`
/// requires, and no other core feature.
fn downlevel_core(adapter: &super::super::Adapter) -> vk::PhysicalDeviceFeatures {
    let offered = adapter.get_physical_device_features().get_core();
    vk::PhysicalDeviceFeatures::default()
        .independent_blend(offered.independent_blend != 0)
        .sample_rate_shading(offered.sample_rate_shading != 0)
        .image_cube_array(offered.image_cube_array != 0)
        .sampler_anisotropy(offered.sampler_anisotropy != 0)
        .fragment_stores_and_atomics(offered.fragment_stores_and_atomics != 0)
        .depth_bias_clamp(offered.depth_bias_clamp != 0)
}

/// Creates a device on `adapter`, on queue 0 of family 0, with wgpu-hal's required extensions for
/// no feature and the features of `enabled`, given as a creator other than wgpu-hal gives them:
/// as a `VkPhysicalDeviceFeatures2` chain in `VkDeviceCreateInfo::pNext`. Returns it with its
/// extensions.
fn create(
    adapter: &super::super::Adapter,
    instance: &super::super::Instance,
    enabled: &vk::PhysicalDeviceFeatures2<'_>,
) -> (ash::Device, Vec<&'static CStr>) {
    let extensions = adapter.required_device_extensions(wgt::Features::empty());
    let priorities = [1.0];
    let queues = [vk::DeviceQueueCreateInfo::default()
        .queue_family_index(0)
        .queue_priorities(&priorities)];
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let mut info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queues)
        .enabled_extension_names(&names);
    let enabled: *const vk::PhysicalDeviceFeatures2<'_> = enabled;
    info.p_next = enabled.cast();
    // SAFETY: A valid create info, whose features the adapter offers.
    let raw = unsafe {
        instance
            .shared
            .raw
            .create_device(adapter.raw_physical_device(), &info, None)
    }
    .unwrap();
    (raw, extensions)
}

/// What an adapter offers of the features that wgpu-hal would use, for the test's output, which
/// shows that the test discriminates on it.
fn offered(adapter: &super::super::Adapter) -> String {
    let caps = &adapter.private_caps;
    alloc::format!(
        "the adapter offers timelineSemaphore: {}, robustBufferAccess2: {}, robustImageAccess: {}, \
         shaderZeroInitializeWorkgroupMemory: {}, shaderIntegerDotProduct: {}, shaderInt8: {}, \
         multiDrawIndirect: {}, swapchainMaintenance1: {}",
        caps.timeline_semaphores,
        caps.robust_buffer_access2,
        caps.robust_image_access,
        caps.zero_initialize_workgroup_memory,
        caps.shader_integer_dot_product,
        caps.shader_int8,
        caps.multi_draw_indirect,
        adapter
            .phd_features
            .swapchain_maintenance1
            .is_some_and(|features| features.swapchain_maintenance1 == vk::TRUE),
    )
}

/// On each adapter, a device created without any feature that wgpu-hal would use, but with the
/// core features behind the adapter's downlevel flags, and with the Vulkan 1.1, 1.2 and 1.3
/// feature structures, all off, in its chain: wgpu-hal takes the paths that do without them. Its
/// fences are binary fences, which a submission signals and a wait sees; Naga injects bounds
/// checks, polyfills zero-initialization, and declares neither dot product nor 8-bit integer
/// capabilities; and presents signal no fence, although the device has the swapchain maintenance
/// extension where the adapter lists it. Every adapter here offers timeline semaphores, so the
/// test fails on each of them if wgpu-hal follows the adapter.
#[test]
fn a_device_without_the_features_wgpu_hal_would_use_takes_the_paths_without_them() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        if adapter.phd_capabilities.device_api_version < vk::API_VERSION_1_3 {
            std::println!("skipped {:?}: not Vulkan 1.3", exposed.info.name);
            continue;
        }
        std::println!("{:?}: {}", exposed.info.name, offered(adapter));
        let mut vulkan11 = vk::PhysicalDeviceVulkan11Features::default();
        let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default();
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default();
        let enabled = vk::PhysicalDeviceFeatures2::default()
            .features(downlevel_core(adapter))
            .push_next(&mut vulkan11)
            .push_next(&mut vulkan12)
            .push_next(&mut vulkan13);
        let (raw, extensions) = create(adapter, &instance, &enabled);
        // SAFETY: The chain is valid.
        let enabled_features =
            unsafe { super::super::PhysicalDeviceFeatures::from_enabled(&enabled) };
        // SAFETY: Created above with these extensions and features; wgpu-hal destroys it.
        let open = unsafe {
            adapter.device_from_raw(
                raw,
                None,
                &extensions,
                &enabled_features,
                wgt::Features::empty(),
                &wgt::Limits::downlevel_defaults(),
                &wgt::MemoryHints::default(),
                super::super::DeviceQueue::new(0, 0),
            )
        }
        .unwrap();
        let (device, queue) = (open.device, open.queue);

        let caps = &device.shared.private_caps;
        assert!(!caps.timeline_semaphores);
        assert!(!caps.robust_buffer_access);
        assert!(!caps.robust_buffer_access2);
        assert!(!caps.robust_image_access);
        assert!(!caps.robust_image_access2);
        assert!(!caps.zero_initialize_workgroup_memory);
        assert!(!caps.shader_integer_dot_product);
        assert!(!caps.shader_int8);
        assert!(!caps.multi_draw_indirect);
        assert!(
            !device.presents_signal_fences(),
            "presents signal fences without swapchainMaintenance1"
        );

        let options = &device.naga_options;
        assert_eq!(
            options.bounds_check_policies.buffer,
            naga::proc::BoundsCheckPolicy::Restrict
        );
        assert_eq!(
            options.bounds_check_policies.image_load,
            naga::proc::BoundsCheckPolicy::Restrict
        );
        assert!(matches!(
            options.zero_initialize_workgroup_memory,
            naga::back::spv::ZeroInitializeWorkgroupMemoryMode::Polyfill
        ));
        let capabilities = options.capabilities.as_ref().unwrap();
        assert!(!capabilities.contains(&naga::back::spv::Capability::DotProductKHR));
        assert!(!capabilities.contains(&naga::back::spv::Capability::Int8));
        assert_eq!(
            capabilities.contains(&naga::back::spv::Capability::SampledCubeArray),
            enabled.features.image_cube_array != 0
        );

        // SAFETY: The device's own fence, signaled by a submission that completes before it is
        // destroyed.
        unsafe {
            let fence = device.create_fence().unwrap();
            assert!(
                matches!(fence, Fence::FencePool { .. }),
                "a timeline semaphore without timelineSemaphore: {fence:?}"
            );
            queue.submit(&[], &[], (&fence, 1)).unwrap();
            assert!(device
                .wait(&fence, 1, Some(Duration::from_secs(5)))
                .unwrap());
            device.destroy_fence(fence);
        }
        drop((queue, device));
    }
}

/// On each adapter with a headless surface whose devices can have `swapchainMaintenance1`, a
/// device created with the swapchain maintenance extension but without its feature, with a lock
/// that the application gave: presents carry no fence, and the swapchain's teardown waits for the
/// queue to become idle, holding the lock, as on a device without the extension.
#[test]
fn a_device_with_the_extension_but_not_swapchain_maintenance1_waits_for_the_queue() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, surface)) = super::headless_instance(wgt::InstanceFlags::empty()) else {
        return;
    };
    for (exposed, caps) in super::adapters(&instance, &surface) {
        let adapter = &exposed.adapter;
        let features = wgt::Features::empty();
        let extensions = adapter.required_device_extensions(features);
        if !super::super::adapter::SWAPCHAIN_MAINTENANCE1
            .iter()
            .any(|(name, _)| extensions.contains(name))
        {
            std::println!(
                "skipped {:?}: no swapchain maintenance extension",
                exposed.info.name
            );
            continue;
        }
        // wgpu-hal's own features for no feature, but with `swapchainMaintenance1` off.
        let mut physical_features = adapter.physical_device_features(&extensions, features);
        physical_features.swapchain_maintenance1 =
            Some(vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default());
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(0)
            .queue_priorities(&priorities)];
        let names = extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect::<Vec<_>>();
        let info = physical_features.add_to_device_create(
            vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&names),
        );
        // SAFETY: A valid create info, whose extensions and features the adapter offers.
        let raw = unsafe {
            instance
                .shared
                .raw
                .create_device(adapter.raw_physical_device(), &info, None)
        }
        .unwrap();
        let lock = Arc::new(InstrumentedLock::default());
        // SAFETY: Created above with these extensions and features; wgpu-hal destroys it.
        let open = unsafe {
            adapter.device_from_raw(
                raw,
                None,
                &extensions,
                &physical_features,
                features,
                &wgt::Limits::downlevel_defaults(),
                &wgt::MemoryHints::default(),
                super::super::DeviceQueue::new(0, 0).lock(lock),
            )
        }
        .unwrap();
        let (device, queue) = (open.device, open.queue);
        std::println!("{:?}: {}", exposed.info.name, super::teardown_path(&device));
        let presents_signal_fences = device.presents_signal_fences();
        let raw_queue = queue.as_raw();
        let config = super::config(&caps);
        // SAFETY: The device's own fence.
        let fence = unsafe { device.create_fence() }.unwrap();
        call_log::take();

        // SAFETY: Each call meets wgpu-hal's requirements: every submission completes before the
        // surface is unconfigured, and before the idle wait.
        unsafe {
            surface.configure(&device, &config).unwrap();
            super::present_frame(&device, &queue, &surface, &fence, 1);
            super::present_frame(&device, &queue, &surface, &fence, 2);
            call_log::mark("unconfigure");
            surface.unconfigure(&device);
            call_log::mark("unconfigured");
            queue.wait_for_idle().unwrap();
            device.destroy_fence(fence);
        }
        drop((queue, device));

        let calls = call_log::take().calls;
        assert!(
            calls
                .iter()
                .filter(|call| call.name == "vkQueuePresentKHR")
                .all(|present| present.fences.is_empty()),
            "a present carries a fence: {calls:#?}"
        );
        let teardowns = super::teardowns(&calls);
        let [teardown] = &teardowns[..] else {
            panic!("{calls:#?}");
        };
        let teardown = &calls[teardown.clone()];
        assert!(
            teardown.iter().any(|call| call.name == "vkQueueWaitIdle"
                && call.queue == raw_queue
                && call.locked),
            "the teardown did not wait for the queue, holding the lock: {teardown:#?}"
        );
        assert!(
            !teardown.iter().any(|call| call.kind == Kind::FenceWait),
            "the teardown waited on a fence: {teardown:#?}"
        );
        assert!(!presents_signal_fences);
        super::check_destroyed(&calls);
    }
}

/// On each adapter whose downlevel flags have `CUBE_ARRAY_TEXTURES`, a device created without
/// `imageCubeArray`: `missing_downlevel_features` names it, and `device_from_raw` refuses the
/// device, naming it, before taking it.
#[test]
fn a_device_without_a_downlevel_feature_of_its_adapter_is_refused() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        if !exposed
            .capabilities
            .downlevel
            .flags
            .contains(wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES)
        {
            std::println!("skipped {:?}: no cube array textures", exposed.info.name);
            continue;
        }
        let enabled = vk::PhysicalDeviceFeatures2::default()
            .features(downlevel_core(adapter).image_cube_array(false));
        let (raw, extensions) = create(adapter, &instance, &enabled);
        // SAFETY: No chain.
        let enabled_features =
            unsafe { super::super::PhysicalDeviceFeatures::from_enabled(&enabled) };
        assert_eq!(
            adapter.missing_downlevel_features(&enabled_features),
            ["imageCubeArray"]
        );
        let taken = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: Created above with these extensions and features. wgpu-hal refuses it
            // before taking it.
            unsafe {
                adapter.device_from_raw(
                    raw.clone(),
                    None,
                    &extensions,
                    &enabled_features,
                    wgt::Features::empty(),
                    &wgt::Limits::downlevel_defaults(),
                    &wgt::MemoryHints::default(),
                    super::super::DeviceQueue::new(0, 0),
                )
            }
        }));
        let payload = match taken {
            // wgpu-hal took the device, and destroys it as the result drops.
            Ok(open) => {
                drop(open);
                panic!(
                    "device_from_raw took a device without imageCubeArray on {:?}",
                    exposed.info.name
                );
            }
            Err(payload) => payload,
        };
        // `device_from_raw` refused the device before taking it: destroy it before its instance.
        // SAFETY: Nothing uses it.
        unsafe { raw.destroy_device(None) };
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        std::println!("{:?}: refused: {message}", exposed.info.name);
        assert!(message.contains("imageCubeArray"), "{message}");
    }
}
