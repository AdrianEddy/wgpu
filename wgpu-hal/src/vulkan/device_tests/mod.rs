//! Tests that open a Vulkan device, at wgpu-hal's level, with a headless surface
//! (`VK_EXT_headless_surface`, which needs no display). They skip at run time, printing why, where
//! there is no Vulkan or no headless surface, and run on CPU adapters too.
//!
//! A run that must not use a GPU leaves them all out by their name, which contains
//! `device_tests::`. They watch the Vulkan calls themselves, through [`call_log`].

mod injected;
mod present_fences;
mod queue_lock;
mod teardown;

use alloc::{vec, vec::Vec};
use core::{borrow::Borrow, iter, ops::Range, time::Duration};
use std::sync::Mutex;

use ash::{ext, vk};

use super::call_log;
use crate::{Adapter as _, CommandEncoder as _, Device as _, Instance as _, Queue as _};

/// One test at a time: the tests share [`call_log`]'s records.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// An instance with `flags`, whose device functions go through [`call_log`], and a headless surface
/// of it. `None`, having printed why, without Vulkan or `VK_EXT_headless_surface`.
fn headless_instance(flags: wgt::InstanceFlags) -> Option<(super::Instance, super::Surface)> {
    // SAFETY: Loading the Vulkan library has no requirements.
    let Ok(entry) = (unsafe { ash::Entry::load() }) else {
        std::println!("skipped: no Vulkan");
        return None;
    };
    // SAFETY: No requirements.
    let version = unsafe { entry.try_enumerate_instance_version() }
        .unwrap()
        .unwrap_or(vk::API_VERSION_1_0);
    // SAFETY: No requirements.
    let available = unsafe { entry.enumerate_instance_extension_properties(None) }.unwrap();
    if !available
        .iter()
        .any(|extension| extension.extension_name_as_c_str() == Ok(ext::headless_surface::NAME))
    {
        std::println!("skipped: no VK_EXT_headless_surface");
        return None;
    }
    let mut extensions = super::Instance::desired_extensions(&entry, version, flags).unwrap();
    extensions.push(ext::headless_surface::NAME);
    let app_info = vk::ApplicationInfo::default().api_version(if version < vk::API_VERSION_1_1 {
        vk::API_VERSION_1_0
    } else {
        vk::API_VERSION_1_3
    });
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&names);
    // SAFETY: A valid create info.
    let handle = unsafe { entry.create_instance(&info, None) }
        .unwrap()
        .handle();
    // SAFETY: Created from `entry` just now.
    let raw = unsafe { call_log::load_instance(&entry, handle) };
    let headless = ext::headless_surface::Instance::new(&entry, &raw);
    // SAFETY: Created from `entry` with wgpu-hal's desired extensions; wgpu-hal destroys it.
    let instance = unsafe {
        super::Instance::from_raw(
            entry.clone(),
            raw,
            version,
            0,
            None,
            extensions,
            flags,
            wgt::MemoryBudgetThresholds::default(),
            false,
            wgt::VulkanSwapchainKind::Native,
            None,
            None,
        )
    }
    .unwrap();
    // SAFETY: A valid create info; the surface destroys it.
    let vk_surface = unsafe {
        headless.create_headless_surface(&vk::HeadlessSurfaceCreateInfoEXT::default(), None)
    }
    .unwrap();
    let surface = instance.create_surface_from_vk_surface_khr(vk_surface, None);
    Some((instance, surface))
}

/// An instance with wgpu-hal's desired extensions and no surface, whose device functions are the
/// driver's own. `None`, having printed why, without Vulkan.
fn plain_instance() -> Option<super::Instance> {
    // SAFETY: Loading the Vulkan library has no requirements.
    let Ok(entry) = (unsafe { ash::Entry::load() }) else {
        std::println!("skipped: no Vulkan");
        return None;
    };
    let flags = wgt::InstanceFlags::empty();
    // SAFETY: No requirements.
    let version = unsafe { entry.try_enumerate_instance_version() }
        .unwrap()
        .unwrap_or(vk::API_VERSION_1_0);
    let extensions = super::Instance::desired_extensions(&entry, version, flags).unwrap();
    let app_info = vk::ApplicationInfo::default().api_version(if version < vk::API_VERSION_1_1 {
        vk::API_VERSION_1_0
    } else {
        vk::API_VERSION_1_3
    });
    let names = extensions
        .iter()
        .map(|name| name.as_ptr())
        .collect::<Vec<_>>();
    let info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&names);
    // SAFETY: A valid create info.
    let raw = unsafe { entry.create_instance(&info, None) }.unwrap();
    // SAFETY: Created from `entry` with wgpu-hal's desired extensions; wgpu-hal destroys it.
    let instance = unsafe {
        super::Instance::from_raw(
            entry,
            raw,
            version,
            0,
            None,
            extensions,
            flags,
            wgt::MemoryBudgetThresholds::default(),
            false,
            wgt::VulkanSwapchainKind::Native,
            None,
            None,
        )
    }
    .unwrap();
    Some(instance)
}

/// The instance flags each test runs with: without and with `DEBUG`, which loads the
/// `VK_EXT_debug_utils` device functions, so that their wrappers are installed.
const FLAGS: [wgt::InstanceFlags; 2] = [wgt::InstanceFlags::empty(), wgt::InstanceFlags::DEBUG];

