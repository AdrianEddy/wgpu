//! Which swapchain maintenance extension a device takes, given what its physical device lists and
//! what its instance enabled, which optional extensions a device enables, how a device's
//! capabilities and downlevel flags follow the features and extensions it was created with, and
//! which wgpu features those give.

use core::ffi::CStr;

use ash::{ext, khr, vk};

use super::{
    device_downlevel_capabilities, enables_swapchain_maintenance1, uses_device_addresses,
    FormatSupport, PhysicalDeviceFeatures, PhysicalDeviceProperties, DOWNLEVEL_CORE_FEATURES,
    KHR_SURFACE_MAINTENANCE1, KHR_SWAPCHAIN_MAINTENANCE1,
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

/// A Vulkan 1.0 physical device that lists the extensions behind downlevel flags and offers every
/// core feature behind them and `fullDrawIndexUint32`, the features it offers, and its adapter's
/// downlevel capabilities.
fn downlevel_adapter() -> (
    PhysicalDeviceProperties,
    PhysicalDeviceFeatures,
    wgt::DownlevelCapabilities,
) {
    let caps = device_of_version(
        vk::API_VERSION_1_0,
        &[
            khr::swapchain::NAME,
            khr::swapchain_mutable_format::NAME,
            khr::maintenance1::NAME,
            khr::maintenance2::NAME,
            khr::storage_buffer_storage_class::NAME,
        ],
    );
    let mut core = vk::PhysicalDeviceFeatures::default();
    for (_, feature) in DOWNLEVEL_CORE_FEATURES {
        *feature(&mut core) = vk::TRUE;
    }
    let offered = PhysicalDeviceFeatures {
        core,
        ..PhysicalDeviceFeatures::default()
    };
    let (_, flags) = offered.to_wgpu(
        &caps,
        |extension| caps.supports_extension(extension),
        &FormatSupport::default(),
        &vk::QueueFamilyProperties::default(),
    );
    let downlevel = wgt::DownlevelCapabilities {
        flags,
        ..wgt::DownlevelCapabilities::default()
    };
    (caps, offered, downlevel)
}

/// A device created without one of the core features behind downlevel flags has its adapter's
/// downlevel capabilities without exactly that feature's flag, `FULL_DRAW_INDEX_UINT32` included.
#[test]
fn a_device_lacks_exactly_the_downlevel_flags_of_the_features_it_lacks() {
    let (caps, offered, adapter) = downlevel_adapter();
    let extensions = [
        khr::swapchain::NAME,
        khr::swapchain_mutable_format::NAME,
        khr::maintenance2::NAME,
    ];
    for (flag, feature) in DOWNLEVEL_CORE_FEATURES {
        let mut core = offered.core;
        *feature(&mut core) = vk::FALSE;
        let enabled = PhysicalDeviceFeatures {
            core,
            ..PhysicalDeviceFeatures::default()
        };
        let device = device_downlevel_capabilities(
            &adapter,
            &caps,
            |extension| extensions.contains(&extension),
            &enabled,
        );
        assert_eq!(device.flags, adapter.flags - flag, "without {flag:?}");
    }
}

/// `fullDrawIndexUint32` must be enabled to use index values above 2^24 - 1: wgpu-hal's own
/// devices enable it where the adapter offers it, and keep `FULL_DRAW_INDEX_UINT32`; a device
/// created without it lacks the flag.
#[test]
fn full_draw_index_uint32_is_enabled_and_per_device() {
    let (caps, offered, adapter) = downlevel_adapter();
    assert!(adapter
        .flags
        .contains(wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32));
    let enabled = PhysicalDeviceFeatures::from_extensions_and_requested_features(
        &caps,
        &offered,
        &[],
        wgt::Features::empty(),
        adapter.flags,
        &PrivateCapabilities::default(),
    );
    assert_eq!(enabled.core.full_draw_index_uint32, vk::TRUE);
    let mut without = PhysicalDeviceFeatures {
        core: offered.core,
        ..PhysicalDeviceFeatures::default()
    };
    without.core.full_draw_index_uint32 = vk::FALSE;
    let device = device_downlevel_capabilities(&adapter, &caps, |_| true, &without);
    assert!(!device
        .flags
        .contains(wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32));
}

/// A device has no downlevel flag that its adapter lacks: one created without `VK_KHR_swapchain`
/// on an adapter without `VK_KHR_swapchain_mutable_format` does not gain `SURFACE_VIEW_FORMATS`,
/// and one created with a core feature that its adapter does not offer does not gain its flag.
#[test]
fn a_device_has_no_downlevel_flag_that_its_adapter_lacks() {
    let (caps, offered, mut adapter) = downlevel_adapter();
    adapter.flags -= wgt::DownlevelFlags::SURFACE_VIEW_FORMATS
        | wgt::DownlevelFlags::INDEPENDENT_BLEND
        | wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32;
    let device = device_downlevel_capabilities(&adapter, &caps, |_| false, &offered);
    assert_eq!(
        device.flags - adapter.flags,
        wgt::DownlevelFlags::empty(),
        "{:?}",
        device.flags
    );
}

/// The downlevel flags that extensions give follow the extensions a device was created with:
/// `READ_ONLY_DEPTH_STENCIL` needs `VK_KHR_maintenance2` before Vulkan 1.1, and
/// `SURFACE_VIEW_FORMATS` needs `VK_KHR_swapchain_mutable_format` on a device with swapchains.
#[test]
fn a_devices_extension_flags_follow_its_extensions() {
    let (caps, offered, adapter) = downlevel_adapter();
    let device = device_downlevel_capabilities(
        &adapter,
        &caps,
        |extension| extension == khr::swapchain::NAME,
        &offered,
    );
    assert_eq!(
        adapter.flags - device.flags,
        wgt::DownlevelFlags::READ_ONLY_DEPTH_STENCIL | wgt::DownlevelFlags::SURFACE_VIEW_FORMATS
    );
}

/// A device that wgpu-hal opens has its adapter's downlevel capabilities, whatever it is opened
/// with: it enables every core feature behind a flag that the adapter has.
#[test]
fn a_device_that_wgpu_hal_opens_has_its_adapters_downlevel_capabilities() {
    let (caps, offered, adapter) = downlevel_adapter();
    for requested in [wgt::Features::empty(), wgt::Features::all()] {
        let extensions = caps
            .get_required_extensions(requested)
            .into_iter()
            .filter(|&extension| caps.supports_extension(extension))
            .collect::<alloc::vec::Vec<_>>();
        let enabled = PhysicalDeviceFeatures::from_extensions_and_requested_features(
            &caps,
            &offered,
            &extensions,
            requested,
            adapter.flags,
            &PrivateCapabilities::default(),
        );
        let device = device_downlevel_capabilities(
            &adapter,
            &caps,
            |extension| extensions.contains(&extension),
            &enabled,
        );
        assert_eq!(device, adapter, "{requested:?}");
    }
}

/// The wgpu features of a device of the physical device `caps`, which supports no format that a
/// feature depends on, with the extensions `extensions` and the features `features`, and a queue
/// without timestamps.
fn derived(
    caps: &PhysicalDeviceProperties,
    extensions: &[&CStr],
    features: &PhysicalDeviceFeatures,
) -> wgt::Features {
    features
        .to_wgpu(
            caps,
            |extension| extensions.contains(&extension),
            &FormatSupport::default(),
            &vk::QueueFamilyProperties::default(),
        )
        .0
}

/// A Vulkan 1.3 physical device with every subgroup operation in every stage, and one cooperative
/// matrix configuration.
fn subgroups_and_matrices() -> PhysicalDeviceProperties {
    PhysicalDeviceProperties {
        subgroup: Some(
            vk::PhysicalDeviceSubgroupProperties::default()
                .supported_operations(vk::SubgroupFeatureFlags::from_raw(!0))
                .supported_stages(vk::ShaderStageFlags::ALL),
        ),
        cooperative_matrix_properties: alloc::vec![wgt::CooperativeMatrixProperties {
            m_size: 16,
            n_size: 16,
            k_size: 16,
            ab_type: wgt::CooperativeScalarType::F16,
            cr_type: wgt::CooperativeScalarType::F32,
            saturating_accumulation: false,
        }],
        ..device_of_version(vk::API_VERSION_1_3, &[])
    }
}

/// Each wgpu feature, the extensions that give it, and the features of a device that has them with
/// every member that the feature needs off (`.0`) and on (`.1`).
fn member_cases() -> alloc::vec::Vec<(
    wgt::Features,
    &'static [&'static CStr],
    PhysicalDeviceFeatures,
    PhysicalDeviceFeatures,
)> {
    let acceleration_structures: &[&CStr] = &[
        khr::deferred_host_operations::NAME,
        khr::acceleration_structure::NAME,
        khr::buffer_device_address::NAME,
        khr::ray_query::NAME,
        khr::ray_tracing_pipeline::NAME,
    ];
    let ray_tracing = |on: bool| PhysicalDeviceFeatures {
        acceleration_structure: Some(
            vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                .acceleration_structure(on),
        ),
        buffer_device_address: Some(
            vk::PhysicalDeviceBufferDeviceAddressFeaturesKHR::default().buffer_device_address(on),
        ),
        ray_query: Some(vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(on)),
        ray_tracing_pipeline: Some(
            vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default().ray_tracing_pipeline(on),
        ),
        ..PhysicalDeviceFeatures::default()
    };
    alloc::vec![
        (
            wgt::Features::TEXTURE_INT64_ATOMIC,
            &[ext::shader_image_atomic_int64::NAME][..],
            PhysicalDeviceFeatures {
                shader_image_atomic_int64: Some(
                    vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default(),
                ),
                ..PhysicalDeviceFeatures::default()
            },
            PhysicalDeviceFeatures {
                shader_image_atomic_int64: Some(
                    vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default()
                        .shader_image_int64_atomics(true),
                ),
                ..PhysicalDeviceFeatures::default()
            },
        ),
        (
            wgt::Features::EXPERIMENTAL_RAY_QUERY,
            acceleration_structures,
            ray_tracing(false),
            ray_tracing(true),
        ),
        (
            wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES,
            acceleration_structures,
            ray_tracing(false),
            ray_tracing(true),
        ),
        (
            wgt::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN,
            &[
                khr::deferred_host_operations::NAME,
                khr::acceleration_structure::NAME,
                khr::buffer_device_address::NAME,
                khr::ray_tracing_position_fetch::NAME,
            ][..],
            PhysicalDeviceFeatures {
                position_fetch: Some(
                    vk::PhysicalDeviceRayTracingPositionFetchFeaturesKHR::default(),
                ),
                ..ray_tracing(true)
            },
            PhysicalDeviceFeatures {
                position_fetch: Some(
                    vk::PhysicalDeviceRayTracingPositionFetchFeaturesKHR::default()
                        .ray_tracing_position_fetch(true),
                ),
                ..ray_tracing(true)
            },
        ),
        (
            wgt::Features::EXPERIMENTAL_MESH_SHADER,
            &[ext::mesh_shader::NAME][..],
            PhysicalDeviceFeatures {
                mesh_shader: Some(vk::PhysicalDeviceMeshShaderFeaturesEXT::default()),
                ..PhysicalDeviceFeatures::default()
            },
            PhysicalDeviceFeatures {
                mesh_shader: Some(
                    vk::PhysicalDeviceMeshShaderFeaturesEXT::default()
                        .mesh_shader(true)
                        .task_shader(true),
                ),
                ..PhysicalDeviceFeatures::default()
            },
        ),
        (
            wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX,
            &[khr::cooperative_matrix::NAME][..],
            PhysicalDeviceFeatures {
                cooperative_matrix: Some(vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default()),
                vulkan_memory_model: Some(
                    vk::PhysicalDeviceVulkanMemoryModelFeatures::default()
                        .vulkan_memory_model(true)
                        .vulkan_memory_model_device_scope(true),
                ),
                ..PhysicalDeviceFeatures::default()
            },
            PhysicalDeviceFeatures {
                cooperative_matrix: Some(
                    vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default()
                        .cooperative_matrix(true),
                ),
                vulkan_memory_model: Some(
                    vk::PhysicalDeviceVulkanMemoryModelFeatures::default()
                        .vulkan_memory_model(true)
                        .vulkan_memory_model_device_scope(true),
                ),
                ..PhysicalDeviceFeatures::default()
            },
        ),
        (
            wgt::Features::SUBGROUP,
            &[][..],
            PhysicalDeviceFeatures {
                subgroup_size_control: Some(
                    vk::PhysicalDeviceSubgroupSizeControlFeatures::default(),
                ),
                ..PhysicalDeviceFeatures::default()
            },
            PhysicalDeviceFeatures {
                subgroup_size_control: Some(
                    vk::PhysicalDeviceSubgroupSizeControlFeatures::default()
                        .subgroup_size_control(true),
                ),
                ..PhysicalDeviceFeatures::default()
            },
        ),
    ]
}

