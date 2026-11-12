//! Where the device has `swapchainMaintenance1`, each present signals a fence, and the swapchain's
//! teardown waits on its presents' fences, without the queue lock. Without it, or when a caller's
//! present chain gives a present its own fence, the teardown waits for the queue to become idle,
//! holding the lock.

use alloc::{sync::Arc, vec::Vec};
use core::{ffi::CStr, time::Duration};

use ash::vk;

use super::injected::{open, InstrumentedLock};
use crate::vulkan::call_log::{self, Kind};
use crate::{Device as _, Instance as _, Queue as _, Surface as _};

/// The presents in `calls`.
fn presents(calls: &[call_log::Call]) -> Vec<&call_log::Call> {
    calls
        .iter()
        .filter(|call| call.name == "vkQueuePresentKHR")
        .collect()
}

/// Checks that `teardown` waited for `queue` to become idle, holding the lock, and on no fence
/// holding it.
fn check_waited_for_the_queue(teardown: &[call_log::Call], queue: vk::Queue) {
    assert!(
        teardown
            .iter()
            .any(|call| call.name == "vkQueueWaitIdle" && call.queue == queue && call.locked),
        "the teardown did not wait for the queue, holding the lock: {teardown:#?}"
    );
    assert!(
        !teardown
            .iter()
            .any(|call| call.kind == Kind::FenceWait && call.locked),
        "the teardown waited on a fence holding the lock: {teardown:#?}"
    );
}

/// On each adapter whose devices have `swapchainMaintenance1`, with a lock that the application
/// gave: every present carries one fence; a teardown after presents waits on their fences, the
/// last one's among them, without the lock, and makes no queue command; a teardown with no
/// present since the configuration waits for nothing.
#[test]
fn the_teardown_waits_on_its_presents_fences_without_the_lock() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    for flags in super::FLAGS {
        std::println!("{flags:?}");
        let Some((instance, surface)) = super::headless_instance(flags) else {
            return;
        };
        for (exposed, caps) in super::adapters(&instance, &surface) {
            let lock = Arc::new(InstrumentedLock::default());
            let (device, queue) = open(&exposed.adapter, &instance, lock, true);
            if !device.presents_signal_fences() {
                std::println!(
                    "skipped {:?}: presents signal no fences (no swapchainMaintenance1)",
                    exposed.info.name
                );
                continue;
            }
            std::println!("{:?}: {}", exposed.info.name, super::teardown_path(&device));
            let config = super::config(&caps);
            // SAFETY: The device's own fence.
            let fence = unsafe { device.create_fence() }.unwrap();
            call_log::take();

            // SAFETY: Each call meets wgpu-hal's requirements: every submission completes before
            // the surface is unconfigured, and before the idle wait.
            unsafe {
                surface.configure(&device, &config).unwrap();
                for value in 1..=3 {
                    super::present_frame(&device, &queue, &surface, &fence, value);
                }
                call_log::mark("unconfigure");
                surface.unconfigure(&device);
                call_log::mark("unconfigured");
                surface.configure(&device, &config).unwrap();
                call_log::mark("unconfigure");
                surface.unconfigure(&device);
                call_log::mark("unconfigured");
                queue.wait_for_idle().unwrap();
                device.destroy_fence(fence);
            }
            drop((queue, device));

            let calls = call_log::take().calls;
            let presents = presents(&calls);
            assert_eq!(presents.len(), 3, "{calls:#?}");
            for present in &presents {
                assert_eq!(present.fences.len(), 1, "{present:#?}");
                assert!(present.locked, "{present:#?}");
            }
            let teardowns = super::teardowns(&calls);
            assert_eq!(teardowns.len(), 2, "{calls:#?}");

            let teardown = &calls[teardowns[0].clone()];
            assert!(
                !teardown.iter().any(|call| call.kind == Kind::Queue),
                "the teardown made a queue command: {teardown:#?}"
            );
            let waits = teardown
                .iter()
                .filter(|call| call.kind == Kind::FenceWait)
                .collect::<Vec<_>>();
            let [wait] = waits[..] else {
                panic!("the teardown did not wait on fences once: {teardown:#?}");
            };
            assert!(!wait.locked, "{wait:#?}");
            assert!(wait.fences.contains(&presents[2].fences[0]), "{calls:#?}");
            assert!(
                wait.fences.iter().all(|fence| presents
                    .iter()
                    .any(|present| present.fences.contains(fence))),
                "the teardown waited on a fence of no present: {calls:#?}"
            );

            let teardown = &calls[teardowns[1].clone()];
            assert!(
                !teardown
                    .iter()
                    .any(|call| matches!(call.kind, Kind::Queue | Kind::FenceWait)),
                "a teardown with no present waited: {teardown:#?}"
            );
            super::check_destroyed(&calls);
        }
    }
}

