//! Which swapchain maintenance extension a device takes, given what its physical device lists and
//! what its instance enabled, and which versions of Arm's proprietary driver can leak memory on
//! timeline semaphore signals.

use core::ffi::CStr;

use ash::{ext, khr, vk};

use super::{PhysicalDeviceProperties, KHR_SURFACE_MAINTENANCE1, KHR_SWAPCHAIN_MAINTENANCE1};

const EXT_SWAPCHAIN_MAINTENANCE1: &CStr = ext::swapchain_maintenance1::NAME;
const EXT_SURFACE_MAINTENANCE1: &CStr = ext::surface_maintenance1::NAME;

/// The extensions of a physical device that lists both swapchain maintenance extensions.
const LISTS_BOTH: [&CStr; 3] = [
    khr::swapchain::NAME,
    KHR_SWAPCHAIN_MAINTENANCE1,
    EXT_SWAPCHAIN_MAINTENANCE1,
];

/// A physical device that lists `extensions`, as `InstanceShared::inspect` keeps them for an
/// instance with `instance_extensions` and, with `gpdp2`, the functions of
/// `VK_KHR_get_physical_device_properties2`.
fn device(
    extensions: &[&CStr],
    instance_extensions: &[&'static CStr],
    gpdp2: bool,
) -> PhysicalDeviceProperties {
    let mut properties = PhysicalDeviceProperties {
        supported_extensions: extensions
            .iter()
            .map(|&name| {
                vk::ExtensionProperties::default()
                    .extension_name(name)
                    .unwrap()
            })
            .collect(),
        ..Default::default()
    };
    properties.drop_extensions_unusable_with(instance_extensions, gpdp2);
    properties
}

/// Where the instance has both surface maintenance extensions, a device that lists both swapchain
/// ones takes the KHR one.
#[test]
fn a_device_takes_the_khr_extension_where_its_instance_has_both() {
    let device = device(
        &LISTS_BOTH,
        &[KHR_SURFACE_MAINTENANCE1, EXT_SURFACE_MAINTENANCE1],
        true,
    );
    assert_eq!(
        device.swapchain_maintenance1(),
        Some(KHR_SWAPCHAIN_MAINTENANCE1)
    );
}

/// Where the instance has only the EXT surface extension, the KHR swapchain extension is
/// unsupported, and the device takes the EXT one.
#[test]
fn a_device_takes_the_ext_extension_where_its_instance_has_only_the_ext_surface_one() {
    let device = device(&LISTS_BOTH, &[EXT_SURFACE_MAINTENANCE1], true);
    assert!(!device.supports_extension(KHR_SWAPCHAIN_MAINTENANCE1));
    assert_eq!(
        device.swapchain_maintenance1(),
        Some(EXT_SWAPCHAIN_MAINTENANCE1)
    );
}

/// Where the instance has only the KHR surface extension, a device that lists only the EXT
/// swapchain extension takes none: that one is unsupported there.
#[test]
fn a_device_that_lists_only_the_ext_extension_takes_none_where_its_instance_has_only_the_khr_surface_one(
) {
    let device = device(
        &[khr::swapchain::NAME, EXT_SWAPCHAIN_MAINTENANCE1],
        &[KHR_SURFACE_MAINTENANCE1],
        true,
    );
    assert!(!device.supports_extension(EXT_SWAPCHAIN_MAINTENANCE1));
    assert_eq!(device.swapchain_maintenance1(), None);
}

/// Without the surface maintenance extensions, or without the functions of
/// `VK_KHR_get_physical_device_properties2`, a device takes neither, and keeps its other
/// extensions.
#[test]
fn a_device_takes_neither_without_its_instance_extensions_or_gpdp2() {
    let all: &[&'static CStr] = &[KHR_SURFACE_MAINTENANCE1, EXT_SURFACE_MAINTENANCE1];
    for (instance_extensions, gpdp2) in [(&[][..], true), (all, false)] {
        let device = device(&LISTS_BOTH, instance_extensions, gpdp2);
        assert_eq!(device.swapchain_maintenance1(), None);
        assert!(device.supports_extension(khr::swapchain::NAME));
    }
}

#[test]
fn arm_timeline_semaphore_driver_versions() {
    use super::arm_driver_leaks_timeline_semaphores as leaks;

    assert!(leaks("v1.g18p0-01eac0.9e534f339477793d151cc7ab4c27f2c0"));
    assert!(!leaks("v1.g24p0-00eac0.3c7aec95bbe4cb46ae6709632084ff89"));
    assert!(leaks(
        "v1.r44p0-wayland-drm-g310-dmaheap-aarch64-d877e132c-18"
    ));
    assert!(!leaks("v1.r46p0-01eac0"));
    assert!(leaks("?"));
    assert!(leaks("v1.x50p0"));
    assert!(leaks("v1.r50"));
}
