//! A device that wgpu-hal did not create gives `device_from_raw` the features it was created
//! with, and wgpu-hal takes its paths from them: where the device lacks a feature that wgpu-hal
//! would use, it takes the path that does without it, and where it lacks a core feature behind a
//! downlevel flag, it opens without the flag. It refuses a requested feature whose Vulkan features
//! and extensions the device lacks.

use alloc::{string::String, sync::Arc, vec::Vec};
use core::{ffi::CStr, panic::AssertUnwindSafe, time::Duration};

use ash::vk;

use super::super::{call_log, Fence};
use super::injected::InstrumentedLock;
use crate::vulkan::call_log::Kind;
use crate::{Device as _, Queue as _, Surface as _};

/// The core features of `adapter`'s downlevel flags that it offers, and no other core feature.
fn downlevel_core(adapter: &super::super::Adapter) -> vk::PhysicalDeviceFeatures {
    let offered = adapter.get_physical_device_features().get_core();
    vk::PhysicalDeviceFeatures::default()
        .independent_blend(offered.independent_blend != 0)
        .sample_rate_shading(offered.sample_rate_shading != 0)
        .image_cube_array(offered.image_cube_array != 0)
        .sampler_anisotropy(offered.sampler_anisotropy != 0)
        .fragment_stores_and_atomics(offered.fragment_stores_and_atomics != 0)
        .depth_bias_clamp(offered.depth_bias_clamp != 0)
        .full_draw_index_uint32(offered.full_draw_index_uint32 != 0)
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

/// On each adapter with a headless surface, a device created without `VK_KHR_swapchain` cannot
/// configure the surface: an error, before wgpu-hal calls a swapchain function the device lacks.
/// Before, it loaded and called them, and panicked. With a scaling mode, the device's failure
/// comes before the scaling decision, which would query the surface for the device, and it
/// consumes the caller's swapchain creation chain. Before, the scaling decision came first, and
/// refused the mode, keeping the chain.
#[test]
fn a_device_without_the_swapchain_extension_cannot_configure_a_surface() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, surface)) = super::headless_instance(wgt::InstanceFlags::empty()) else {
        return;
    };
    let mut wrong = Vec::new();
    for (exposed, caps) in super::adapters(&instance, &surface) {
        let adapter = &exposed.adapter;
        let features = wgt::Features::empty();
        let mut extensions = adapter.required_device_extensions(features);
        extensions.retain(|&name| {
            name != ash::khr::swapchain::NAME
                && !super::super::adapter::SWAPCHAIN_MAINTENANCE1
                    .iter()
                    .any(|(maintenance, _)| *maintenance == name)
                && name != ash::khr::swapchain_mutable_format::NAME
        });
        let mut physical_features = adapter.physical_device_features(&extensions, features);
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
                super::super::DeviceQueue::new(0, 0),
            )
        }
        .unwrap();
        let pending = || {
            surface
                .inner
                .as_any()
                .downcast_ref::<super::super::swapchain::NativeSurface>()
                .unwrap()
                .has_pending_swapchain_create_chain()
        };
        // A caller's chain, which no swapchain is created with here.
        let mut format_list = vk::ImageFormatListCreateInfo::default();
        for scaling_mode in [
            wgt::SurfaceScalingMode::Auto,
            wgt::SurfaceScalingMode::Stretch,
        ] {
            let config = crate::SurfaceConfiguration {
                scaling_mode,
                ..super::config(&caps)
            };
            // SAFETY: A valid chain, which stays valid until the surface is dropped.
            unsafe { surface.set_next_swapchain_create_chain(<*mut _>::cast(&mut format_list)) };
            let configured = std::panic::catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: Nothing uses the surface.
                unsafe { surface.configure(&open.device, &config) }
            }));
            let name = &exposed.info.name;
            match configured {
                Err(_) => wrong.push(alloc::format!("{name:?}, {scaling_mode:?}: panicked")),
                Ok(Ok(())) => {
                    // SAFETY: Configured just now.
                    unsafe { surface.unconfigure(&open.device) };
                    wrong.push(alloc::format!("{name:?}, {scaling_mode:?}: configured"));
                }
                Ok(Err(crate::SurfaceError::Other(reason))) => {
                    std::println!("{name:?}, {scaling_mode:?}: refused: {reason}")
                }
                Ok(Err(other)) => {
                    wrong.push(alloc::format!("{name:?}, {scaling_mode:?}: {other:?}"))
                }
            }
            if pending() {
                wrong.push(alloc::format!(
                    "{name:?}, {scaling_mode:?}: the chain was kept"
                ));
            }
        }
        // SAFETY: Null.
        unsafe { surface.set_next_swapchain_create_chain(core::ptr::null_mut()) };
        drop(open);
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
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