/// On each adapter, with a device made without the swapchain maintenance extensions: presents
/// carry no fence, and the teardown waits for the queue to become idle, holding the lock.
#[test]
fn without_present_fences_the_teardown_waits_for_the_queue_holding_the_lock() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, surface)) = super::headless_instance(wgt::InstanceFlags::empty()) else {
        return;
    };
    for (exposed, caps) in super::adapters(&instance, &surface) {
        let lock = Arc::new(InstrumentedLock::default());
        let (device, queue) = open(&exposed.adapter, &instance, lock, false);
        assert!(!device.presents_signal_fences());
        std::println!("{:?}: {}", exposed.info.name, super::teardown_path(&device));
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
        let presents = presents(&calls);
        assert_eq!(presents.len(), 2, "{calls:#?}");
        assert!(
            presents.iter().all(|present| present.fences.is_empty()),
            "{calls:#?}"
        );
        let teardowns = super::teardowns(&calls);
        let [teardown] = &teardowns[..] else {
            panic!("{calls:#?}");
        };
        let teardown = &calls[teardown.clone()];
        check_waited_for_the_queue(teardown, raw_queue);
        assert!(
            !teardown.iter().any(|call| call.kind == Kind::FenceWait),
            "the teardown waited on a fence: {teardown:#?}"
        );
        super::check_destroyed(&calls);
    }
}

/// On each adapter whose devices have `swapchainMaintenance1`: a present whose caller's chain
/// gives it its own fence carries that fence alone, and the teardown, whose fences no longer cover
/// every present, waits on the fence of the present before without the lock, then for the queue
/// to become idle, holding the lock.
#[test]
fn a_present_given_the_callers_fence_makes_the_teardown_wait_for_the_queue() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, surface)) = super::headless_instance(wgt::InstanceFlags::empty()) else {
        return;
    };
    for (exposed, caps) in super::adapters(&instance, &surface) {
        let lock = Arc::new(InstrumentedLock::default());
        let (device, queue) = open(&exposed.adapter, &instance, lock, true);
        if !device.presents_signal_fences() {
            std::println!(
                "skipped {:?}: presents signal no fences (no swapchainMaintenance1)",
                exposed.info.name
            );
            continue;
        }
        let raw_queue = queue.as_raw();
        let config = super::config(&caps);
        // SAFETY: The device's own fence.
        let fence = unsafe { device.create_fence() }.unwrap();
        call_log::take();

        // SAFETY: Each call meets wgpu-hal's requirements: every submission completes before the
        // surface is unconfigured, and before the idle wait. The caller's chain lives until the
        // present returns, and its fence until it has signaled.
        let callers_fence = unsafe {
            surface.configure(&device, &config).unwrap();
            super::present_frame(&device, &queue, &surface, &fence, 1);
            let acquired = surface
                .acquire_texture(Some(Duration::from_secs(1)), &fence)
                .unwrap();
            super::submit_for_present(&device, &queue, &acquired.texture, &fence, 2);
            let callers_fence = device
                .shared
                .raw
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .unwrap();
            let callers_fences = [callers_fence];
            let mut chain = vk::SwapchainPresentFenceInfoEXT::default().fences(&callers_fences);
            let chain: *mut vk::SwapchainPresentFenceInfoEXT<'_> = &mut chain;
            surface.set_next_present_chain(chain.cast());
            queue.present(&surface, acquired.texture).unwrap();
            call_log::mark("unconfigure");
            surface.unconfigure(&device);
            call_log::mark("unconfigured");
            device
                .shared
                .raw
                .wait_for_fences(
                    &callers_fences,
                    true,
                    Duration::from_secs(5).as_nanos() as u64,
                )
                .unwrap();
            device.shared.raw.destroy_fence(callers_fence, None);
            queue.wait_for_idle().unwrap();
            device.destroy_fence(fence);
            callers_fence
        };
        drop((queue, device));

        let calls = call_log::take().calls;
        let presents = presents(&calls);
        assert_eq!(presents.len(), 2, "{calls:#?}");
        assert_eq!(presents[0].fences.len(), 1, "{calls:#?}");
        assert_eq!(presents[1].fences, [callers_fence], "{calls:#?}");
        let teardowns = super::teardowns(&calls);
        let [teardown] = &teardowns[..] else {
            panic!("{calls:#?}");
        };
        let teardown = &calls[teardown.clone()];
        assert!(
            teardown
                .iter()
                .any(|call| call.kind == Kind::FenceWait
                    && call.fences.contains(&presents[0].fences[0])),
            "the teardown did not wait on the fence of the present before: {teardown:#?}"
        );
        check_waited_for_the_queue(teardown, raw_queue);
        super::check_destroyed(&calls);
    }
}