/// The adapters of `instance` that support `surface`, with the surface's capabilities. Prints
/// each adapter, and why one is skipped.
fn adapters(
    instance: &super::Instance,
    surface: &super::Surface,
) -> Vec<(
    crate::ExposedAdapter<super::Api>,
    crate::SurfaceCapabilities,
)> {
    // SAFETY: No requirements.
    let adapters = unsafe { instance.enumerate_adapters(Some(surface)) };
    adapters
        .into_iter()
        .filter_map(|exposed| {
            std::println!("{:?}", exposed.info.name);
            // SAFETY: The surface is of this instance.
            match unsafe { crate::Adapter::surface_capabilities(&exposed.adapter, surface) } {
                Some(caps) => Some((exposed, caps)),
                None => {
                    std::println!(
                        "skipped {:?}: the surface is not supported",
                        exposed.info.name
                    );
                    None
                }
            }
        })
        .collect()
}

/// A configuration of the surface that `caps` describe.
fn config(caps: &crate::SurfaceCapabilities) -> crate::SurfaceConfiguration {
    let format = caps
        .formats
        .iter()
        .find(|format| format.color_spaces.contains(wgt::SurfaceColorSpaces::SRGB))
        .expect("a format in sRGB");
    crate::SurfaceConfiguration {
        maximum_frame_latency: 2.clamp(
            *caps.maximum_frame_latency.start(),
            *caps.maximum_frame_latency.end(),
        ),
        present_mode: caps.present_modes[0],
        composite_alpha_mode: caps.composite_alpha_modes[0],
        format: format.format,
        color_space: wgt::SurfaceColorSpace::Srgb,
        extent: wgt::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        usage: wgt::TextureUses::COLOR_TARGET,
        view_formats: vec![],
    }
}

/// Opens a device on `adapter` with `Adapter::open`.
fn open(adapter: &super::Adapter) -> (super::Device, super::Queue) {
    // SAFETY: No requirements beyond the adapter's own.
    let open = unsafe {
        adapter.open(
            wgt::Features::empty(),
            &wgt::Limits::downlevel_defaults(),
            &wgt::MemoryHints::default(),
        )
    }
    .unwrap();
    (open.device, open.queue)
}

/// Moves `texture` to the present state, submits that, setting `fence` to `value`, and waits for
/// it.
///
/// # Safety
///
/// `texture` was acquired from a surface configured for `device`.
unsafe fn submit_for_present(
    device: &super::Device,
    queue: &super::Queue,
    texture: &super::SurfaceTexture,
    fence: &super::Fence,
    value: crate::FenceValue,
) {
    // SAFETY: Our caller's contract; the encoder records only the transition.
    unsafe {
        let mut encoder = device
            .create_command_encoder(&crate::CommandEncoderDescriptor { label: None, queue })
            .unwrap();
        encoder.begin_encoding(None).unwrap();
        encoder.transition_textures(iter::once(crate::TextureBarrier {
            texture: texture.borrow(),
            range: wgt::ImageSubresourceRange::default(),
            usage: crate::StateTransition {
                from: wgt::TextureUses::UNINITIALIZED,
                to: wgt::TextureUses::PRESENT,
            },
            queue_family_ownership_transfer: None,
        }));
        let command_buffer = encoder.end_encoding().unwrap();
        queue
            .submit(&[&command_buffer], &[texture], (fence, value))
            .unwrap();
        assert!(device.wait(fence, value, None).unwrap());
        encoder.reset_all(iter::once(command_buffer));
    }
}

/// Acquires a frame, moves it to the present state, submits that, setting `fence` to `value`,
/// waits for it, and presents the frame.
///
/// # Safety
///
/// `surface` is configured for `device`.
unsafe fn present_frame(
    device: &super::Device,
    queue: &super::Queue,
    surface: &super::Surface,
    fence: &super::Fence,
    value: crate::FenceValue,
) {
    // SAFETY: Our caller's contract.
    unsafe {
        let acquired =
            crate::Surface::acquire_texture(surface, Some(Duration::from_secs(1)), fence).unwrap();
        submit_for_present(device, queue, &acquired.texture, fence, value);
        queue.present(surface, acquired.texture, &[]).unwrap();
    }
}

/// Checks the destruction of the device in `calls`: `vkDestroyDevice` is the last device call,
/// and no queue command was in progress then.
fn check_destroyed(calls: &[call_log::Call]) {
    let last = calls
        .iter()
        .rposition(|call| call.kind != call_log::Kind::Mark)
        .unwrap();
    assert_eq!(
        calls[last].kind,
        call_log::Kind::DestroyDevice,
        "the device was not destroyed last: {calls:#?}"
    );
    assert_eq!(calls[last].in_flight, 0, "{calls:#?}");
}

/// The span of each teardown in `calls`: from a mark `unconfigure` to the next mark
/// `unconfigured`.
fn teardowns(calls: &[call_log::Call]) -> Vec<Range<usize>> {
    calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.name == "unconfigure")
        .map(|(start, _)| {
            let end = start
                + calls[start..]
                    .iter()
                    .position(|call| call.name == "unconfigured")
                    .unwrap();
            start..end
        })
        .collect()
}

/// How a device's swapchains wait for their presents in their teardown, for the test's output.
fn teardown_path(device: &super::Device) -> &'static str {
    if device.presents_signal_fences() {
        "the teardown waits on present fences"
    } else {
        "no present fences: the teardown waits for the queue"
    }
}