/// On each adapter with a headless surface whose devices can have `swapchainMaintenance1`, a
/// device created with the swapchain maintenance extension but without its feature supports no
/// present scaling: it refuses a configuration with each scaling mode other than `Auto`, with
/// `SurfaceError::UnsupportedScalingMode`, before it creates a swapchain, which the feature would
/// need (VUID-VkSwapchainPresentScalingCreateInfoKHR-swapchainMaintenance1-10154), and takes one
/// with `Auto`, which asks for nothing, and presents. The surface needn't list the modes: the
/// device's refusal comes first. A configuration with a background colour, which Vulkan surfaces
/// do not support, is refused too. A refusal keeps the configured swapchain, which still
/// presents, and leaves the caller's swapchain creation chain set; a null chain withdraws it, and
/// the configuration with `Auto` consumes it. Before, a refused scaling mode came after the
/// configured swapchain was released, and left the surface unconfigured.
#[test]
fn a_device_without_swapchain_maintenance1_refuses_scaling_modes() {
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
                super::super::DeviceQueue::new(0, 0),
            )
        }
        .unwrap();
        let (device, queue) = (open.device, open.queue);
        std::println!(
            "{:?}: the surface lists the scaling modes {:?}",
            exposed.info.name,
            caps.scaling_modes
        );
        assert!(!device.supports_present_scaling());
        let config = super::config(&caps);
        // A caller's chain: an empty format list, which may extend `VkSwapchainCreateInfoKHR` on
        // a Vulkan 1.2 device (VUID-VkSwapchainCreateInfoKHR-pNext-pNext), as RADV's is.
        assert!(adapter.phd_capabilities.device_api_version >= vk::API_VERSION_1_2);
        let mut format_list = vk::ImageFormatListCreateInfo::default();
        let pending = || {
            surface
                .inner
                .as_any()
                .downcast_ref::<super::super::swapchain::NativeSurface>()
                .unwrap()
                .has_pending_swapchain_create_chain()
        };
        // SAFETY: The device's own fence.
        let fence = unsafe { device.create_fence() }.unwrap();
        let mut value = 0;
        // A configured swapchain, which every refusal below keeps.
        // SAFETY: Nothing uses the surface, which is not configured; the submission completes
        // before the next configuration.
        unsafe {
            surface.configure(&device, &config).unwrap();
            value += 1;
            super::present_frame(&device, &queue, &surface, &fence, value);
        }
        // SAFETY: A valid chain, which stays valid until the surface is dropped.
        unsafe { surface.set_next_swapchain_create_chain(<*mut _>::cast(&mut format_list)) };
        let refused = [
            wgt::SurfaceScalingMode::Stretch,
            wgt::SurfaceScalingMode::AspectRatio,
            wgt::SurfaceScalingMode::OneToOne,
        ]
        .map(|scaling_mode| crate::SurfaceConfiguration {
            scaling_mode,
            ..config.clone()
        })
        .into_iter()
        .chain([crate::SurfaceConfiguration {
            background_color: Some(wgt::SurfaceBackgroundColor {
                r: 0x2b,
                g: 0x33,
                b: 0x3d,
            }),
            ..config.clone()
        }]);
        for refused in refused {
            let asked = (refused.scaling_mode, refused.background_color);
            // SAFETY: Nothing uses the surface's swapchain: each frame's submission completed.
            match (asked.1, unsafe { surface.configure(&device, &refused) }) {
                (None, Err(crate::SurfaceError::UnsupportedScalingMode))
                | (Some(_), Err(crate::SurfaceError::Other(_))) => {}
                (_, Ok(())) => {
                    // SAFETY: Configured just now, and nothing uses it.
                    unsafe { surface.unconfigure(&device) };
                    panic!("{:?}: configured with {asked:?}", exposed.info.name);
                }
                (_, Err(other)) => panic!("{:?}: {asked:?}: {other:?}", exposed.info.name),
            }
            // The refusal leaves the caller's chain for the next configuration, and keeps the
            // configured swapchain, which presents a frame.
            assert!(pending(), "{:?}: {asked:?}", exposed.info.name);
            value += 1;
            // SAFETY: Still configured, as asserted by the present; the submission completes
            // before the next configuration.
            unsafe { super::present_frame(&device, &queue, &surface, &fence, value) };
        }
        // A null chain withdraws it.
        // SAFETY: Null.
        unsafe { surface.set_next_swapchain_create_chain(core::ptr::null_mut()) };
        assert!(!pending());
        // SAFETY: As above.
        unsafe { surface.set_next_swapchain_create_chain(<*mut _>::cast(&mut format_list)) };
        // SAFETY: Each call meets wgpu-hal's requirements: the submission completes before the
        // surface is configured again and unconfigured.
        unsafe {
            surface.configure(&device, &config).unwrap();
            // `Auto` consumed the chain.
            assert!(!pending());
            value += 1;
            super::present_frame(&device, &queue, &surface, &fence, value);
            surface.unconfigure(&device);
            queue.wait_for_idle().unwrap();
            device.destroy_fence(fence);
        }
        drop((queue, device));
    }
}