/// On each adapter, with an instance of wgpu-hal's desired extensions: the device enables
/// `VK_KHR_swapchain_maintenance1` where the driver lists it and the loader lists
/// `VK_KHR_surface_maintenance1`, else the EXT pair likewise, else neither, and its presents signal
/// fences exactly when it enables one. The expectation reads the loader's and the driver's own
/// lists by literal name, so a misspelt extension name fails it. It needs no surface, so it runs on
/// every driver, and prints what each device takes.
#[test]
fn each_device_takes_the_swapchain_maintenance_extension_its_instance_allows() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some(instance) = super::plain_instance() else {
        return;
    };
    // SAFETY: No requirements.
    let loader = unsafe {
        instance
            .shared
            .entry
            .enumerate_instance_extension_properties(None)
    }
    .unwrap();
    let loader_lists = |name: &CStr| {
        loader
            .iter()
            .any(|extension| extension.extension_name_as_c_str() == Ok(name))
    };
    // `desired_extensions` asks for the surface maintenance extensions with these two.
    let instance_allows = loader_lists(c"VK_KHR_get_physical_device_properties2")
        && loader_lists(c"VK_KHR_get_surface_capabilities2");
    // SAFETY: No requirements.
    for exposed in unsafe { instance.enumerate_adapters(None) } {
        // SAFETY: A physical device of the instance.
        let listed = unsafe {
            instance
                .shared
                .raw
                .enumerate_device_extension_properties(exposed.adapter.raw_physical_device())
        }
        .unwrap();
        let driver_lists = |name: &CStr| {
            listed
                .iter()
                .any(|extension| extension.extension_name_as_c_str() == Ok(name))
        };
        let expected = [
            (
                c"VK_KHR_swapchain_maintenance1",
                c"VK_KHR_surface_maintenance1",
            ),
            (
                c"VK_EXT_swapchain_maintenance1",
                c"VK_EXT_surface_maintenance1",
            ),
        ]
        .into_iter()
        .find(|&(device_extension, surface_extension)| {
            instance_allows && driver_lists(device_extension) && loader_lists(surface_extension)
        })
        .map(|(device_extension, _)| device_extension);
        let (device, queue) = super::open(&exposed.adapter);
        let enabled = device
            .enabled_device_extensions()
            .iter()
            .copied()
            .filter(|name| name.to_bytes().ends_with(b"_swapchain_maintenance1"))
            .collect::<Vec<_>>();
        std::println!(
            "{:?}: enabled {enabled:?}, expected {expected:?}; {}",
            exposed.info.name,
            super::teardown_path(&device)
        );
        assert_eq!(enabled, expected.into_iter().collect::<Vec<_>>());
        assert_eq!(device.presents_signal_fences(), expected.is_some());
        drop((queue, device));
    }
}
