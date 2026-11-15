//! Which swapchain maintenance extension a device takes, given what its physical device lists and
//! what its instance enabled, which optional extensions a device enables, how a device's
//! capabilities follow the features it was created with, and which versions of Arm's proprietary
//! driver can leak memory on timeline semaphore signals.

use core::ffi::CStr;

use ash::{ext, khr, vk};

use super::{
    enables_swapchain_maintenance1, missing_downlevel_features, PhysicalDeviceFeatures,
    PhysicalDeviceProperties, KHR_SURFACE_MAINTENANCE1, KHR_SWAPCHAIN_MAINTENANCE1,
};
use crate::vulkan::PrivateCapabilities;

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

/// A physical device of Vulkan `version` that lists `extensions`.
fn device_of_version(version: u32, extensions: &[&CStr]) -> PhysicalDeviceProperties {
    PhysicalDeviceProperties {
        supported_extensions: extensions
            .iter()
            .map(|&name| {
                vk::ExtensionProperties::default()
                    .extension_name(name)
                    .unwrap()
            })
            .collect(),
        device_api_version: version,
        ..Default::default()
    }
}

/// A Vulkan 1.2 device that lists `VK_KHR_zero_initialize_workgroup_memory` enables it, so that the
/// feature, which the adapter then offers and Naga then uses, is enabled; a Vulkan 1.3 device has
/// the feature in core and does not.
#[test]
fn a_pre_1_3_device_enables_zero_initialize_workgroup_memory_where_listed() {
    let extension = khr::zero_initialize_workgroup_memory::NAME;
    for (version, listed, enabled) in [
        (vk::API_VERSION_1_2, true, true),
        (vk::API_VERSION_1_2, false, false),
        (vk::API_VERSION_1_3, true, false),
    ] {
        let extensions: &[&CStr] = if listed { &[extension] } else { &[] };
        let device = device_of_version(version, extensions);
        assert_eq!(
            device
                .get_required_extensions(wgt::Features::empty())
                .contains(&extension),
            enabled,
            "Vulkan {}.{}, listed: {listed}",
            vk::api_version_major(version),
            vk::api_version_minor(version),
        );
    }
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

/// The adapter's workaround that disables timeline semaphores holds for a device created with
/// `timelineSemaphore` as well.
#[test]
fn the_timeline_semaphore_workaround_holds_for_a_device_created_with_the_feature() {
    let mut timeline =
        vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(true);
    // SAFETY: A valid feature structure with a null `p_next`.
    let features = unsafe {
        enabled(
            vk::PhysicalDeviceFeatures::default(),
            &mut [base(&mut timeline)],
        )
    };
    let device = device_of_version(vk::API_VERSION_1_2, &[]);
    let with = |workarounds| {
        let mut capabilities = PrivateCapabilities::default();
        capabilities.set_extension_and_feature_capabilities(
            &device,
            |_| false,
            &features,
            workarounds,
        );
        capabilities.timeline_semaphores
    };
    assert!(with(super::super::Workarounds::empty()));
    assert!(!with(
        super::super::Workarounds::DISABLE_TIMELINE_SEMAPHORES
    ));
}

/// The downlevel flags whose core features wgpu-core relies on a device having.
const DOWNLEVEL_FLAGS: wgt::DownlevelFlags = wgt::DownlevelFlags::INDEPENDENT_BLEND
    .union(wgt::DownlevelFlags::MULTISAMPLED_SHADING)
    .union(wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES)
    .union(wgt::DownlevelFlags::ANISOTROPIC_FILTERING)
    .union(wgt::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE)
    .union(wgt::DownlevelFlags::DEPTH_BIAS_CLAMP);

/// The capabilities of a device of `version` with `extensions` and `features`, whose physical
/// device reports `VK_EXT_robustness2`'s properties.
fn capabilities(
    version: u32,
    extensions: &[&CStr],
    features: &PhysicalDeviceFeatures,
) -> PrivateCapabilities {
    let mut capabilities = PrivateCapabilities::default();
    capabilities.set_extension_and_feature_capabilities(
        &PhysicalDeviceProperties {
            robustness2: Some(Default::default()),
            ..device_of_version(version, &[])
        },
        |extension| extensions.contains(&extension),
        features,
        super::super::Workarounds::empty(),
    );
    capabilities
}

/// The features of a device created with the core features `core` and the chain of `structures`.
///
/// # Safety
///
/// Each of `structures` is a valid feature structure with a null `p_next`.
unsafe fn enabled(
    core: vk::PhysicalDeviceFeatures,
    structures: &mut [&mut vk::BaseOutStructure<'_>],
) -> PhysicalDeviceFeatures {
    // Link the structures in order.
    for index in (1..structures.len()).rev() {
        let next: *mut vk::BaseOutStructure<'_> = &mut *structures[index];
        structures[index - 1].p_next = next.cast();
    }
    let mut features2 = vk::PhysicalDeviceFeatures2::default().features(core);
    if let Some(first) = structures.first_mut() {
        let first: *mut vk::BaseOutStructure<'_> = &mut **first;
        features2.p_next = first.cast();
    }
    // SAFETY: The structures form a valid chain, as our caller guarantees.
    unsafe { PhysicalDeviceFeatures::from_enabled(&features2) }
}

/// A feature structure, as the first member of a chain.
fn base<T: vk::TaggedStructure>(structure: &mut T) -> &mut vk::BaseOutStructure<'static> {
    let structure: *mut T = structure;
    // SAFETY: Every Vulkan structure starts with `s_type` and `p_next`.
    unsafe { &mut *structure.cast() }
}

/// `from_enabled` reads the core features, the Vulkan 1.1, 1.2 and 1.3 structures and the
/// extensions' own structures of a creator's chain, skips a structure it does not track, and keeps
/// no pointer of the chain.
#[test]
fn enabled_features_are_read_from_the_vulkan_1_x_and_extension_structures() {
    let mut vulkan11 = vk::PhysicalDeviceVulkan11Features::default()
        .storage_buffer16_bit_access(true)
        .storage_input_output16(true)
        .multiview(true);
    let mut synchronization2 =
        vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true);
    let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default()
        .timeline_semaphore(true)
        .shader_int8(true)
        .descriptor_binding_partially_bound(true);
    let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default()
        .robust_image_access(true)
        .shader_zero_initialize_workgroup_memory(true)
        .shader_integer_dot_product(true);
    let mut robustness2 =
        vk::PhysicalDeviceRobustness2FeaturesEXT::default().robust_buffer_access2(true);
    let mut swapchain =
        vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default().swapchain_maintenance1(true);
    let core = vk::PhysicalDeviceFeatures::default()
        .image_cube_array(true)
        .multi_draw_indirect(true);
    // SAFETY: Valid feature structures, each with a null `p_next`.
    let features = unsafe {
        enabled(
            core,
            &mut [
                base(&mut vulkan11),
                base(&mut synchronization2),
                base(&mut vulkan12),
                base(&mut vulkan13),
                base(&mut robustness2),
                base(&mut swapchain),
            ],
        )
    };

    assert_eq!(features.core.image_cube_array, vk::TRUE);
    assert_eq!(features.core.multi_draw_indirect, vk::TRUE);
    assert_eq!(features.core.robust_buffer_access, vk::FALSE);
    let storage16 = features._16bit_storage.unwrap();
    assert_eq!(storage16.storage_buffer16_bit_access, vk::TRUE);
    assert!(features.supports_storage_input_output_16());
    assert_eq!(features.multiview.unwrap().multiview, vk::TRUE);
    assert_eq!(
        features.timeline_semaphore.unwrap().timeline_semaphore,
        vk::TRUE
    );
    let float16_int8 = features.shader_float16_int8.unwrap();
    assert_eq!(float16_int8.shader_int8, vk::TRUE);
    assert_eq!(float16_int8.shader_float16, vk::FALSE);
    let indexing = features.descriptor_indexing.unwrap();
    assert_eq!(indexing.descriptor_binding_partially_bound, vk::TRUE);
    assert_eq!(
        indexing.shader_sampled_image_array_non_uniform_indexing,
        vk::FALSE
    );
    assert_eq!(
        features.image_robustness.unwrap().robust_image_access,
        vk::TRUE
    );
    assert_eq!(
        features
            .zero_initialize_workgroup_memory
            .unwrap()
            .shader_zero_initialize_workgroup_memory,
        vk::TRUE
    );
    assert_eq!(
        features
            .shader_integer_dot_product
            .unwrap()
            .shader_integer_dot_product,
        vk::TRUE
    );
    let robustness2 = features.robustness2.unwrap();
    assert_eq!(robustness2.robust_buffer_access2, vk::TRUE);
    assert_eq!(robustness2.robust_image_access2, vk::FALSE);
    let swapchain = features.swapchain_maintenance1.unwrap();
    assert_eq!(swapchain.swapchain_maintenance1, vk::TRUE);
    assert!(robustness2.p_next.is_null() && swapchain.p_next.is_null());
    assert!(features.acceleration_structure.is_none());
}