/// Gives `device_from_raw` `raw`, created on `adapter` with `extensions` and `enabled`, with
/// `features` requested, catching a refusal: then destroys `raw` and returns the refusal's message.
fn take(
    adapter: &super::super::Adapter,
    raw: ash::Device,
    extensions: &[&'static CStr],
    enabled: &vk::PhysicalDeviceFeatures2<'_>,
    features: wgt::Features,
) -> Result<crate::OpenDevice<super::super::Api>, String> {
    // SAFETY: The chain is valid.
    let enabled_features = unsafe { super::super::PhysicalDeviceFeatures::from_enabled(enabled) };
    let taken = std::panic::catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Created with these extensions and features; wgpu-hal destroys it once it takes
        // it.
        unsafe {
            adapter.device_from_raw(
                raw.clone(),
                None,
                extensions,
                &enabled_features,
                features,
                &wgt::Limits::downlevel_defaults(),
                &wgt::MemoryHints::default(),
                super::super::DeviceQueue::new(0, 0),
            )
        }
    }));
    match taken {
        Ok(open) => Ok(open.unwrap()),
        Err(payload) => {
            // `device_from_raw` refused the device before taking it: destroy it before its
            // instance.
            // SAFETY: Nothing uses it.
            unsafe { raw.destroy_device(None) };
            Err(payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default())
        }
    }
}

