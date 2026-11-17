//! The dependencies of the device extensions that wgpu-hal enables, as `vk.xml` writes them, and
//! whether a physical device can meet them.
//!
//! An extension's `depends` attribute is "a boolean expression of one or more API core versions and
//! extension names. The extension requires the expression in the string to be satisfied ... to use
//! any device-level functionality it provides (for device extensions). ... Terms which are core
//! version names are true if the corresponding API version is supported. Terms which are extension
//! names are true if the corresponding extension is enabled." (the Vulkan registry's schema,
//! "Attributes of extension tags"). Enabling an extension whose dependencies are unmet is invalid:
//! "All required device extensions for each extension in the
//! `VkDeviceCreateInfo::ppEnabledExtensionNames` list must also be present in that list"
//! (VUID-vkCreateDevice-ppEnabledExtensionNames-01387), and a device extension that depends on an
//! instance extension needs it enabled on the instance (the specification's "Extension
//! Dependencies").
//!
//! A version is supported by a device where its effective version reaches it
//! ([`super::InstanceShared::effective_device_version`]).

use core::ffi::CStr;

use ash::{ext, google, khr, vk};

use super::adapter::{KHR_SURFACE_MAINTENANCE1, KHR_SWAPCHAIN_MAINTENANCE1};

/// A `depends` expression of `vk.xml`.
#[derive(Clone, Copy, Debug)]
pub(super) enum Depends {
    /// A core version.
    Version(u32),
    /// An instance extension, met where the instance has it enabled.
    Instance(&'static CStr),
    /// A device extension, met where wgpu-hal can enable it on the device
    /// ([`Context::can_enable`]).
    Device(&'static CStr),
    /// Every one of these (`+`).
    All(&'static [Depends]),
    /// Any one of these (`,`).
    Any(&'static [Depends]),
}

use Depends::{All, Any, Device, Instance, Version};

/// No dependency.
const NONE: Depends = All(&[]);
/// `VK_KHR_get_physical_device_properties2,VK_VERSION_1_1`, the most common dependency.
const PROPERTIES2: Depends = Any(&[
    Instance(khr::get_physical_device_properties2::NAME),
    Version(vk::API_VERSION_1_1),
]);
const V1_1: Depends = Version(vk::API_VERSION_1_1);
const V1_2: Depends = Version(vk::API_VERSION_1_2);

/// Each device extension that wgpu-hal enables, with its `depends` from `vk.xml` (Vulkan SDK
/// 1.4.363.1). A dependency on an extension that is not here (`VK_KHR_external_memory`,
/// `VK_KHR_device_group`, `VK_KHR_depth_stencil_resolve`), which wgpu-hal never enables, is met
/// only through the alternative its expression offers, the core version that promoted it.
pub(super) const DEVICE_EXTENSION_DEPENDENCIES: &[(&CStr, Depends)] = &[
    (ash::qcom::render_pass_store_ops::NAME, NONE),
    (ext::conservative_rasterization::NAME, PROPERTIES2),
    // (VK_KHR_get_physical_device_properties2+VK_KHR_maintenance3),VK_VERSION_1_1
    (
        ext::descriptor_indexing::NAME,
        Any(&[
            All(&[
                Instance(khr::get_physical_device_properties2::NAME),
                Device(khr::maintenance3::NAME),
            ]),
            V1_1,
        ]),
    ),
    (
        ext::external_memory_dma_buf::NAME,
        Device(khr::external_memory_fd::NAME),
    ),
    // (((VK_KHR_bind_memory2+VK_KHR_get_physical_device_properties2+VK_KHR_sampler_ycbcr_conversion),
    // VK_VERSION_1_1)+VK_KHR_image_format_list),VK_VERSION_1_2
    (
        ext::image_drm_format_modifier::NAME,
        Any(&[
            All(&[
                Any(&[
                    All(&[
                        Device(khr::bind_memory2::NAME),
                        Instance(khr::get_physical_device_properties2::NAME),
                        Device(khr::sampler_ycbcr_conversion::NAME),
                    ]),
                    V1_1,
                ]),
                Device(khr::image_format_list::NAME),
            ]),
            V1_2,
        ]),
    ),
    (ext::image_robustness::NAME, PROPERTIES2),
    (ext::load_store_op_none::NAME, NONE),
    (ext::memory_budget::NAME, PROPERTIES2),
    // VK_KHR_spirv_1_4,VK_VERSION_1_2
    (
        ext::mesh_shader::NAME,
        Any(&[Device(khr::spirv_1_4::NAME), V1_2]),
    ),
    (ext::robustness2::NAME, PROPERTIES2),
    (ext::shader_atomic_float::NAME, PROPERTIES2),
    (ext::shader_image_atomic_int64::NAME, PROPERTIES2),
    (ext::subgroup_size_control::NAME, V1_1),
    // VK_KHR_swapchain+VK_EXT_surface_maintenance1+(VK_KHR_get_physical_device_properties2,
    // VK_VERSION_1_1)
    (
        ext::swapchain_maintenance1::NAME,
        All(&[
            Device(khr::swapchain::NAME),
            Instance(ext::surface_maintenance1::NAME),
            PROPERTIES2,
        ]),
    ),
    (ext::texture_compression_astc_hdr::NAME, PROPERTIES2),
    (google::display_timing::NAME, Device(khr::swapchain::NAME)),
    // (VK_KHR_get_physical_device_properties2+VK_KHR_storage_buffer_storage_class),VK_VERSION_1_1
    (
        khr::_16bit_storage::NAME,
        Any(&[
            All(&[
                Instance(khr::get_physical_device_properties2::NAME),
                Device(khr::storage_buffer_storage_class::NAME),
            ]),
            V1_1,
        ]),
    ),
    // ((VK_VERSION_1_1+VK_EXT_descriptor_indexing+VK_KHR_buffer_device_address),VK_VERSION_1_2)
    // +VK_KHR_deferred_host_operations
    (
        khr::acceleration_structure::NAME,
        All(&[
            Any(&[
                All(&[
                    V1_1,
                    Device(ext::descriptor_indexing::NAME),
                    Device(khr::buffer_device_address::NAME),
                ]),
                V1_2,
            ]),
            Device(khr::deferred_host_operations::NAME),
        ]),
    ),
    (khr::bind_memory2::NAME, NONE),
    // (VK_KHR_get_physical_device_properties2+VK_KHR_device_group),VK_VERSION_1_1
    (
        khr::buffer_device_address::NAME,
        Any(&[
            All(&[
                Instance(khr::get_physical_device_properties2::NAME),
                Device(khr::device_group::NAME),
            ]),
            V1_1,
        ]),
    ),
    (khr::cooperative_matrix::NAME, PROPERTIES2),
    (khr::deferred_host_operations::NAME, NONE),
    (khr::draw_indirect_count::NAME, NONE),
    (khr::driver_properties::NAME, PROPERTIES2),
    // ((VK_KHR_get_physical_device_properties2,VK_VERSION_1_1)+VK_KHR_depth_stencil_resolve),
    // VK_VERSION_1_2
    (
        khr::dynamic_rendering::NAME,
        Any(&[
            All(&[PROPERTIES2, Device(khr::depth_stencil_resolve::NAME)]),
            V1_2,
        ]),
    ),
    (
        khr::external_memory_fd::NAME,
        Any(&[Device(khr::external_memory::NAME), V1_1]),
    ),
    (
        khr::external_memory_win32::NAME,
        Any(&[Device(khr::external_memory::NAME), V1_1]),
    ),
    (khr::fragment_shader_barycentric::NAME, PROPERTIES2),
    (khr::get_memory_requirements2::NAME, NONE),
    (khr::image_format_list::NAME, NONE),
    (khr::load_store_op_none::NAME, NONE),
    (khr::maintenance1::NAME, NONE),
    (khr::maintenance2::NAME, NONE),
    (khr::maintenance3::NAME, PROPERTIES2),
    (khr::maintenance4::NAME, V1_1),
    (khr::multiview::NAME, PROPERTIES2),
    (khr::portability_subset::NAME, PROPERTIES2),
    // (VK_KHR_spirv_1_4,VK_VERSION_1_2)+VK_KHR_acceleration_structure
    (
        khr::ray_query::NAME,
        All(&[
            Any(&[Device(khr::spirv_1_4::NAME), V1_2]),
            Device(khr::acceleration_structure::NAME),
        ]),
    ),
    (
        khr::ray_tracing_pipeline::NAME,
        All(&[
            Any(&[Device(khr::spirv_1_4::NAME), V1_2]),
            Device(khr::acceleration_structure::NAME),
        ]),
    ),
    (
        khr::ray_tracing_position_fetch::NAME,
        Device(khr::acceleration_structure::NAME),
    ),
    // (VK_KHR_maintenance1+VK_KHR_bind_memory2+VK_KHR_get_memory_requirements2
    // +VK_KHR_get_physical_device_properties2),VK_VERSION_1_1
    (
        khr::sampler_ycbcr_conversion::NAME,
        Any(&[
            All(&[
                Device(khr::maintenance1::NAME),
                Device(khr::bind_memory2::NAME),
                Device(khr::get_memory_requirements2::NAME),
                Instance(khr::get_physical_device_properties2::NAME),
            ]),
            V1_1,
        ]),
    ),
    (khr::shader_atomic_int64::NAME, PROPERTIES2),
    (khr::shader_draw_parameters::NAME, NONE),
    (khr::shader_float16_int8::NAME, PROPERTIES2),
    (khr::shader_float_controls::NAME, PROPERTIES2),
    (khr::shader_integer_dot_product::NAME, PROPERTIES2),
    (khr::shader_non_semantic_info::NAME, NONE),
    (khr::shader_subgroup_extended_types::NAME, V1_1),
    // VK_VERSION_1_1+VK_KHR_shader_float_controls
    (
        khr::spirv_1_4::NAME,
        All(&[V1_1, Device(khr::shader_float_controls::NAME)]),
    ),
    (khr::storage_buffer_storage_class::NAME, NONE),
    (khr::swapchain::NAME, Instance(khr::surface::NAME)),
    // VK_KHR_swapchain+(VK_KHR_maintenance2,VK_VERSION_1_1)+(VK_KHR_image_format_list,VK_VERSION_1_2)
    (
        khr::swapchain_mutable_format::NAME,
        All(&[
            Device(khr::swapchain::NAME),
            Any(&[Device(khr::maintenance2::NAME), V1_1]),
            Any(&[Device(khr::image_format_list::NAME), V1_2]),
        ]),
    ),
    // VK_KHR_swapchain+VK_KHR_surface_maintenance1+(VK_KHR_get_physical_device_properties2,
    // VK_VERSION_1_1)
    (
        KHR_SWAPCHAIN_MAINTENANCE1,
        All(&[
            Device(khr::swapchain::NAME),
            Instance(KHR_SURFACE_MAINTENANCE1),
            PROPERTIES2,
        ]),
    ),
    (khr::timeline_semaphore::NAME, PROPERTIES2),
    (khr::vulkan_memory_model::NAME, PROPERTIES2),
    (khr::zero_initialize_workgroup_memory::NAME, PROPERTIES2),
];

/// What a physical device's extensions' dependencies are judged against.
pub(super) struct Context<'a> {
    /// The device's effective version.
    pub version: u32,
    /// The instance extensions enabled on its instance.
    pub instance_extensions: &'a [&'static CStr],
    /// Whether the physical device lists a device extension.
    pub lists: &'a dyn Fn(&CStr) -> bool,
}

impl Context<'_> {
    /// Whether wgpu-hal can enable `extension` on the device: it is one that wgpu-hal enables
    /// ([`DEVICE_EXTENSION_DEPENDENCIES`]), the device lists it, and its dependencies hold, a
    /// dependency on a device extension recursively. `vk.xml`'s dependencies have no cycles.
    pub fn can_enable(&self, extension: &CStr) -> bool {
        dependencies(extension)
            .is_some_and(|depends| (self.lists)(extension) && self.holds(depends))
    }

    fn holds(&self, depends: Depends) -> bool {
        match depends {
            Version(version) => self.version >= version,
            Instance(extension) => self.instance_extensions.contains(&extension),
            Device(extension) => self.can_enable(extension),
            All(terms) => terms.iter().all(|&term| self.holds(term)),
            Any(terms) => terms.iter().any(|&term| self.holds(term)),
        }
    }
}

/// `extension`'s dependencies, if wgpu-hal knows them.
pub(super) fn dependencies(extension: &CStr) -> Option<Depends> {
    DEVICE_EXTENSION_DEPENDENCIES
        .iter()
        .find(|&&(name, _)| name == extension)
        .map(|&(_, depends)| depends)
}