/// A feature that both a Vulkan 1.x structure and the extension's own structure give is enabled
/// where either enables it.
#[test]
fn a_feature_given_twice_is_enabled_by_either() {
    let mut timeline = vk::PhysicalDeviceTimelineSemaphoreFeatures::default();
    let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
    // SAFETY: Valid feature structures, each with a null `p_next`.
    let features = unsafe {
        enabled(
            vk::PhysicalDeviceFeatures::default(),
            &mut [base(&mut timeline), base(&mut vulkan12)],
        )
    };
    assert_eq!(
        features.timeline_semaphore.unwrap().timeline_semaphore,
        vk::TRUE
    );
}

/// A `VkPhysicalDeviceFeatures2` inside a creator's chain gives its core features: `ash`'s
/// `push_next` prepends, so the creator's own `VkPhysicalDeviceFeatures2` can follow its other
/// structures. `from_device_create_info` reads a `VkDeviceCreateInfo` whatever the order of its
/// chain, or with `pEnabledFeatures` instead.
#[test]
fn a_create_info_is_read_whatever_the_order_of_its_chain() {
    let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default().timeline_semaphore(true);
    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .features(vk::PhysicalDeviceFeatures::default().image_cube_array(true));
    let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default().robust_image_access(true);
    // `vulkan13`, `vulkan12`, `features2`: the creator's own structure is last.
    let info = vk::DeviceCreateInfo::default()
        .push_next(&mut features2)
        .push_next(&mut vulkan12)
        .push_next(&mut vulkan13);
    // SAFETY: A create info with a valid chain and no `pEnabledFeatures`.
    let features = unsafe { PhysicalDeviceFeatures::from_device_create_info(&info) };
    assert_eq!(features.core.image_cube_array, vk::TRUE);
    assert_eq!(features.core.sampler_anisotropy, vk::FALSE);
    assert_eq!(
        features.timeline_semaphore.unwrap().timeline_semaphore,
        vk::TRUE
    );
    assert_eq!(
        features.image_robustness.unwrap().robust_image_access,
        vk::TRUE
    );

    let core = vk::PhysicalDeviceFeatures::default().sampler_anisotropy(true);
    let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default().robust_image_access(true);
    let info = vk::DeviceCreateInfo::default()
        .enabled_features(&core)
        .push_next(&mut vulkan13);
    // SAFETY: A create info with a valid `pEnabledFeatures` and chain.
    let features = unsafe { PhysicalDeviceFeatures::from_device_create_info(&info) };
    assert_eq!(features.core.sampler_anisotropy, vk::TRUE);
    assert_eq!(
        features.image_robustness.unwrap().robust_image_access,
        vk::TRUE
    );
}