/// On each adapter, for each core feature behind a downlevel flag that the adapter offers, a
/// device created without it: `device_from_raw` takes it, and the device has its adapter's
/// downlevel capabilities without exactly that feature's flag, as
/// `Adapter::device_downlevel_capabilities` says. Before, `device_from_raw` refused such a device.
#[test]
fn a_device_without_a_downlevel_feature_opens_without_its_flag() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    let mut wrong = Vec::new();
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        let offered = exposed.capabilities.downlevel.flags;
        for (flag, feature) in super::super::adapter::DOWNLEVEL_CORE_FEATURES {
            if !offered.contains(flag) {
                std::println!("{:?}: skipped {flag:?}: not offered", exposed.info.name);
                continue;
            }
            let mut core = downlevel_core(adapter);
            *feature(&mut core) = vk::FALSE;
            let enabled = vk::PhysicalDeviceFeatures2::default().features(core);
            let (raw, extensions) = create(adapter, &instance, &enabled);
            // SAFETY: No chain.
            let expected = adapter.device_downlevel_capabilities(&extensions, &unsafe {
                super::super::PhysicalDeviceFeatures::from_enabled(&enabled)
            });
            match take(adapter, raw, &extensions, &enabled, wgt::Features::empty()) {
                Err(message) => wrong.push(alloc::format!(
                    "{:?}: refused a device without {flag:?}: {message}",
                    exposed.info.name
                )),
                Ok(open) => {
                    let flags = open.device.downlevel_capabilities().flags;
                    std::println!("{:?}: without {flag:?}: {flags:?}", exposed.info.name);
                    if flags != offered - flag || expected.flags != flags {
                        wrong.push(alloc::format!(
                            "{:?}: without {flag:?}, the device has {flags:?}, expected {:?}",
                            exposed.info.name,
                            offered - flag
                        ));
                    }
                    drop(open);
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The core features that FFmpeg copies into the devices it creates (`device_features_copy_needed`
/// in `libavutil/hwcontext_vulkan.c`), of those that `adapter` offers.
fn ffmpeg_core(adapter: &super::super::Adapter) -> vk::PhysicalDeviceFeatures {
    let offered = adapter.get_physical_device_features().get_core();
    vk::PhysicalDeviceFeatures::default()
        .shader_image_gather_extended(offered.shader_image_gather_extended != 0)
        .shader_storage_image_read_without_format(
            offered.shader_storage_image_read_without_format != 0,
        )
        .shader_storage_image_write_without_format(
            offered.shader_storage_image_write_without_format != 0,
        )
        .fragment_stores_and_atomics(offered.fragment_stores_and_atomics != 0)
        .vertex_pipeline_stores_and_atomics(offered.vertex_pipeline_stores_and_atomics != 0)
        .shader_int64(offered.shader_int64 != 0)
        .shader_int16(offered.shader_int16 != 0)
        .shader_float64(offered.shader_float64 != 0)
}

/// On each adapter, `device_from_raw` refuses a queue family that the physical device lacks with an
/// error, leaving the device with its caller: the drop callback is not called, and the caller
/// destroys the device.
#[test]
fn a_device_that_device_from_raw_refuses_stays_with_its_caller() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        let enabled = vk::PhysicalDeviceFeatures2::default();
        let (raw, extensions) = create(adapter, &instance, &enabled);
        // SAFETY: No chain.
        let enabled_features =
            unsafe { super::super::PhysicalDeviceFeatures::from_enabled(&enabled) };
        let called = Arc::new(core::sync::atomic::AtomicBool::new(false));
        let callback = {
            let called = Arc::clone(&called);
            alloc::boxed::Box::new(move || called.store(true, core::sync::atomic::Ordering::SeqCst))
        };
        // SAFETY: Created from this adapter with these extensions and features; the queue family
        // does not exist, which is what is tested.
        let opened = unsafe {
            adapter.device_from_raw(
                raw.clone(),
                Some(callback),
                &extensions,
                &enabled_features,
                wgt::Features::empty(),
                &wgt::Limits::downlevel_defaults(),
                &wgt::MemoryHints::default(),
                super::super::DeviceQueue::new(u32::MAX, 0),
            )
        };
        assert!(opened.is_err(), "{:?}", exposed.info.name);
        assert!(
            !called.load(core::sync::atomic::Ordering::SeqCst),
            "{:?}: the drop callback was called",
            exposed.info.name
        );
        // SAFETY: Still ours, and nothing uses it.
        unsafe { raw.destroy_device(None) };
    }
}

/// On each adapter, `open_with_callback` refuses, before creating the device, to open one whose
/// callback removed an extension that a requested feature needs, with an error: the device would
/// not give the feature. Before, it created the device and then panicked, leaking it.
#[test]
fn opening_refuses_a_device_that_a_callback_left_without_a_requested_feature() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    let mut wrong = Vec::new();
    let mut checked = 0;
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        let base = adapter.required_device_extensions(wgt::Features::empty());
        // A feature that needs an extension of its own.
        let Some((name, feature, extension)) =
            exposed.features.iter_names().find_map(|(name, feature)| {
                adapter
                    .required_device_extensions(feature)
                    .into_iter()
                    .find(|extension| !base.contains(extension))
                    .map(|extension| (name, feature, extension))
            })
        else {
            std::println!(
                "{:?}: skipped: no feature needs an extension",
                exposed.info.name
            );
            continue;
        };
        checked += 1;
        std::println!("{:?}: {name} without {extension:?}", exposed.info.name);
        let opened = std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: The callback removes an extension, against its contract, which is what is
            // tested: no device is created.
            unsafe {
                adapter.open_with_callback(
                    feature,
                    &wgt::Limits::downlevel_defaults(),
                    &wgt::MemoryHints::default(),
                    Some(alloc::boxed::Box::new(
                        move |args: super::super::CreateDeviceCallbackArgs<'_, '_, '_>| {
                            args.extensions.retain(|&other| other != extension);
                        },
                    )),
                )
            }
        }));
        match opened {
            Ok(Err(crate::DeviceError::Unexpected)) => {}
            Ok(Err(other)) => wrong.push(alloc::format!("{:?}: {other:?}", exposed.info.name)),
            Ok(Ok(_)) => wrong.push(alloc::format!("{:?}: opened", exposed.info.name)),
            Err(_) => wrong.push(alloc::format!("{:?}: panicked", exposed.info.name)),
        }
    }
    std::println!("{checked} adapters checked");
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// On each adapter, a device created with FFmpeg's core features, the only one of the seven
/// behind downlevel flags that it has being `fragmentStoresAndAtomics`: it opens, without the
/// flags of the other six, and submits work. Before, `device_from_raw` refused it.
#[test]
fn a_device_with_ffmpegs_features_opens() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    let lacking = wgt::DownlevelFlags::INDEPENDENT_BLEND
        | wgt::DownlevelFlags::MULTISAMPLED_SHADING
        | wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES
        | wgt::DownlevelFlags::ANISOTROPIC_FILTERING
        | wgt::DownlevelFlags::DEPTH_BIAS_CLAMP
        | wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32;
    let mut wrong = Vec::new();
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        let offered = exposed.capabilities.downlevel.flags;
        let enabled = vk::PhysicalDeviceFeatures2::default().features(ffmpeg_core(adapter));
        let (raw, extensions) = create(adapter, &instance, &enabled);
        let open = match take(adapter, raw, &extensions, &enabled, wgt::Features::empty()) {
            Ok(open) => open,
            Err(message) => {
                wrong.push(alloc::format!(
                    "{:?}: refused: {message}",
                    exposed.info.name
                ));
                continue;
            }
        };
        let (device, queue) = (open.device, open.queue);
        let flags = device.downlevel_capabilities().flags;
        std::println!("{:?}: {flags:?}", exposed.info.name);
        if flags != offered - lacking {
            wrong.push(alloc::format!(
                "{:?}: the device has {flags:?}, expected {:?}",
                exposed.info.name,
                offered - lacking
            ));
        }
        // SAFETY: The device's own fence, signaled by a submission that completes before it is
        // destroyed.
        unsafe {
            let fence = device.create_fence().unwrap();
            queue.submit(&[], &[], (&fence, 1)).unwrap();
            assert!(device
                .wait(&fence, 1, Some(Duration::from_secs(5)))
                .unwrap());
            device.destroy_fence(fence);
        }
        drop((queue, device));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// On each adapter, every feature that it offers round-trips: a device that wgpu-hal would open