/// A feature is derived from the members of the feature structures that enable what it uses: a
/// device with the feature's extensions and structures, but without those members, lacks it, as
/// a device created elsewhere may (the structures of an adapter that lists the extensions have
/// them). Before, `TEXTURE_INT64_ATOMIC` read a member it had just set, and the ray tracing,
/// mesh shader, cooperative matrix and subgroup features followed their extensions alone.
#[test]
fn a_feature_needs_the_members_that_enable_it() {
    let caps = subgroups_and_matrices();
    let mut wrong = alloc::vec::Vec::new();
    for (feature, extensions, off, on) in member_cases() {
        if derived(&caps, extensions, &off).contains(feature) {
            wrong.push(alloc::format!("{feature:?} without its members"));
        }
        if !derived(&caps, extensions, &on).contains(feature) {
            wrong.push(alloc::format!("{feature:?} lacking with its members"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A feature promoted to core needs its extension on a device older than the promotion, and the
/// two-plane formats need no structure at all: they come with `VK_KHR_sampler_ycbcr_conversion`
/// or Vulkan 1.1, and nothing wgpu-hal does with them needs `samplerYcbcrConversion`. Before, a
/// Vulkan 1.0 device with the multiview structure but without `VK_KHR_multiview` had `MULTIVIEW`,
/// and a Vulkan 1.1 device without the conversion structure lacked `TEXTURE_FORMAT_NV12`.
#[test]
fn a_promoted_feature_needs_its_extension_before_its_promotion() {
    let multiview = PhysicalDeviceFeatures {
        multiview: Some(vk::PhysicalDeviceMultiviewFeatures::default().multiview(true)),
        ..PhysicalDeviceFeatures::default()
    };
    let vulkan_1_0 = device_of_version(vk::API_VERSION_1_0, &[]);
    let vulkan_1_1 = device_of_version(vk::API_VERSION_1_1, &[]);
    assert!(!derived(&vulkan_1_0, &[], &multiview).contains(wgt::Features::MULTIVIEW));
    assert!(derived(&vulkan_1_0, &[khr::multiview::NAME], &multiview)
        .contains(wgt::Features::MULTIVIEW));
    assert!(derived(&vulkan_1_1, &[], &multiview).contains(wgt::Features::MULTIVIEW));

    let formats = FormatSupport {
        nv12: true,
        p010: true,
        ..FormatSupport::default()
    };
    let both = wgt::Features::TEXTURE_FORMAT_NV12 | wgt::Features::TEXTURE_FORMAT_P010;
    let two_plane = |caps: &PhysicalDeviceProperties, extensions: &[&CStr]| {
        PhysicalDeviceFeatures::default()
            .to_wgpu(
                caps,
                |extension| extensions.contains(&extension),
                &formats,
                &vk::QueueFamilyProperties::default(),
            )
            .0
            .intersection(both)
    };
    assert!(two_plane(&vulkan_1_0, &[]).is_empty());
    // On Vulkan 1.0 the conversion extension counts with the extensions it requires
    // (VUID-vkCreateDevice-ppEnabledExtensionNames-01387).
    assert!(two_plane(&vulkan_1_0, &[khr::sampler_ycbcr_conversion::NAME]).is_empty());
    assert_eq!(
        two_plane(
            &vulkan_1_0,
            &[
                khr::sampler_ycbcr_conversion::NAME,
                khr::bind_memory2::NAME,
                khr::get_memory_requirements2::NAME,
            ]
        ),
        both
    );
    assert_eq!(two_plane(&vulkan_1_1, &[]), both);
}

/// On a device with `VK_KHR_portability_subset`, the features that the subset leaves out need the
/// subset's members; a device without the extension has them. Before, a device with the extension
/// but without the subset's structure had them.
#[test]
fn a_portability_subset_device_needs_its_members() {
    let subset = [khr::portability_subset::NAME];
    let features = wgt::Features::TEXTURE_COMPONENT_SWIZZLE | wgt::Features::MULTISAMPLE_ARRAY;
    let none = PhysicalDeviceFeatures::default();
    let other = device_of_version(vk::API_VERSION_1_3, &[]);
    assert!(derived(&other, &[], &none).contains(features));
    // The restrictions are the implementation's: a device of one has them whether or not it
    // enabled the extension, as it must (VUID-VkDeviceCreateInfo-pProperties-04451).
    let caps = device_of_version(vk::API_VERSION_1_3, &subset);
    assert!(!derived(&caps, &subset, &none).intersects(features));
    assert!(!derived(&caps, &[], &none).intersects(features));
    let with = PhysicalDeviceFeatures {
        portability_subset: Some(
            vk::PhysicalDevicePortabilitySubsetFeaturesKHR::default()
                .image_view_format_swizzle(true)
                .multisample_array_image(true),
        ),
        ..PhysicalDeviceFeatures::default()
    };
    assert!(derived(&caps, &subset, &with).contains(features));
}

/// On an implementation of `VK_KHR_portability_subset`, which lists it, point polygons need its
/// `pointPolygons`, and opening a device for them enables both the extension and the member;
/// elsewhere they need only `fillModeNonSolid`.
#[test]
fn point_polygons_on_a_portability_subset_need_its_member() {
    let point = wgt::Features::POLYGON_MODE_POINT;
    let core = vk::PhysicalDeviceFeatures::default().fill_mode_non_solid(true);
    let non_solid = PhysicalDeviceFeatures {
        core,
        ..PhysicalDeviceFeatures::default()
    };
    let other = device_of_version(vk::API_VERSION_1_3, &[]);
    assert!(derived(&other, &[], &non_solid).contains(point));

    let subset = [khr::portability_subset::NAME];
    let caps = device_of_version(vk::API_VERSION_1_3, &subset);
    let offered = PhysicalDeviceFeatures {
        portability_subset: Some(Default::default()),
        core,
        ..PhysicalDeviceFeatures::default()
    };
    assert!(!derived(&caps, &subset, &offered).contains(point));
    let offered = PhysicalDeviceFeatures {
        portability_subset: Some(
            vk::PhysicalDevicePortabilitySubsetFeaturesKHR::default().point_polygons(true),
        ),
        core,
        ..PhysicalDeviceFeatures::default()
    };
    assert!(derived(&caps, &subset, &offered).contains(point));
    let (extensions, enabled) = opened(&caps, &offered, point);
    assert!(extensions.contains(&khr::portability_subset::NAME));
    assert_eq!(enabled.portability_subset.unwrap().point_polygons, vk::TRUE);
    assert!(derived(&caps, &extensions, &enabled).contains(point));
}

/// A device allocates memory with device addresses only with `VK_KHR_buffer_device_address` and
/// its `bufferDeviceAddress`. Before, the extension alone sufficed, and a device created elsewhere
/// with the extension but without the feature allocated against
/// VUID-VkMemoryAllocateInfo-flags-03331.
#[test]
fn device_addresses_need_the_feature_as_well_as_the_extension() {
    let extension = [khr::buffer_device_address::NAME];
    let on = PhysicalDeviceFeatures {
        buffer_device_address: Some(
            vk::PhysicalDeviceBufferDeviceAddressFeaturesKHR::default().buffer_device_address(true),
        ),
        ..PhysicalDeviceFeatures::default()
    };
    let off = PhysicalDeviceFeatures {
        buffer_device_address: Some(vk::PhysicalDeviceBufferDeviceAddressFeaturesKHR::default()),
        ..PhysicalDeviceFeatures::default()
    };
    assert!(uses_device_addresses(&extension, &on));
    assert!(!uses_device_addresses(&extension, &off));
    assert!(!uses_device_addresses(&[], &on));
}

/// The features that wgpu-hal enables to open a device with `requested` on the physical device
/// `caps`, which offers `offered`, with the extensions that `caps` lists of those it requires.
/// Returns them with those extensions.
fn opened(
    caps: &PhysicalDeviceProperties,
    offered: &PhysicalDeviceFeatures,
    requested: wgt::Features,
) -> (alloc::vec::Vec<&'static CStr>, PhysicalDeviceFeatures) {
    let extensions = caps
        .get_required_extensions(requested)
        .into_iter()
        .filter(|&extension| caps.supports_extension(extension))
        .collect::<alloc::vec::Vec<_>>();
    let enabled = PhysicalDeviceFeatures::from_extensions_and_requested_features(
        caps,
        offered,
        &extensions,
        requested,
        wgt::DownlevelFlags::empty(),
        &PrivateCapabilities::default(),
    );
    (extensions, enabled)
}

/// The binding-array features.
const BINDING_ARRAYS: [wgt::Features; 5] = [
    wgt::Features::BUFFER_BINDING_ARRAY,
    wgt::Features::TEXTURE_BINDING_ARRAY,
    wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY,
    wgt::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
    wgt::Features::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING,
];

/// Storage resource arrays enable the dynamic indexing of storage buffers and of storage images,
/// with buffer arrays, texture arrays or both. Before, the storage image one was never enabled,
/// and a second call for storage buffers, keyed on texture arrays, overwrote the first, so that
/// storage buffer arrays without texture arrays were indexed without
/// `shaderStorageBufferArrayDynamicIndexing`.
#[test]
fn storage_resource_arrays_enable_storage_dynamic_indexing() {
    let caps = device_of_version(vk::API_VERSION_1_3, &[]);
    let storage = wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY;
    for others in [
        wgt::Features::BUFFER_BINDING_ARRAY,
        wgt::Features::TEXTURE_BINDING_ARRAY,
        wgt::Features::BUFFER_BINDING_ARRAY | wgt::Features::TEXTURE_BINDING_ARRAY,
    ] {
        let (_, enabled) = opened(&caps, &PhysicalDeviceFeatures::default(), storage | others);
        assert_eq!(
            (
                enabled.core.shader_storage_buffer_array_dynamic_indexing,
                enabled.core.shader_storage_image_array_dynamic_indexing,
            ),
            (vk::TRUE, vk::TRUE),
            "{others:?}"
        );
    }
}

/// Texture and storage resource arrays need the dynamic indexing of the descriptors in their
/// arrays, and opening a device with one alone gives it. Before, a device with the descriptor
/// indexing features had every binding-array feature, whatever dynamic indexing it lacked.
#[test]
fn binding_arrays_need_dynamic_indexing() {
    let caps = device_of_version(vk::API_VERSION_1_3, &[]);
    for feature in BINDING_ARRAYS {
        let (extensions, enabled) = opened(&caps, &PhysicalDeviceFeatures::default(), feature);
        assert!(
            derived(&caps, &extensions, &enabled).contains(feature),
            "{feature:?} alone"
        );
    }
    let (extensions, mut enabled) = opened(
        &caps,
        &PhysicalDeviceFeatures::default(),
        wgt::Features::all(),
    );
    enabled.core.shader_storage_image_array_dynamic_indexing = vk::FALSE;
    let features = derived(&caps, &extensions, &enabled);
    assert!(!features.contains(wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY));
    assert!(features.contains(wgt::Features::TEXTURE_BINDING_ARRAY));
    enabled.core.shader_sampled_image_array_dynamic_indexing = vk::FALSE;
    let features = derived(&caps, &extensions, &enabled);
    assert!(!features.contains(wgt::Features::TEXTURE_BINDING_ARRAY));
}

/// Buffer arrays keep the acceptance they had before commit 47 (the owner's ruling): a device with
/// the descriptor indexing features has them without `shaderUniformBufferArrayDynamicIndexing`, so
/// no uniform or dynamic-offset buffer array that wgpu-core accepts is refused. Opening one enables
/// that feature where the adapter offers it, and only there.
#[test]
fn buffer_arrays_do_not_need_uniform_dynamic_indexing() {
    let caps = device_of_version(vk::API_VERSION_1_3, &[]);
    let buffer = wgt::Features::BUFFER_BINDING_ARRAY;
    for offered in [false, true] {
        let mut offers = PhysicalDeviceFeatures::default();
        offers.core.shader_uniform_buffer_array_dynamic_indexing = offered.into();
        let (extensions, enabled) = opened(&caps, &offers, buffer);
        assert_eq!(
            enabled.core.shader_uniform_buffer_array_dynamic_indexing,
            vk::Bool32::from(offered),
            "offered: {offered}"
        );
        assert!(derived(&caps, &extensions, &enabled).contains(buffer));
        let mut without = enabled;
        without.core.shader_uniform_buffer_array_dynamic_indexing = vk::FALSE;
        assert!(
            derived(&caps, &extensions, &without).contains(buffer),
            "offered: {offered}"
        );
    }
}

/// A Vulkan 1.2 physical device with every subgroup operation in every stage that lists the
/// extensions of subgroup size control, mesh shaders and ray tracing.
fn extending_device() -> PhysicalDeviceProperties {
    PhysicalDeviceProperties {
        subgroup: Some(
            vk::PhysicalDeviceSubgroupProperties::default()
                .supported_operations(vk::SubgroupFeatureFlags::from_raw(!0))
                .supported_stages(vk::ShaderStageFlags::ALL),
        ),
        ..device_of_version(
            vk::API_VERSION_1_2,
            &[
                ext::subgroup_size_control::NAME,
                ext::mesh_shader::NAME,
                khr::maintenance4::NAME,
                khr::deferred_host_operations::NAME,
                khr::acceleration_structure::NAME,
                khr::buffer_device_address::NAME,
                khr::ray_query::NAME,
                khr::ray_tracing_position_fetch::NAME,
            ],
        )
    }
}

/// The features that extend another, each of which a device opened with it alone has: wgpu-hal
/// enables what it needs, as for the feature it extends. Before, they enabled nothing of their
/// own, and a device opened with one of them alone lacked it.
#[test]
fn a_feature_that_extends_another_enables_what_it_needs() {
    let caps = extending_device();
    let mut wrong = alloc::vec::Vec::new();
    for feature in [
        wgt::Features::SELECTIVE_MULTIVIEW,
        wgt::Features::EXPERIMENTAL_MESH_SHADER_POINTS,
        wgt::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW,
        wgt::Features::SUBGROUP_VERTEX,
        wgt::Features::SUBGROUP_BARRIER,
        wgt::Features::EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS,
        wgt::Features::ACCELERATION_STRUCTURE_BINDING_ARRAY,
        wgt::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN,
    ] {
        let (extensions, enabled) = opened(&caps, &PhysicalDeviceFeatures::default(), feature);
        if !derived(&caps, &extensions, &enabled).contains(feature) {
            wrong.push(alloc::format!("{feature:?} lacking alone"));
        }
    }

    // `multiviewMeshShader` needs `multiview` (VUID-VkPhysicalDeviceMeshShaderFeaturesEXT-
    // multiviewMeshShader-07032), and `VK_KHR_ray_tracing_position_fetch` needs
    // `VK_KHR_acceleration_structure` (VUID-vkCreateDevice-ppEnabledExtensionNames-01387).
    let (_, enabled) = opened(
        &caps,
        &PhysicalDeviceFeatures::default(),
        wgt::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW,
    );
    if enabled.multiview.unwrap().multiview != vk::TRUE {
        wrong.push("multiviewMeshShader without multiview".into());
    }
    let (extensions, _) = opened(
        &caps,
        &PhysicalDeviceFeatures::default(),
        wgt::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN,
    );
    if !extensions.contains(&khr::acceleration_structure::NAME) {
        wrong.push("position fetch without VK_KHR_acceleration_structure".into());
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Mesh shaders in multiview render passes need `multiview` as well as `multiviewMeshShader`.
/// Before, a device with the latter but without the former had
/// `EXPERIMENTAL_MESH_SHADER_MULTIVIEW`.
#[test]
fn mesh_shader_multiview_needs_multiview() {
    let caps = extending_device();
    let mesh = |multiview: bool| PhysicalDeviceFeatures {
        mesh_shader: Some(
            vk::PhysicalDeviceMeshShaderFeaturesEXT::default()
                .mesh_shader(true)
                .task_shader(true)
                .multiview_mesh_shader(true),
        ),
        multiview: Some(vk::PhysicalDeviceMultiviewFeatures::default().multiview(multiview)),
        ..PhysicalDeviceFeatures::default()
    };
    let extensions = [ext::mesh_shader::NAME];
    let feature = wgt::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW;
    assert!(!derived(&caps, &extensions, &mesh(false)).contains(feature));
    assert!(derived(&caps, &extensions, &mesh(true)).contains(feature));
}

/// A device opened with `TEXTURE_FORMAT_P010` before Vulkan 1.1, or with
/// `EXPERIMENTAL_COOPERATIVE_MATRIX` before Vulkan 1.2, enables the extension that its formats or
/// its memory model come from, and has the feature. Before, only `TEXTURE_FORMAT_NV12` enabled
/// `VK_KHR_sampler_ycbcr_conversion`, and nothing enabled `VK_KHR_vulkan_memory_model`.
#[test]
fn older_devices_enable_the_extensions_their_features_come_from() {
    let formats = FormatSupport {
        p010: true,
        ..FormatSupport::default()
    };
    let ycbcr = [
        khr::sampler_ycbcr_conversion::NAME,
        khr::bind_memory2::NAME,
        khr::get_memory_requirements2::NAME,
    ];
    let caps = device_of_version(vk::API_VERSION_1_0, &ycbcr);
    let (extensions, enabled) = opened(
        &caps,
        &PhysicalDeviceFeatures::default(),
        wgt::Features::TEXTURE_FORMAT_P010,
    );
    let mut wrong = alloc::vec::Vec::new();
    if !ycbcr.iter().all(|extension| extensions.contains(extension)) {
        wrong.push("P010 before 1.1 without VK_KHR_sampler_ycbcr_conversion and its dependencies");
    }
    let (features, _) = enabled.to_wgpu(
        &caps,
        |extension| extensions.contains(&extension),
        &formats,
        &vk::QueueFamilyProperties::default(),
    );
    if !features.contains(wgt::Features::TEXTURE_FORMAT_P010) {
        wrong.push("P010 lacking alone before 1.1");
    }

    let caps = PhysicalDeviceProperties {
        device_api_version: vk::API_VERSION_1_1,
        supported_extensions: [
            khr::cooperative_matrix::NAME,
            khr::vulkan_memory_model::NAME,
        ]
        .iter()
        .map(|&name| {
            vk::ExtensionProperties::default()
                .extension_name(name)
                .unwrap()
        })
        .collect(),
        ..subgroups_and_matrices()
    };
    let (extensions, enabled) = opened(
        &caps,
        &PhysicalDeviceFeatures::default(),
        wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX,
    );
    if !extensions.contains(&khr::vulkan_memory_model::NAME) {
        wrong.push("cooperative matrices before 1.2 without VK_KHR_vulkan_memory_model");
    }
    if !derived(&caps, &extensions, &enabled)
        .contains(wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX)
    {
        wrong.push("cooperative matrices lacking alone before 1.2");
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The sliced 3D compression features follow the 3D image support of their formats, and need no
/// feature of a device: a device opened with one alone has it. Before,
/// `TEXTURE_COMPRESSION_BC_SLICED_3D` followed `textureCompressionBC`, which the specification
/// does not tie to 3D images, and both needed their compression feature enabled, which requesting
/// them did not enable.
#[test]
fn sliced_3d_compression_follows_3d_image_support() {
    let caps = device_of_version(vk::API_VERSION_1_3, &[]);
    let sliced = |formats: &FormatSupport, requested: wgt::Features| {
        let (extensions, enabled) = opened(&caps, &PhysicalDeviceFeatures::default(), requested);
        enabled
            .to_wgpu(
                &caps,
                |extension| extensions.contains(&extension),
                formats,
                &vk::QueueFamilyProperties::default(),
            )
            .0
    };
    let mut wrong = alloc::vec::Vec::new();
    for (feature, supported) in [
        (
            wgt::Features::TEXTURE_COMPRESSION_BC_SLICED_3D,
            FormatSupport {
                bc_3d: true,
                ..FormatSupport::default()
            },
        ),
        (
            wgt::Features::TEXTURE_COMPRESSION_ASTC_SLICED_3D,
            FormatSupport {
                astc_3d: true,
                ..FormatSupport::default()
            },
        ),
    ] {
        if !sliced(&supported, feature).contains(feature) {
            wrong.push(alloc::format!("{feature:?} lacking alone"));
        }
        let base = wgt::Features::TEXTURE_COMPRESSION_BC | wgt::Features::TEXTURE_COMPRESSION_ASTC;
        if sliced(&FormatSupport::default(), base).contains(feature) {
            wrong.push(alloc::format!("{feature:?} without 3D images"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A device has a sliced 3D feature only with its family's formats, as WebGPU's feature
/// dependencies make it imply them: a request for it alone enables `textureCompressionBC` or
/// `textureCompressionASTC_LDR`, and a device created without that feature lacks it, though its
/// physical device has the 3D images.
#[test]
fn sliced_3d_compression_implies_its_formats() {
    let caps = device_of_version(vk::API_VERSION_1_3, &[]);
    let formats = FormatSupport {
        bc_3d: true,
        astc_3d: true,
        ..FormatSupport::default()
    };
    for (sliced, base) in [
        (
            wgt::Features::TEXTURE_COMPRESSION_BC_SLICED_3D,
            wgt::Features::TEXTURE_COMPRESSION_BC,
        ),
        (
            wgt::Features::TEXTURE_COMPRESSION_ASTC_SLICED_3D,
            wgt::Features::TEXTURE_COMPRESSION_ASTC,
        ),
    ] {
        let (extensions, mut enabled) = opened(&caps, &PhysicalDeviceFeatures::default(), sliced);
        let derive = |enabled: &PhysicalDeviceFeatures| {
            enabled
                .to_wgpu(
                    &caps,
                    |extension| extensions.contains(&extension),
                    &formats,
                    &vk::QueueFamilyProperties::default(),
                )
                .0
        };
        assert!(derive(&enabled).contains(sliced | base), "{sliced:?}");
        enabled.core.texture_compression_bc = vk::FALSE;
        enabled.core.texture_compression_astc_ldr = vk::FALSE;
        assert!(!derive(&enabled).intersects(sliced | base), "{sliced:?}");
    }
}

/// The `count` `vk::Bool32` members of a feature structure, after its `s_type` and `p_next`.
///
/// # Safety
///
/// `structure` is a feature structure whose members are `count` `vk::Bool32`s.
unsafe fn members_of<T>(structure: &mut T, count: usize) -> &mut [vk::Bool32] {
    let header = size_of::<vk::BaseOutStructure<'static>>();
    let bool32 = size_of::<vk::Bool32>();
    // The members, then no more than the padding to the structure's alignment, which is never
    // read: a structure built with `Default` leaves it unspecified.
    assert!(header + count * bool32 <= size_of::<T>());
    assert!(size_of::<T>() < header + (count + 2) * bool32);
    let first: *mut u8 = core::ptr::from_mut(structure).cast();
    // SAFETY: Our caller's contract; the members follow the header, and stay borrowed.
    unsafe { core::slice::from_raw_parts_mut(first.add(header).cast(), count) }
}

/// Every member of `features`, by the name of its structure: the core features, then those of each
/// structure present. The counts are those of `ash` 0.38's definitions.
fn members(
    features: &mut PhysicalDeviceFeatures,
) -> alloc::vec::Vec<(&'static str, &mut [vk::Bool32])> {
    let mut all = alloc::vec::Vec::new();
    let core: *mut vk::PhysicalDeviceFeatures = &mut features.core;
    assert_eq!(
        size_of::<vk::PhysicalDeviceFeatures>(),
        55 * size_of::<vk::Bool32>()
    );
    // SAFETY: `vk::PhysicalDeviceFeatures` is 55 `vk::Bool32` members, and stays borrowed.
    all.push(("core", unsafe {
        core::slice::from_raw_parts_mut(core.cast(), 55)
    }));
    macro_rules! structures {
        ($($field:ident: $count:literal),* $(,)?) => {$(
            if let Some(structure) = features.$field.as_mut() {
                // SAFETY: A feature structure of `$count` members.
                all.push((stringify!($field), unsafe { members_of(structure, $count) }));
            }
        )*};
    }
    structures!(
        descriptor_indexing: 20,
        timeline_semaphore: 1,
        image_robustness: 1,
        robustness2: 3,
        multiview: 3,
        astc_hdr: 1,
        shader_float16_int8: 2,
        _16bit_storage: 4,
        acceleration_structure: 5,
        buffer_device_address: 3,
        ray_query: 1,
        ray_tracing_pipeline: 5,
        zero_initialize_workgroup_memory: 1,
        position_fetch: 1,
        shader_atomic_int64: 2,
        shader_image_atomic_int64: 2,
        shader_atomic_float: 12,
        subgroup_size_control: 2,
        maintenance4: 1,
        mesh_shader: 5,
        shader_integer_dot_product: 1,
        shader_barycentrics: 1,
        portability_subset: 15,
        cooperative_matrix: 2,
        vulkan_memory_model: 3,
        shader_draw_parameters: 1,
        swapchain_maintenance1: 1,
        shader_subgroup_extended_types: 1,
    );
    all
}

/// Every device extension that a wgpu feature, or wgpu-hal itself, depends on.
const EVERY_EXTENSION: &[&CStr] = &[
    khr::swapchain::NAME,
    khr::swapchain_mutable_format::NAME,
    khr::maintenance1::NAME,
    khr::maintenance2::NAME,
    khr::maintenance3::NAME,
    khr::maintenance4::NAME,
    khr::storage_buffer_storage_class::NAME,
    khr::multiview::NAME,
    khr::sampler_ycbcr_conversion::NAME,
    khr::_16bit_storage::NAME,
    khr::shader_draw_parameters::NAME,
    khr::image_format_list::NAME,
    khr::driver_properties::NAME,
    khr::timeline_semaphore::NAME,
    ext::descriptor_indexing::NAME,
    khr::shader_float16_int8::NAME,
    khr::spirv_1_4::NAME,
    ext::image_robustness::NAME,
    ext::robustness2::NAME,
    ext::subgroup_size_control::NAME,
    khr::shader_integer_dot_product::NAME,
    khr::zero_initialize_workgroup_memory::NAME,
    khr::dynamic_rendering::NAME,
    khr::load_store_op_none::NAME,
    khr::shader_non_semantic_info::NAME,
    khr::draw_indirect_count::NAME,
    khr::deferred_host_operations::NAME,
    khr::acceleration_structure::NAME,
    khr::buffer_device_address::NAME,
    khr::ray_query::NAME,
    khr::ray_tracing_pipeline::NAME,
    khr::ray_tracing_position_fetch::NAME,
    ext::conservative_rasterization::NAME,
    ext::texture_compression_astc_hdr::NAME,
    khr::shader_atomic_int64::NAME,
    ext::shader_image_atomic_int64::NAME,
    ext::shader_atomic_float::NAME,
    ext::mesh_shader::NAME,
    khr::fragment_shader_barycentric::NAME,
    khr::cooperative_matrix::NAME,
    khr::vulkan_memory_model::NAME,
    khr::shader_subgroup_extended_types::NAME,
    khr::shader_float_controls::NAME,
    khr::bind_memory2::NAME,
    khr::get_memory_requirements2::NAME,
];

/// A physical device of Vulkan `version` that offers everything: every extension of
/// [`EVERY_EXTENSION`], every member of every feature structure, every subgroup operation in every
/// stage, a cooperative matrix configuration, every format that a feature depends on, and a queue
/// with timestamps. Returns it with what it offers, its format support and its queue.
fn offers_everything(
    version: u32,
) -> (
    PhysicalDeviceProperties,
    PhysicalDeviceFeatures,
    FormatSupport,
    vk::QueueFamilyProperties,
) {
    let caps = PhysicalDeviceProperties {
        device_api_version: version,
        supported_extensions: EVERY_EXTENSION
            .iter()
            .map(|&name| {
                vk::ExtensionProperties::default()
                    .extension_name(name)
                    .unwrap()
            })
            .collect(),
        ..subgroups_and_matrices()
    };
    let mut offered = PhysicalDeviceFeatures {
        descriptor_indexing: Some(Default::default()),
        timeline_semaphore: Some(Default::default()),
        image_robustness: Some(Default::default()),
        robustness2: Some(Default::default()),
        multiview: Some(Default::default()),
        astc_hdr: Some(Default::default()),
        shader_float16_int8: Some(Default::default()),
        _16bit_storage: Some(Default::default()),
        acceleration_structure: Some(Default::default()),
        buffer_device_address: Some(Default::default()),
        ray_query: Some(Default::default()),
        ray_tracing_pipeline: Some(Default::default()),
        zero_initialize_workgroup_memory: Some(Default::default()),
        position_fetch: Some(Default::default()),
        shader_atomic_int64: Some(Default::default()),
        shader_image_atomic_int64: Some(Default::default()),
        shader_atomic_float: Some(Default::default()),
        subgroup_size_control: Some(Default::default()),
        maintenance4: Some(Default::default()),
        mesh_shader: Some(Default::default()),
        shader_integer_dot_product: Some(Default::default()),
        shader_barycentrics: Some(Default::default()),
        portability_subset: None,
        cooperative_matrix: Some(Default::default()),
        vulkan_memory_model: Some(Default::default()),
        shader_draw_parameters: Some(Default::default()),
        swapchain_maintenance1: None,
        shader_subgroup_extended_types: Some(Default::default()),
        ..PhysicalDeviceFeatures::default()
    };
    for (_, members) in members(&mut offered) {
        members.fill(vk::TRUE);
    }
    let formats = FormatSupport {
        s8: true,
        d32: true,
        d24_s8: true,
        d32_s8: true,
        rg11b10ufloat_renderable: true,
        bgra8unorm_storage: true,
        float32_filterable: true,
        float32_blendable: true,
        norm16: true,
        bc_3d: true,
        astc_3d: true,
        nv12: true,
        p010: true,
    };
    let queue = vk::QueueFamilyProperties::default().timestamp_valid_bits(64);
    (caps, offered, formats, queue)
}

/// What wgpu-hal enables for `requested` on the physical device `caps`, which offers `offered`
/// and has the downlevel flags `downlevel_flags`: the extensions and the features.
fn enabled_for(
    caps: &PhysicalDeviceProperties,
    offered: &PhysicalDeviceFeatures,
    requested: wgt::Features,
    downlevel_flags: wgt::DownlevelFlags,
) -> (alloc::vec::Vec<&'static CStr>, PhysicalDeviceFeatures) {
    let extensions = caps
        .get_required_extensions(requested)
        .into_iter()
        .filter(|&extension| caps.supports_extension(extension))
        .collect::<alloc::vec::Vec<_>>();
    let enabled = PhysicalDeviceFeatures::from_extensions_and_requested_features(
        caps,
        offered,
        &extensions,
        requested,
        downlevel_flags,
        &PrivateCapabilities::default(),
    );
    (extensions, enabled)
}

/// What a request enables beyond need, by the feature, the structure and member or extension, and
/// the oldest Vulkan version at which it does so. The derivation does not require them:
/// - Mesh shaders enable `maintenance4`, whose only gate is the `LocalSizeId` execution mode
///   (VUID-RuntimeSpirv-LocalSizeId-06434), which Naga does not emit.
/// - 16-bit types enable `storageInputOutput16` where offered: Naga uses it where present and
///   passes 16-bit inputs and outputs through 32-bit types without it.
/// - Buffer arrays enable `shaderUniformBufferArrayDynamicIndexing` (`core[33]`) where offered,
///   for the uniform buffer arrays that wgpu-core lets through with them; by the owner's ruling,
///   buffer arrays do not need it.
/// - `VK_KHR_shader_atomic_int64` is enabled on the versions that have it in core.
fn beyond_need(feature: wgt::Features, what: &str, version: u32) -> bool {
    use wgt::Features as F;
    let mesh = F::EXPERIMENTAL_MESH_SHADER
        | F::EXPERIMENTAL_MESH_SHADER_POINTS
        | F::EXPERIMENTAL_MESH_SHADER_MULTIVIEW;
    (mesh.contains(feature) && what.starts_with("maintenance4["))
        || (feature == F::BUFFER_BINDING_ARRAY && what == "core[33]")
        || ((F::SHADER_F16 | F::SHADER_I16).contains(feature) && what == "_16bit_storage[3]")
        || ((F::SHADER_INT64_ATOMIC_ALL_OPS | F::SHADER_INT64_ATOMIC_MIN_MAX).contains(feature)
            && what == "VK_KHR_shader_atomic_int64"
            && version >= vk::API_VERSION_1_2)
}

/// On a physical device that offers everything, at Vulkan 1.0, 1.1, 1.2 and 1.3, every feature of
/// the adapter round-trips: a device that wgpu-hal opens with it alone, or with all of them, has
/// it, as `device_from_raw` requires. And every member and every extension that the request
/// enables beyond what opening with no feature enables is needed: without it, the device lacks the
/// feature, but for what [`beyond_need`] lists.
#[test]
fn every_feature_round_trips_and_needs_what_it_enables() {
    let mut wrong = alloc::vec::Vec::new();
    for version in [
        vk::API_VERSION_1_0,
        vk::API_VERSION_1_1,
        vk::API_VERSION_1_2,
        vk::API_VERSION_1_3,
    ] {
        let v = alloc::format!(
            "Vulkan {}.{}",
            vk::api_version_major(version),
            vk::api_version_minor(version)
        );
        let (caps, offered, formats, queue) = offers_everything(version);
        let derive = |extensions: &[&'static CStr], enabled: &PhysicalDeviceFeatures| {
            enabled
                .to_wgpu(
                    &caps,
                    |extension| extensions.contains(&extension),
                    &formats,
                    &queue,
                )
                .0
        };
        let (adapter_features, adapter_flags) = offered.to_wgpu(
            &caps,
            |extension| caps.supports_extension(extension),
            &formats,
            &queue,
        );
        let (all_extensions, all_enabled) =
            enabled_for(&caps, &offered, adapter_features, adapter_flags);
        let lacking = adapter_features - derive(&all_extensions, &all_enabled);
        if !lacking.is_empty() {
            wrong.push(alloc::format!("{v}: all together, lacking {lacking:?}"));
        }
        let (base_extensions, mut base) =
            enabled_for(&caps, &offered, wgt::Features::empty(), adapter_flags);
        let base_members = members(&mut base)
            .into_iter()
            .map(|(name, members)| (name, members.to_vec()))
            .collect::<alloc::vec::Vec<_>>();
        for (name, feature) in adapter_features.iter_names() {
            let (extensions, mut enabled) = enabled_for(&caps, &offered, feature, adapter_flags);
            if !derive(&extensions, &enabled).contains(feature) {
                wrong.push(alloc::format!("{v}: {name} lacking alone"));
                continue;
            }
            // Each member that the request adds.
            let added = members(&mut enabled)
                .into_iter()
                .flat_map(|(structure, members)| {
                    let before = base_members
                        .iter()
                        .find(|(base, _)| *base == structure)
                        .map(|(_, members)| members.clone());
                    members
                        .iter()
                        .enumerate()
                        .filter(move |&(index, &member)| {
                            member == vk::TRUE
                                && before
                                    .as_ref()
                                    .is_none_or(|before| before[index] != vk::TRUE)
                        })
                        .map(move |(index, _)| (structure, index))
                        .collect::<alloc::vec::Vec<_>>()
                })
                .collect::<alloc::vec::Vec<_>>();
            for (structure, index) in added {
                let what = alloc::format!("{structure}[{index}]");
                let (extensions, mut without) =
                    enabled_for(&caps, &offered, feature, adapter_flags);
                for (name, members) in members(&mut without) {
                    if name == structure {
                        members[index] = vk::FALSE;
                    }
                }
                let needed = !derive(&extensions, &without).contains(feature);
                if needed == beyond_need(feature, &what, version) {
                    wrong.push(alloc::format!(
                        "{v}: {name} {} {what}",
                        if needed {
                            "needs, as listed beyond need,"
                        } else {
                            "does not need"
                        }
                    ));
                }
            }
            // Each extension that the request adds.
            for &extension in extensions
                .iter()
                .filter(|extension| !base_extensions.contains(extension))
            {
                let what = extension.to_str().unwrap();
                let fewer = extensions
                    .iter()
                    .copied()
                    .filter(|&other| other != extension)
                    .collect::<alloc::vec::Vec<_>>();
                let needed = !derive(&fewer, &enabled).contains(feature);
                if needed == beyond_need(feature, what, version) {
                    wrong.push(alloc::format!(
                        "{v}: {name} {} {what}",
                        if needed {
                            "needs, as listed beyond need,"
                        } else {
                            "does not need"
                        }
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// xorshift64*: deterministic, with no dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn chance(&mut self, num: u64, den: u64) -> bool {
        self.next() % den < num
    }
}

/// A random partial adapter: of Vulkan 1.0 to 1.4, listing each extension wgpu-hal knows with
/// probability 3/4 and `VK_KHR_portability_subset` with 1/4, with the feature structures that
/// `PhysicalDeviceFeatures::queried` queries on it (none without
/// `VK_KHR_get_physical_device_properties2`, with probability 1/8), random members, formats,
/// subgroup properties, matrix configurations, driver and timestamp bits; the private
/// capabilities derived as `Instance::expose_adapter` derives them. Returns its properties,
/// offered features, format support, queue family and private capabilities.
fn random_adapter(
    rng: &mut Rng,
) -> (
    PhysicalDeviceProperties,
    PhysicalDeviceFeatures,
    FormatSupport,
    vk::QueueFamilyProperties,
    PrivateCapabilities,
) {
    let versions = [
        vk::API_VERSION_1_0,
        vk::API_VERSION_1_1,
        vk::API_VERSION_1_2,
        vk::API_VERSION_1_3,
        vk::make_api_version(0, 1, 4, 0),
    ];
    let version = versions[(rng.next() % versions.len() as u64) as usize];
    let mut extensions: alloc::vec::Vec<&CStr> = EVERY_EXTENSION
        .iter()
        .copied()
        .filter(|_| rng.chance(3, 4))
        .collect();
    let portability = rng.chance(1, 4);
    if portability {
        extensions.push(khr::portability_subset::NAME);
    }
    let mut caps = device_of_version(version, &extensions);
    if version >= vk::API_VERSION_1_1 {
        let all = vk::SubgroupFeatureFlags::from_raw(!0);
        caps.subgroup = Some(
            vk::PhysicalDeviceSubgroupProperties::default()
                .supported_operations(if rng.chance(3, 4) {
                    all
                } else {
                    vk::SubgroupFeatureFlags::from_raw(rng.next() as u32 & all.as_raw())
                })
                .supported_stages(if rng.chance(1, 2) {
                    vk::ShaderStageFlags::ALL
                } else {
                    vk::ShaderStageFlags::from_raw(rng.next() as u32)
                }),
        );
    }
    if rng.chance(1, 2) {
        caps.cooperative_matrix_properties = subgroups_and_matrices().cooperative_matrix_properties;
    }
    if rng.chance(2, 3) {
        caps.driver = Some(vk::PhysicalDeviceDriverProperties::default().driver_id(
            if rng.chance(1, 4) {
                vk::DriverId::MOLTENVK
            } else {
                vk::DriverId::MESA_RADV
            },
        ));
    }
    let mut offered = if rng.chance(7, 8) {
        PhysicalDeviceFeatures::queried(&caps)
    } else {
        PhysicalDeviceFeatures::default()
    };
    for (_, members) in members(&mut offered) {
        for member in members.iter_mut() {
            *member = vk::Bool32::from(rng.chance(3, 4));
        }
    }
    let formats = FormatSupport {
        s8: rng.chance(3, 4),
        d32: rng.chance(3, 4),
        d24_s8: rng.chance(3, 4),
        d32_s8: rng.chance(3, 4),
        rg11b10ufloat_renderable: rng.chance(3, 4),
        bgra8unorm_storage: rng.chance(3, 4),
        float32_filterable: rng.chance(3, 4),
        float32_blendable: rng.chance(3, 4),
        norm16: rng.chance(3, 4),
        bc_3d: rng.chance(3, 4),
        astc_3d: rng.chance(3, 4),
        nv12: rng.chance(3, 4),
        p010: rng.chance(3, 4),
    };
    let queue = vk::QueueFamilyProperties::default()
        .timestamp_valid_bits([0, 36, 64][(rng.next() % 3) as usize]);
    let mut private_caps = PrivateCapabilities::default();
    private_caps.set_extension_and_feature_capabilities(
        &caps,
        |extension| caps.supports_extension(extension),
        &offered,
    );
    (caps, offered, formats, queue, private_caps)
}

/// Over 3000 random partial adapters, each feature that an adapter offers round-trips through
/// wgpu-hal's own opening, alone and with every other: the device that `open` creates for it
/// gives it, so `open` never refuses a request for what the adapter offers. Any change to what an
/// adapter queries or offers, or to what `open` enables, runs through it.
#[test]
fn partial_adapters_round_trip() {
    let mut rng = Rng(0x5EED_0001);
    let mut wrong = alloc::vec::Vec::new();
    let mut checked = 0usize;
    for _ in 0..3000 {
        let (caps, offered, formats, queue, private_caps) = random_adapter(&mut rng);
        let (features, downlevel) = offered.to_wgpu(
            &caps,
            |extension| caps.supports_extension(extension),
            &formats,
            &queue,
        );
        let open = |requested: wgt::Features| {
            let extensions = caps
                .get_required_extensions(requested)
                .into_iter()
                .filter(|&extension| caps.supports_extension(extension))
                .collect::<alloc::vec::Vec<_>>();
            let enabled = PhysicalDeviceFeatures::from_extensions_and_requested_features(
                &caps,
                &offered,
                &extensions,
                requested,
                downlevel,
                &private_caps,
            );
            let (device, _) = enabled.to_wgpu(
                &caps,
                |extension| extensions.contains(&extension),
                &formats,
                &queue,
            );
            device
        };
        for (name, feature) in features.iter_names() {
            checked += 1;
            if !open(feature).contains(feature) {
                wrong.push(alloc::format!(
                    "{name} alone, Vulkan 1.{}: {:?}",
                    vk::api_version_minor(caps.device_api_version),
                    caps.supported_extensions.len()
                ));
            }
        }
        let lacking = features - open(features);
        if !lacking.is_empty() {
            wrong.push(alloc::format!(
                "all together, Vulkan 1.{}, lacking {lacking:?}",
                vk::api_version_minor(caps.device_api_version)
            ));
        }
    }
    assert!(checked > 30_000, "{checked}");
    assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(20)]);
}

/// An adapter queries the structures of the extensions promoted to its version, which are core
/// there, without the extensions: a device of it can be created with them, and its features follow
/// them. A Vulkan 1.3 device that does not list `VK_EXT_texture_compression_astc_hdr` offers
/// `TEXTURE_COMPRESSION_ASTC_HDR` with the feature, and opens with it without the extension.
#[test]
fn promoted_structures_are_queried_at_their_version() {
    for (version, expected) in [
        (vk::API_VERSION_1_0, 0),
        (vk::API_VERSION_1_1, 1),
        (vk::API_VERSION_1_2, 4),
        (vk::API_VERSION_1_3, 6),
    ] {
        let queried = PhysicalDeviceFeatures::queried(&device_of_version(version, &[]));
        let promoted = [
            queried._16bit_storage.is_some(),
            queried.descriptor_indexing.is_some(),
            queried.timeline_semaphore.is_some(),
            queried.buffer_device_address.is_some(),
            queried.image_robustness.is_some(),
            queried.astc_hdr.is_some(),
        ];
        assert_eq!(
            promoted.iter().filter(|&&queried| queried).count(),
            expected,
            "Vulkan 1.{}: {promoted:?}",
            vk::api_version_minor(version)
        );
    }

    let caps = device_of_version(vk::API_VERSION_1_3, &[]);
    let mut offered = PhysicalDeviceFeatures::queried(&caps);
    offered
        .astc_hdr
        .as_mut()
        .unwrap()
        .texture_compression_astc_hdr = vk::TRUE;
    let hdr = wgt::Features::TEXTURE_COMPRESSION_ASTC_HDR;
    assert!(derived(&caps, &[], &offered).contains(hdr));
    let (extensions, enabled) = opened(&caps, &offered, hdr);
    assert!(!extensions.contains(&ext::texture_compression_astc_hdr::NAME));
    assert!(derived(&caps, &extensions, &enabled).contains(hdr));
}

/// Subgroup operations take the 16- and 64-bit types that `SHADER_F16`, `SHADER_I16` and
/// `SHADER_INT64` give shaders only with `shaderSubgroupExtendedTypes`: an adapter with one of them
/// but without it offers no subgroup feature, a device created with one but without it has none,
/// and opening a device with a subgroup feature and one of them enables it, while a subgroup feature
/// alone does not.
#[test]
fn subgroups_with_16_and_64_bit_types_need_extended_types() {
    let (caps, mut offered, formats, queue) = offers_everything(vk::API_VERSION_1_2);
    let derive = |extensions: &[&'static CStr], features: &PhysicalDeviceFeatures| {
        features
            .to_wgpu(
                &caps,
                |extension| extensions.contains(&extension),
                &formats,
                &queue,
            )
            .0
    };
    let types = wgt::Features::SHADER_F16 | wgt::Features::SHADER_I16 | wgt::Features::SHADER_INT64;
    let offer = |offered: &PhysicalDeviceFeatures| {
        offered
            .to_wgpu(
                &caps,
                |extension| caps.supports_extension(extension),
                &formats,
                &queue,
            )
            .0
    };
    let offers = offer(&offered);
    assert!(
        offers.contains(wgt::Features::SUBGROUP | types),
        "{offers:?}"
    );
    offered
        .shader_subgroup_extended_types
        .as_mut()
        .unwrap()
        .shader_subgroup_extended_types = vk::FALSE;
    let without = offer(&offered);
    assert!(
        without.contains(types) && !without.intersects(super::SUBGROUP_FEATURES),
        "{without:?}"
    );
    offered
        .shader_subgroup_extended_types
        .as_mut()
        .unwrap()
        .shader_subgroup_extended_types = vk::TRUE;

    let (_, alone) = opened(&caps, &offered, wgt::Features::SUBGROUP);
    assert!(alone.shader_subgroup_extended_types.is_none());
    let (extensions, both) = opened(
        &caps,
        &offered,
        wgt::Features::SUBGROUP | wgt::Features::SHADER_F16,
    );
    assert_eq!(
        both.shader_subgroup_extended_types
            .unwrap()
            .shader_subgroup_extended_types,
        vk::TRUE
    );
    assert!(
        derive(&extensions, &both).contains(wgt::Features::SUBGROUP | wgt::Features::SHADER_F16)
    );
}

/// Naga lets subgroup operations into task and mesh shaders, so a device with mesh shaders has
/// the subgroup features only where its subgroup stages include the task and mesh stages; one
/// without mesh shaders has them without those stages.
#[test]
fn subgroups_with_mesh_shaders_need_the_task_and_mesh_stages() {
    let (mut caps, offered, formats, queue) = offers_everything(vk::API_VERSION_1_3);
    let mesh = wgt::Features::EXPERIMENTAL_MESH_SHADER;
    for (stages, subgroups) in [
        (vk::ShaderStageFlags::ALL, true),
        (
            vk::ShaderStageFlags::VERTEX
                | vk::ShaderStageFlags::FRAGMENT
                | vk::ShaderStageFlags::COMPUTE,
            false,
        ),
    ] {
        caps.subgroup = Some(
            vk::PhysicalDeviceSubgroupProperties::default()
                .supported_operations(vk::SubgroupFeatureFlags::from_raw(!0))
                .supported_stages(stages),
        );
        let derive = |extensions: &[&'static CStr], features: &PhysicalDeviceFeatures| {
            features
                .to_wgpu(
                    &caps,
                    |extension| extensions.contains(&extension),
                    &formats,
                    &queue,
                )
                .0
        };
        let offers = offered
            .to_wgpu(
                &caps,
                |extension| caps.supports_extension(extension),
                &formats,
                &queue,
            )
            .0;
        assert!(offers.contains(mesh), "{stages:?}");
        assert_eq!(
            offers.contains(wgt::Features::SUBGROUP),
            subgroups,
            "{stages:?}"
        );
        let (extensions, enabled) = opened(&caps, &offered, wgt::Features::SUBGROUP);
        assert!(
            derive(&extensions, &enabled).contains(wgt::Features::SUBGROUP),
            "{stages:?}: without mesh shaders"
        );
    }
}

/// 64-bit atomic minimum and maximum are storage-buffer operations: they need
/// `shaderBufferInt64Atomics` alone, and opening a device for them enables it alone; all
/// operations also need, and enable, `shaderSharedInt64Atomics`.
#[test]
fn int64_atomic_min_max_needs_only_buffer_atomics() {
    let (caps, mut offered, formats, queue) = offers_everything(vk::API_VERSION_1_2);
    offered
        .shader_atomic_int64
        .as_mut()
        .unwrap()
        .shader_shared_int64_atomics = vk::FALSE;
    let offers = offered
        .to_wgpu(
            &caps,
            |extension| caps.supports_extension(extension),
            &formats,
            &queue,
        )
        .0;
    assert!(offers.contains(wgt::Features::SHADER_INT64_ATOMIC_MIN_MAX));
    assert!(!offers.contains(wgt::Features::SHADER_INT64_ATOMIC_ALL_OPS));
    for (feature, shared) in [
        (wgt::Features::SHADER_INT64_ATOMIC_MIN_MAX, vk::FALSE),
        (wgt::Features::SHADER_INT64_ATOMIC_ALL_OPS, vk::TRUE),
    ] {
        let (_, enabled) = opened(&caps, &offered, feature);
        let atomics = enabled.shader_atomic_int64.unwrap();
        assert_eq!(
            (
                atomics.shader_buffer_int64_atomics,
                atomics.shader_shared_int64_atomics
            ),
            (vk::TRUE, shared),
            "{feature:?}"
        );
    }
}

/// Below Vulkan 1.2, opening a device enables the extensions that the extensions it enables
/// depend on, and a feature needs them: `VK_KHR_ray_query` needs `VK_KHR_spirv_1_4`, which needs
/// `VK_KHR_shader_float_controls`; `VK_KHR_acceleration_structure` needs
/// `VK_EXT_descriptor_indexing`; `VK_KHR_sampler_ycbcr_conversion` needs `VK_KHR_bind_memory2`
/// and `VK_KHR_get_memory_requirements2` (VUID-vkCreateDevice-ppEnabledExtensionNames-01387).
#[test]
fn enabled_extensions_bring_the_extensions_they_depend_on() {
    let mut wrong = alloc::vec::Vec::new();
    for (version, feature, dependencies) in [
        (
            vk::API_VERSION_1_1,
            wgt::Features::EXPERIMENTAL_RAY_QUERY,
            &[
                khr::spirv_1_4::NAME,
                khr::shader_float_controls::NAME,
                ext::descriptor_indexing::NAME,
            ][..],
        ),
        (
            vk::API_VERSION_1_1,
            wgt::Features::EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS,
            &[ext::descriptor_indexing::NAME][..],
        ),
        (
            vk::API_VERSION_1_0,
            wgt::Features::TEXTURE_FORMAT_NV12,
            &[khr::bind_memory2::NAME, khr::get_memory_requirements2::NAME][..],
        ),
    ] {
        let (caps, offered, formats, queue) = offers_everything(version);
        let derive = |extensions: &[&'static CStr], enabled: &PhysicalDeviceFeatures| {
            enabled
                .to_wgpu(
                    &caps,
                    |extension| extensions.contains(&extension),
                    &formats,
                    &queue,
                )
                .0
        };
        let (extensions, enabled) = opened(&caps, &offered, feature);
        if !derive(&extensions, &enabled).contains(feature) {
            wrong.push(alloc::format!("{feature:?} lacking alone"));
        }
        for &dependency in dependencies {
            if !extensions.contains(&dependency) {
                wrong.push(alloc::format!("{feature:?} without {dependency:?}"));
            }
            let fewer = extensions
                .iter()
                .copied()
                .filter(|&extension| extension != dependency)
                .collect::<alloc::vec::Vec<_>>();
            if derive(&fewer, &enabled).contains(feature) {
                wrong.push(alloc::format!("{feature:?} does not need {dependency:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// `InstanceShared::inspect` queries every structure that `PhysicalDeviceFeatures::queried` gives:
/// each is chained.
#[test]
fn every_queried_structure_is_chained() {
    let mut caps = device_of_version(vk::API_VERSION_1_3, EVERY_EXTENSION);
    caps.supported_extensions.push(
        vk::ExtensionProperties::default()
            .extension_name(khr::portability_subset::NAME)
            .unwrap(),
    );
    let mut queried = PhysicalDeviceFeatures::queried(&caps);
    // Every structure present, but `core`, which is not chained.
    let present = members(&mut queried).len() - 1;
    let features2 = queried.add_to_features2(vk::PhysicalDeviceFeatures2::default());
    let mut chained = 0;
    let mut next = features2
        .p_next
        .cast_const()
        .cast::<vk::BaseInStructure<'_>>();
    while !next.is_null() {
        chained += 1;
        // SAFETY: A chain of the structures of `queried`.
        next = unsafe { (*next).p_next };
    }
    assert_eq!(chained, present);
}