/// The same structure given twice, which VUID-VkDeviceCreateInfo-sType-unique forbids, enables
/// what either copy enables, in either order, as a Vulkan 1.x structure and an extension's own do.
#[test]
fn a_structure_given_twice_enables_what_either_enables() {
    for on_first in [true, false] {
        let mut first =
            vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(on_first);
        let mut second =
            vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(!on_first);
        let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default().shader_int8(on_first);
        let mut vulkan12_again =
            vk::PhysicalDeviceVulkan12Features::default().shader_int8(!on_first);
        // SAFETY: Valid feature structures, each with a null `p_next`.
        let features = unsafe {
            enabled(
                vk::PhysicalDeviceFeatures::default(),
                &mut [
                    base(&mut first),
                    base(&mut vulkan12),
                    base(&mut second),
                    base(&mut vulkan12_again),
                ],
            )
        };
        assert_eq!(
            features.timeline_semaphore.unwrap().timeline_semaphore,
            vk::TRUE,
            "enabled first: {on_first}"
        );
        assert_eq!(
            features.shader_float16_int8.unwrap().shader_int8,
            vk::TRUE,
            "enabled first: {on_first}"
        );
    }
}

/// A device's capabilities follow the features it was created with: with none, it takes the paths
/// that need none; with those of its Vulkan 1.x structures, it takes the native ones.
#[test]
fn capabilities_follow_the_features_a_device_was_created_with() {
    let none = capabilities(vk::API_VERSION_1_3, &[], &PhysicalDeviceFeatures::default());
    assert!(!none.timeline_semaphores);
    assert!(!none.multi_draw_indirect);
    assert!(!none.robust_buffer_access);
    assert!(!none.robust_image_access);
    assert!(!none.robust_buffer_access2);
    assert!(!none.robust_image_access2);
    assert!(!none.zero_initialize_workgroup_memory);
    assert!(!none.shader_integer_dot_product);
    assert!(!none.shader_int8);

    let mut vulkan12 = vk::PhysicalDeviceVulkan12Features::default()
        .timeline_semaphore(true)
        .shader_int8(true);
    let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default()
        .robust_image_access(true)
        .shader_zero_initialize_workgroup_memory(true)
        .shader_integer_dot_product(true);
    let mut robustness2 = vk::PhysicalDeviceRobustness2FeaturesEXT::default()
        .robust_buffer_access2(true)
        .robust_image_access2(true);
    let core = vk::PhysicalDeviceFeatures::default()
        .robust_buffer_access(true)
        .multi_draw_indirect(true);
    // SAFETY: Valid feature structures, each with a null `p_next`.
    let features = unsafe {
        enabled(
            core,
            &mut [
                base(&mut vulkan12),
                base(&mut vulkan13),
                base(&mut robustness2),
            ],
        )
    };
    let all = capabilities(vk::API_VERSION_1_3, &[], &features);
    assert!(all.timeline_semaphores);
    assert!(all.multi_draw_indirect);
    assert!(all.robust_buffer_access);
    assert!(all.robust_image_access);
    assert!(all.robust_buffer_access2);
    assert!(all.robust_image_access2);
    assert!(all.zero_initialize_workgroup_memory);
    assert!(all.shader_integer_dot_product);
    assert!(all.shader_int8);
}