/// with it alone, or with all of them, has it among `Adapter::device_features`, so that
/// `device_from_raw` takes the device; and wgpu-hal opens a device with all of them.
#[test]
fn every_adapter_feature_round_trips() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    let mut wrong = Vec::new();
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        let offered = exposed.features;
        let mut requests = offered.iter().collect::<Vec<_>>();
        requests.push(offered);
        for requested in &requests {
            let extensions = adapter.required_device_extensions(*requested);
            let enabled = adapter.physical_device_features(&extensions, *requested);
            let lacking = *requested - adapter.device_features(&extensions, &enabled, 0);
            if !lacking.is_empty() {
                wrong.push(alloc::format!(
                    "{:?}: opened with {requested:?}, lacking {lacking:?}",
                    exposed.info.name
                ));
            }
        }
        // SAFETY: Features that the adapter offers; the device is dropped before the instance.
        let opened = unsafe {
            crate::Adapter::open(
                adapter,
                offered,
                &wgt::Limits::downlevel_defaults(),
                &wgt::MemoryHints::default(),
            )
        };
        match opened {
            Ok(open) => drop(open),
            Err(error) => wrong.push(alloc::format!(
                "{:?}: opening with every feature failed: {error}",
                exposed.info.name
            )),
        }
        std::println!(
            "{:?}: {} features, each alone and all together",
            exposed.info.name,
            requests.len() - 1
        );
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// On each adapter, a device created without what a feature that the adapter offers needs, and
/// given `device_from_raw` with that feature requested: `device_from_raw` refuses it before taking
/// it, naming the feature. The features are the first offered of `TEXTURE_COMPRESSION_BC`,
/// `SHADER_F64`, `DEPTH_CLIP_CONTROL` and `POLYGON_MODE_LINE`, whose core features the device
/// lacks, and `CORE_FEATURES_AND_LIMITS` on a device without `independentBlend`. Before,
/// `device_from_raw` took the requested features as the caller's word.
#[test]
fn a_request_for_a_feature_the_device_lacks_is_refused_by_name() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    let mut wrong = Vec::new();
    // SAFETY: No requirements.
    for exposed in unsafe { crate::Instance::enumerate_adapters(&instance, None) } {
        let adapter = &exposed.adapter;
        let mut cases = Vec::new();
        if let Some(feature) = [
            wgt::Features::TEXTURE_COMPRESSION_BC,
            wgt::Features::SHADER_F64,
            wgt::Features::DEPTH_CLIP_CONTROL,
            wgt::Features::POLYGON_MODE_LINE,
        ]
        .into_iter()
        .find(|&feature| exposed.features.contains(feature))
        {
            cases.push((feature, downlevel_core(adapter)));
        }
        if exposed
            .features
            .contains(wgt::Features::CORE_FEATURES_AND_LIMITS)
        {
            cases.push((
                wgt::Features::CORE_FEATURES_AND_LIMITS,
                downlevel_core(adapter).independent_blend(false),
            ));
        }
        for (feature, core) in cases {
            let name = feature.iter_names().next().unwrap().0;
            let enabled = vk::PhysicalDeviceFeatures2::default().features(core);
            let (raw, extensions) = create(adapter, &instance, &enabled);
            match take(adapter, raw, &extensions, &enabled, feature) {
                Ok(open) => {
                    drop(open);
                    wrong.push(alloc::format!(
                        "{:?}: took a device without what {name} needs",
                        exposed.info.name
                    ));
                }
                Err(message) => {
                    std::println!("{:?}: refused: {message}", exposed.info.name);
                    if !message.contains(name) {
                        wrong.push(alloc::format!(
                            "{:?}: refused without naming {name}: {message}",
                            exposed.info.name
                        ));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