/// A device created with `VK_EXT_robustness2`'s features relies on them only where its physical
/// device reports the extension's properties: the workaround for outdated Intel drivers clears
/// them, and the uniform bounds-check alignment that wgpu-core pads lazy initialization to comes
/// from them. Without them, the device keeps Naga's bounds checks.
#[test]
fn robustness2_is_relied_on_only_where_the_physical_device_reports_it() {
    let mut robustness2 = vk::PhysicalDeviceRobustness2FeaturesEXT::default()
        .robust_buffer_access2(true)
        .robust_image_access2(true);
    // SAFETY: A valid feature structure with a null `p_next`.
    let features = unsafe {
        enabled(
            vk::PhysicalDeviceFeatures::default(),
            &mut [base(&mut robustness2)],
        )
    };
    let extensions = [ext::robustness2::NAME];
    let with = capabilities(vk::API_VERSION_1_3, &extensions, &features);
    assert!(with.robust_buffer_access2);
    assert!(with.robust_image_access2);
    assert!(with.robust_image_access);

    let mut without = PrivateCapabilities::default();
    without.set_extension_and_feature_capabilities(
        &device_of_version(vk::API_VERSION_1_3, &[]),
        |extension| extensions.contains(&extension),
        &features,
        super::super::Workarounds::empty(),
    );
    assert!(!without.robust_buffer_access2);
    assert!(!without.robust_image_access2);
    assert!(!without.robust_image_access);
}

/// A device's timeline semaphores need their functions as well as their feature: Vulkan 1.2 or
/// `VK_KHR_timeline_semaphore`. A Vulkan 1.1 device that gives the feature structure without the
/// extension, against the structure pointer chain rule, takes binary fences.
#[test]
fn timeline_semaphores_need_vulkan_1_2_or_the_extension() {
    let mut timeline =
        vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(true);
    // SAFETY: A valid feature structure with a null `p_next`.
    let features = unsafe {
        enabled(
            vk::PhysicalDeviceFeatures::default(),
            &mut [base(&mut timeline)],
        )
    };
    let extension = [khr::timeline_semaphore::NAME];
    assert!(!capabilities(vk::API_VERSION_1_1, &[], &features).timeline_semaphores);
    assert!(capabilities(vk::API_VERSION_1_1, &extension, &features).timeline_semaphores);
    assert!(capabilities(vk::API_VERSION_1_2, &[], &features).timeline_semaphores);
}

/// The capabilities that extensions give follow the extensions a device was created with: a
/// Vulkan 1.0 device has them only with the extensions; a Vulkan 1.3 device has them in core.
#[test]
fn capabilities_follow_the_extensions_a_device_was_created_with() {
    let features = PhysicalDeviceFeatures::default();
    let without = capabilities(vk::API_VERSION_1_0, &[], &features);
    assert!(!without.image_view_usage);
    assert!(!without.image_format_list);
    assert!(!without.store_op_none);

    let with = capabilities(
        vk::API_VERSION_1_0,
        &[
            khr::maintenance2::NAME,
            khr::image_format_list::NAME,
            khr::load_store_op_none::NAME,
        ],
        &features,
    );
    assert!(with.image_view_usage);
    assert!(with.image_format_list);
    assert!(with.store_op_none);

    let core = capabilities(vk::API_VERSION_1_3, &[], &features);
    assert!(core.image_view_usage);
    assert!(core.image_format_list);
    assert!(core.store_op_none);
}

/// Presents signal fences only on a device with a swapchain maintenance extension and its
/// feature: not with the extension alone.
#[test]
fn present_fences_need_the_swapchain_maintenance_feature_as_well_as_its_extension() {
    let mut off = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
    let mut on =
        vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default().swapchain_maintenance1(true);
    let core = vk::PhysicalDeviceFeatures::default();
    // SAFETY: Valid feature structures, each with a null `p_next`.
    let (absent, off, on) = unsafe {
        (
            enabled(core, &mut []),
            enabled(core, &mut [base(&mut off)]),
            enabled(core, &mut [base(&mut on)]),
        )
    };
    let extension = [KHR_SWAPCHAIN_MAINTENANCE1];
    assert!(!enables_swapchain_maintenance1(&absent, &extension));
    assert!(!enables_swapchain_maintenance1(&off, &extension));
    assert!(enables_swapchain_maintenance1(&on, &extension));
    assert!(!enables_swapchain_maintenance1(&on, &[]));
}

/// A device lacks the core features behind the adapter's downlevel flags that it was not created
/// with, by name; the features that wgpu-hal enables on the devices it opens lack none.
#[test]
fn a_device_lacks_the_downlevel_features_it_was_not_created_with() {
    let core = vk::PhysicalDeviceFeatures::default()
        .image_cube_array(true)
        .sampler_anisotropy(true);
    assert_eq!(
        missing_downlevel_features(DOWNLEVEL_FLAGS, &core),
        [
            "independentBlend",
            "sampleRateShading",
            "fragmentStoresAndAtomics",
            "depthBiasClamp"
        ]
    );
    assert!(missing_downlevel_features(wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES, &core).is_empty());

    let opened = PhysicalDeviceFeatures::from_extensions_and_requested_features(
        &device_of_version(vk::API_VERSION_1_3, &[]),
        &PhysicalDeviceFeatures::default(),
        &[],
        wgt::Features::empty(),
        DOWNLEVEL_FLAGS,
        &PrivateCapabilities::default(),
    );
    assert!(missing_downlevel_features(DOWNLEVEL_FLAGS, &opened.core).is_empty());
}
