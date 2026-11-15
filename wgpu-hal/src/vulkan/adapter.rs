use alloc::{borrow::ToOwned as _, boxed::Box, collections::BTreeMap, sync::Arc, vec::Vec};
use core::{ffi::CStr, marker::PhantomData};

use ash::{ext, google, khr, vk};
use wgpu_sync::Mutex;

use crate::{vulkan::semaphore_list::SemaphoreList, AllocationSizes};

use super::semaphore_list::SemaphoreListMode;
use feature_structures::{enable_core_features, FeatureStructure};

fn depth_stencil_required_flags() -> vk::FormatFeatureFlags {
    vk::FormatFeatureFlags::SAMPLED_IMAGE | vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT
}

const INDEXING_FEATURES: wgt::Features = wgt::Features::TEXTURE_BINDING_ARRAY
    .union(wgt::Features::BUFFER_BINDING_ARRAY)
    .union(wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY)
    .union(wgt::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING)
    .union(wgt::Features::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING)
    .union(wgt::Features::UNIFORM_BUFFER_BINDING_ARRAYS)
    .union(wgt::Features::PARTIALLY_BOUND_BINDING_ARRAY);

/// The features that use multiview: `SELECTIVE_MULTIVIEW` and `EXPERIMENTAL_MESH_SHADER_MULTIVIEW`
/// extend `MULTIVIEW`, and need the `multiview` feature as it does (the latter by
/// VUID-VkPhysicalDeviceMeshShaderFeaturesEXT-multiviewMeshShader-07032).
const MULTIVIEW_FEATURES: wgt::Features = wgt::Features::MULTIVIEW
    .union(wgt::Features::SELECTIVE_MULTIVIEW)
    .union(wgt::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW);

/// The features that use mesh shaders, each of which needs what `EXPERIMENTAL_MESH_SHADER` needs.
const MESH_SHADER_FEATURES: wgt::Features = wgt::Features::EXPERIMENTAL_MESH_SHADER
    .union(wgt::Features::EXPERIMENTAL_MESH_SHADER_POINTS)
    .union(wgt::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW);

/// The subgroup features, each of which needs what `SUBGROUP` needs of the device.
const SUBGROUP_FEATURES: wgt::Features = wgt::Features::SUBGROUP
    .union(wgt::Features::SUBGROUP_VERTEX)
    .union(wgt::Features::SUBGROUP_BARRIER);

/// The features that use acceleration structures, and need their extensions and features.
const ACCELERATION_STRUCTURE_FEATURES: wgt::Features = wgt::Features::EXPERIMENTAL_RAY_QUERY
    .union(wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES)
    .union(wgt::Features::EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS)
    .union(wgt::Features::ACCELERATION_STRUCTURE_BINDING_ARRAY)
    .union(wgt::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN);

/// The features that use ray queries: binding arrays of acceleration structures are for them.
const RAY_QUERY_FEATURES: wgt::Features = wgt::Features::EXPERIMENTAL_RAY_QUERY
    .union(wgt::Features::ACCELERATION_STRUCTURE_BINDING_ARRAY);
#[expect(rustdoc::private_intra_doc_links)]
/// Features of a [`vk::PhysicalDevice`] and its extensions.
///
/// This is used in three ways:
///
/// - When enumerating adapters, this represents the features offered by the
///   adapter. [`Instance::expose_adapter`] calls `vkGetPhysicalDeviceFeatures2`
///   (or `vkGetPhysicalDeviceFeatures` if that is not available) to collect
///   this information about the `VkPhysicalDevice` represented by the
///   `wgpu_hal::ExposedAdapter`.
///
/// - When opening a device, this represents the features we would like to
///   enable. At `wgpu_hal::Device` construction time,
///   [`PhysicalDeviceFeatures::from_extensions_and_requested_features`]
///   constructs an value of this type indicating which Vulkan features to
///   enable, based on the `wgpu_types::Features` requested.
///
/// - Given to [`Adapter::device_from_raw`], this represents the features the
///   device was created with: those wgpu-hal enabled, when it opened the device,
///   or, for a device created elsewhere, those its creator's `VkDeviceCreateInfo`
///   enabled ([`PhysicalDeviceFeatures::from_device_create_info`]). wgpu-hal takes
///   its paths from them, not from the features the adapter offers.
///
/// [`Instance::expose_adapter`]: super::Instance::expose_adapter
/// [`Adapter::device_from_raw`]: super::Adapter::device_from_raw
#[derive(Debug, Default)]
pub struct PhysicalDeviceFeatures {
    /// Basic Vulkan 1.0 features.
    core: vk::PhysicalDeviceFeatures,

    /// Features provided by `VK_EXT_descriptor_indexing`, promoted to Vulkan 1.2.
    pub(super) descriptor_indexing:
        Option<vk::PhysicalDeviceDescriptorIndexingFeaturesEXT<'static>>,

    /// Features provided by `VK_KHR_timeline_semaphore`, promoted to Vulkan 1.2
    timeline_semaphore: Option<vk::PhysicalDeviceTimelineSemaphoreFeaturesKHR<'static>>,

    /// Features provided by `VK_EXT_image_robustness`, promoted to Vulkan 1.3
    image_robustness: Option<vk::PhysicalDeviceImageRobustnessFeaturesEXT<'static>>,

    /// Features provided by `VK_EXT_robustness2`.
    robustness2: Option<vk::PhysicalDeviceRobustness2FeaturesEXT<'static>>,

    /// Features provided by `VK_KHR_multiview`, promoted to Vulkan 1.1.
    multiview: Option<vk::PhysicalDeviceMultiviewFeaturesKHR<'static>>,

    /// Features provided by `VK_EXT_texture_compression_astc_hdr`, promoted to Vulkan 1.3.
    astc_hdr: Option<vk::PhysicalDeviceTextureCompressionASTCHDRFeaturesEXT<'static>>,

    /// Features provided by `VK_KHR_shader_float16_int8`, promoted to Vulkan 1.2
    shader_float16_int8: Option<vk::PhysicalDeviceShaderFloat16Int8Features<'static>>,

    /// Features provided by `VK_KHR_16bit_storage`, promoted to Vulkan 1.1
    _16bit_storage: Option<vk::PhysicalDevice16BitStorageFeatures<'static>>,

    /// Features provided by `VK_KHR_acceleration_structure`.
    acceleration_structure: Option<vk::PhysicalDeviceAccelerationStructureFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_buffer_device_address`, promoted to Vulkan 1.2. wgpu-hal uses
    /// them for acceleration structures, through the extension's functions.
    buffer_device_address: Option<vk::PhysicalDeviceBufferDeviceAddressFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_ray_query`.
    ray_query: Option<vk::PhysicalDeviceRayQueryFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_ray_tracing_pipeline`.
    ray_tracing_pipeline: Option<vk::PhysicalDeviceRayTracingPipelineFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_zero_initialize_workgroup_memory`, promoted
    /// to Vulkan 1.3.
    zero_initialize_workgroup_memory:
        Option<vk::PhysicalDeviceZeroInitializeWorkgroupMemoryFeatures<'static>>,
    position_fetch: Option<vk::PhysicalDeviceRayTracingPositionFetchFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_shader_atomic_int64`, promoted to Vulkan 1.2.
    shader_atomic_int64: Option<vk::PhysicalDeviceShaderAtomicInt64Features<'static>>,

    /// Features provided by `VK_EXT_shader_image_atomic_int64`
    shader_image_atomic_int64: Option<vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT<'static>>,

    /// Features provided by `VK_EXT_shader_atomic_float`.
    shader_atomic_float: Option<vk::PhysicalDeviceShaderAtomicFloatFeaturesEXT<'static>>,

    /// Features provided by `VK_EXT_subgroup_size_control`, promoted to Vulkan 1.3.
    subgroup_size_control: Option<vk::PhysicalDeviceSubgroupSizeControlFeatures<'static>>,

    /// Features provided by `VK_KHR_maintenance4`, promoted to Vulkan 1.3.
    maintenance4: Option<vk::PhysicalDeviceMaintenance4FeaturesKHR<'static>>,

    /// Features proved by `VK_EXT_mesh_shader`
    mesh_shader: Option<vk::PhysicalDeviceMeshShaderFeaturesEXT<'static>>,

    /// Features provided by `VK_KHR_shader_integer_dot_product`, promoted to Vulkan 1.3.
    shader_integer_dot_product:
        Option<vk::PhysicalDeviceShaderIntegerDotProductFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_fragment_shader_barycentric`
    shader_barycentrics: Option<vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_portability_subset`.
    ///
    /// Strictly speaking this tells us what features we *don't* have compared to core.
    portability_subset: Option<vk::PhysicalDevicePortabilitySubsetFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_cooperative_matrix`
    cooperative_matrix: Option<vk::PhysicalDeviceCooperativeMatrixFeaturesKHR<'static>>,

    /// Features provided by `VK_KHR_vulkan_memory_model`, promoted to Vulkan 1.2
    vulkan_memory_model: Option<vk::PhysicalDeviceVulkanMemoryModelFeaturesKHR<'static>>,

    shader_draw_parameters: Option<vk::PhysicalDeviceShaderDrawParametersFeatures<'static>>,

    /// Features provided by `VK_KHR_swapchain_maintenance1` or `VK_EXT_swapchain_maintenance1`,
    /// whose structures are the same.
    pub(super) swapchain_maintenance1:
        Option<vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT<'static>>,
}

/// `VK_DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_BIT_KHR`, which `ash` 0.38 does not define.
pub(super) const DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_KHR: vk::DeviceQueueCreateFlags =
    vk::DeviceQueueCreateFlags::from_raw(1 << 2);

/// `VK_KHR_internally_synchronized_queues`, which `ash` 0.38 does not define.
const INTERNALLY_SYNCHRONIZED_QUEUES: &CStr = c"VK_KHR_internally_synchronized_queues";

/// `VK_KHR_surface_maintenance1`, which `ash` 0.38 does not define.
pub(super) const KHR_SURFACE_MAINTENANCE1: &CStr = c"VK_KHR_surface_maintenance1";

/// `VK_KHR_swapchain_maintenance1`, which `ash` 0.38 does not define.
const KHR_SWAPCHAIN_MAINTENANCE1: &CStr = c"VK_KHR_swapchain_maintenance1";

/// The device extensions with which presents can signal fences, `VK_KHR_swapchain_maintenance1`
/// first, each with the instance extension it depends on. Their structures are the same: the KHR
/// structure types have the values of the EXT ones.
pub(super) const SWAPCHAIN_MAINTENANCE1: [(&CStr, &CStr); 2] = [
    (KHR_SWAPCHAIN_MAINTENANCE1, KHR_SURFACE_MAINTENANCE1),
    (
        ext::swapchain_maintenance1::NAME,
        ext::surface_maintenance1::NAME,
    ),
];

/// Whether a device created with `enabled_extensions`, and with the features that
/// [`PhysicalDeviceFeatures::from_extensions_and_requested_features`] selects for them, has
/// `swapchainMaintenance1`: whether its presents can signal fences.
fn enables_swapchain_maintenance1(
    phd_features: &PhysicalDeviceFeatures,
    enabled_extensions: &[&'static CStr],
) -> bool {
    SWAPCHAIN_MAINTENANCE1
        .iter()
        .any(|(extension, _)| enabled_extensions.contains(extension))
        && phd_features
            .swapchain_maintenance1
            .is_some_and(|features| features.swapchain_maintenance1 == vk::TRUE)
}

/// Whether a device created with `enabled_extensions` and `enabled_features` allocates memory with
/// device addresses, for acceleration structures: with `VK_KHR_buffer_device_address`, whose
/// functions wgpu-hal uses, and its `bufferDeviceAddress`, without which no allocation may ask
/// for addresses (VUID-VkMemoryAllocateInfo-flags-03331).
fn uses_device_addresses(
    enabled_extensions: &[&'static CStr],
    enabled_features: &PhysicalDeviceFeatures,
) -> bool {
    enabled_extensions.contains(&khr::buffer_device_address::NAME)
        && enabled_features
            .buffer_device_address
            .is_some_and(|features| features.buffer_device_address == vk::TRUE)
}

/// The member of [`vk::PhysicalDeviceFeatures`] that holds one core feature.
pub(super) type CoreFeature = fn(&mut vk::PhysicalDeviceFeatures) -> &mut vk::Bool32;

/// The core features behind downlevel flags: `independentBlend`, `sampleRateShading`,
/// `imageCubeArray`, `samplerAnisotropy`, `fragmentStoresAndAtomics` and `depthBiasClamp`, which
/// wgpu-core validates the use of a device against, and `fullDrawIndexUint32`. An adapter has a
/// flag where it offers the feature, wgpu-hal enables the feature on the devices it opens where
/// the adapter has the flag, and a device has the flag where it was created with the feature.
///
/// `fullDrawIndexUint32` must be enabled to use index values above 2^24 - 1: "If a feature is
/// supported by a physical device, it must be enabled when creating a VkDevice in order to use its
/// associated functionality" (the Features chapter), though its limit, `maxDrawIndexedIndexValue`,
/// is reported the same either way. wgpu-core cannot validate index values, so it refuses
/// nothing without the flag.
pub(super) const DOWNLEVEL_CORE_FEATURES: [(wgt::DownlevelFlags, CoreFeature); 7] = [
    (wgt::DownlevelFlags::INDEPENDENT_BLEND, |features| {
        &mut features.independent_blend
    }),
    (wgt::DownlevelFlags::MULTISAMPLED_SHADING, |features| {
        &mut features.sample_rate_shading
    }),
    (wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES, |features| {
        &mut features.image_cube_array
    }),
    (wgt::DownlevelFlags::ANISOTROPIC_FILTERING, |features| {
        &mut features.sampler_anisotropy
    }),
    (wgt::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE, |features| {
        &mut features.fragment_stores_and_atomics
    }),
    (wgt::DownlevelFlags::DEPTH_BIAS_CLAMP, |features| {
        &mut features.depth_bias_clamp
    }),
    (wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32, |features| {
        &mut features.full_draw_index_uint32
    }),
];

/// The downlevel flags that depend on what a device was created with: those of
/// [`DOWNLEVEL_CORE_FEATURES`], and those of extensions. A device has its adapter's other flags.
const DEVICE_DOWNLEVEL_FLAGS: wgt::DownlevelFlags = wgt::DownlevelFlags::INDEPENDENT_BLEND
    .union(wgt::DownlevelFlags::MULTISAMPLED_SHADING)
    .union(wgt::DownlevelFlags::CUBE_ARRAY_TEXTURES)
    .union(wgt::DownlevelFlags::ANISOTROPIC_FILTERING)
    .union(wgt::DownlevelFlags::FRAGMENT_WRITABLE_STORAGE)
    .union(wgt::DownlevelFlags::DEPTH_BIAS_CLAMP)
    .union(wgt::DownlevelFlags::FULL_DRAW_INDEX_UINT32)
    .union(wgt::DownlevelFlags::READ_ONLY_DEPTH_STENCIL)
    .union(wgt::DownlevelFlags::SURFACE_VIEW_FORMATS);

/// The downlevel capabilities of a device of the physical device `caps`, whose adapter has
/// `adapter`, created with the features `enabled_features` and the extensions that
/// `has_extension` says it has: the adapter's, but for the [`DEVICE_DOWNLEVEL_FLAGS`], which
/// follow what the device was created with, and which it has no more of than its adapter.
fn device_downlevel_capabilities(
    adapter: &wgt::DownlevelCapabilities,
    caps: &PhysicalDeviceProperties,
    has_extension: impl Fn(&'static CStr) -> bool,
    enabled_features: &PhysicalDeviceFeatures,
) -> wgt::DownlevelCapabilities {
    wgt::DownlevelCapabilities {
        flags: adapter.flags.difference(DEVICE_DOWNLEVEL_FLAGS)
            | (adapter.flags & enabled_features.device_downlevel_flags(caps, has_extension)),
        ..adapter.clone()
    }
}

/// The structure at `structure`, if it is of type `T`. The copy may hold the caller's `p_next`: it
/// is read, not kept.
///
/// # Safety
///
/// `structure` is a valid Vulkan structure.
unsafe fn read_if_of_type<T: vk::TaggedStructure + Copy>(
    structure: *const vk::BaseInStructure<'_>,
) -> Option<T> {
    // SAFETY: A valid structure, as our caller guarantees: one of type `T` if its `s_type` says so.
    unsafe { ((*structure).s_type == T::STRUCTURE_TYPE).then(|| structure.cast::<T>().read()) }
}

/// Enables in `member`, which it inserts if absent, each feature that the structure at `structure`
/// enables, if the structure is of `member`'s type, and returns whether it was. A chain that gives
/// a structure twice, against VUID-VkDeviceCreateInfo-sType-unique, enables what either enables.
///
/// # Safety
///
/// `structure` is a valid Vulkan structure.
unsafe fn enable_if_of_type<T: FeatureStructure>(
    member: &mut Option<T>,
    structure: *const vk::BaseInStructure<'_>,
) -> bool {
    // SAFETY: A valid structure, as our caller guarantees.
    let Some(copy) = (unsafe { read_if_of_type::<T>(structure) }) else {
        return false;
    };
    member.get_or_insert_with(T::default).enable(&copy);
    true
}

/// Sets each listed member of the `$member` structure of `$features`, inserting the structure if
/// it is absent, where `$source` sets it.
macro_rules! enable_members {
    ($features:expr, $member:ident, $source:expr, [$($field:ident),* $(,)?]) => {{
        let target = $features.$member.get_or_insert_with(Default::default);
        $(target.$field |= $source.$field;)*
    }};
}

/// Defines `add_to_device_create` and `add_to_features2`, which chain each listed feature structure
/// of a `PhysicalDeviceFeatures` that is present: every one but `core`.
macro_rules! chain_structures {
    ($($field:ident),* $(,)?) => {
        /// Add the members of `self` into `info.enabled_features` and its `p_next` chain.
        pub fn add_to_device_create<'a>(
            &'a mut self,
            mut info: vk::DeviceCreateInfo<'a>,
        ) -> vk::DeviceCreateInfo<'a> {
            info = info.enabled_features(&self.core);
            $(if let Some(ref mut structure) = self.$field {
                info = info.push_next(structure);
            })*
            info
        }

        /// Add the structures of `self` to the `p_next` chain of `features2`, to query them.
        fn add_to_features2<'a>(
            &'a mut self,
            mut features2: vk::PhysicalDeviceFeatures2<'a>,
        ) -> vk::PhysicalDeviceFeatures2<'a> {
            $(if let Some(ref mut structure) = self.$field {
                features2 = features2.push_next(structure);
            })*
            features2
        }
    };
}

impl PhysicalDeviceFeatures {
    /// The features that a device was created with, from the `VkPhysicalDeviceFeatures2` that its
    /// creator gave `vkCreateDevice`: its core features, and the structures of its `p_next` chain
    /// that wgpu-hal knows, whether a `VkPhysicalDeviceVulkan11Features`,
    /// `VkPhysicalDeviceVulkan12Features` or `VkPhysicalDeviceVulkan13Features` or an extension's
    /// own structure gives them. Other structures are skipped. Give the result to
    /// [`Adapter::device_from_raw`] with the device.
    ///
    /// A `VkPhysicalDeviceFeatures2` within the chain gives its core features as well, so a device
    /// created with `VkDeviceCreateInfo::pEnabledFeatures`, or with its creator's own
    /// `VkPhysicalDeviceFeatures2` anywhere in `VkDeviceCreateInfo::pNext`, has the features of a
    /// `VkPhysicalDeviceFeatures2` whose `features` are the former (none where it is null) and
    /// whose `p_next` is that chain: [`Self::from_device_create_info`] takes those. On a chain that
    /// gives a feature twice, which Vulkan forbids, a feature is enabled where any copy enables it.
    ///
    /// # Safety
    ///
    /// `enabled.p_next` is null or a valid chain of Vulkan structures.
    ///
    /// [`Adapter::device_from_raw`]: super::Adapter::device_from_raw
    pub unsafe fn from_enabled(enabled: &vk::PhysicalDeviceFeatures2<'_>) -> Self {
        let mut features = Self {
            core: enabled.features,
            ..Self::default()
        };
        let mut next = enabled
            .p_next
            .cast_const()
            .cast::<vk::BaseInStructure<'_>>();
        while !next.is_null() {
            // SAFETY: Each structure of a valid chain, as our caller guarantees.
            unsafe {
                if let Some(nested) = read_if_of_type::<vk::PhysicalDeviceFeatures2<'_>>(next) {
                    enable_core_features(&mut features.core, &nested.features);
                } else if let Some(vulkan11) =
                    read_if_of_type::<vk::PhysicalDeviceVulkan11Features<'_>>(next)
                {
                    features.enable_vulkan11(&vulkan11);
                } else if let Some(vulkan12) =
                    read_if_of_type::<vk::PhysicalDeviceVulkan12Features<'_>>(next)
                {
                    features.enable_vulkan12(&vulkan12);
                } else if let Some(vulkan13) =
                    read_if_of_type::<vk::PhysicalDeviceVulkan13Features<'_>>(next)
                {
                    features.enable_vulkan13(&vulkan13);
                } else {
                    let _ = enable_if_of_type(&mut features.descriptor_indexing, next)
                        || enable_if_of_type(&mut features.timeline_semaphore, next)
                        || enable_if_of_type(&mut features.image_robustness, next)
                        || enable_if_of_type(&mut features.robustness2, next)
                        || enable_if_of_type(&mut features.multiview, next)
                        || enable_if_of_type(&mut features.astc_hdr, next)
                        || enable_if_of_type(&mut features.shader_float16_int8, next)
                        || enable_if_of_type(&mut features._16bit_storage, next)
                        || enable_if_of_type(&mut features.acceleration_structure, next)
                        || enable_if_of_type(&mut features.buffer_device_address, next)
                        || enable_if_of_type(&mut features.ray_query, next)
                        || enable_if_of_type(&mut features.ray_tracing_pipeline, next)
                        || enable_if_of_type(&mut features.zero_initialize_workgroup_memory, next)
                        || enable_if_of_type(&mut features.position_fetch, next)
                        || enable_if_of_type(&mut features.shader_atomic_int64, next)
                        || enable_if_of_type(&mut features.shader_image_atomic_int64, next)
                        || enable_if_of_type(&mut features.shader_atomic_float, next)
                        || enable_if_of_type(&mut features.subgroup_size_control, next)
                        || enable_if_of_type(&mut features.maintenance4, next)
                        || enable_if_of_type(&mut features.mesh_shader, next)
                        || enable_if_of_type(&mut features.shader_integer_dot_product, next)
                        || enable_if_of_type(&mut features.shader_barycentrics, next)
                        || enable_if_of_type(&mut features.portability_subset, next)
                        || enable_if_of_type(&mut features.cooperative_matrix, next)
                        || enable_if_of_type(&mut features.vulkan_memory_model, next)
                        || enable_if_of_type(&mut features.shader_draw_parameters, next)
                        || enable_if_of_type(&mut features.swapchain_maintenance1, next);
                }
                next = (*next).p_next;
            }
        }
        features
    }

    /// The features that a device was created with, from the `VkDeviceCreateInfo` that its creator
    /// gave `vkCreateDevice`: its `pEnabledFeatures`, or the `VkPhysicalDeviceFeatures2` anywhere in
    /// its `pNext` chain, and the structures of that chain that wgpu-hal knows
    /// ([`Self::from_enabled`]). Give the result to [`Adapter::device_from_raw`] with the device.
    ///
    /// # Safety
    ///
    /// `info.p_enabled_features` is null or points to a valid `VkPhysicalDeviceFeatures`, and
    /// `info.p_next` is null or a valid chain of Vulkan structures.
    ///
    /// [`Adapter::device_from_raw`]: super::Adapter::device_from_raw
    pub unsafe fn from_device_create_info(info: &vk::DeviceCreateInfo<'_>) -> Self {
        // SAFETY: Null or valid, as our caller guarantees.
        let core = unsafe { info.p_enabled_features.as_ref() }
            .copied()
            .unwrap_or_default();
        let enabled = vk::PhysicalDeviceFeatures2 {
            features: core,
            p_next: info.p_next.cast_mut(),
            ..Default::default()
        };
        // SAFETY: A valid chain, as our caller guarantees.
        unsafe { Self::from_enabled(&enabled) }
    }

    /// Enables the members of the structures that `vulkan11` includes.
    fn enable_vulkan11(&mut self, vulkan11: &vk::PhysicalDeviceVulkan11Features<'_>) {
        enable_members!(
            self,
            _16bit_storage,
            vulkan11,
            [
                storage_buffer16_bit_access,
                uniform_and_storage_buffer16_bit_access,
                storage_push_constant16,
                storage_input_output16,
            ]
        );
        enable_members!(
            self,
            multiview,
            vulkan11,
            [
                multiview,
                multiview_geometry_shader,
                multiview_tessellation_shader,
            ]
        );
        enable_members!(
            self,
            shader_draw_parameters,
            vulkan11,
            [shader_draw_parameters]
        );
    }

    /// Enables the members of the structures that `vulkan12` includes.
    fn enable_vulkan12(&mut self, vulkan12: &vk::PhysicalDeviceVulkan12Features<'_>) {
        enable_members!(
            self,
            shader_atomic_int64,
            vulkan12,
            [shader_buffer_int64_atomics, shader_shared_int64_atomics]
        );
        enable_members!(
            self,
            shader_float16_int8,
            vulkan12,
            [shader_float16, shader_int8]
        );
        enable_members!(
            self,
            descriptor_indexing,
            vulkan12,
            [
                shader_input_attachment_array_dynamic_indexing,
                shader_uniform_texel_buffer_array_dynamic_indexing,
                shader_storage_texel_buffer_array_dynamic_indexing,
                shader_uniform_buffer_array_non_uniform_indexing,
                shader_sampled_image_array_non_uniform_indexing,
                shader_storage_buffer_array_non_uniform_indexing,
                shader_storage_image_array_non_uniform_indexing,
                shader_input_attachment_array_non_uniform_indexing,
                shader_uniform_texel_buffer_array_non_uniform_indexing,
                shader_storage_texel_buffer_array_non_uniform_indexing,
                descriptor_binding_uniform_buffer_update_after_bind,
                descriptor_binding_sampled_image_update_after_bind,
                descriptor_binding_storage_image_update_after_bind,
                descriptor_binding_storage_buffer_update_after_bind,
                descriptor_binding_uniform_texel_buffer_update_after_bind,
                descriptor_binding_storage_texel_buffer_update_after_bind,
                descriptor_binding_update_unused_while_pending,
                descriptor_binding_partially_bound,
                descriptor_binding_variable_descriptor_count,
                runtime_descriptor_array,
            ]
        );
        enable_members!(self, timeline_semaphore, vulkan12, [timeline_semaphore]);
        enable_members!(
            self,
            buffer_device_address,
            vulkan12,
            [
                buffer_device_address,
                buffer_device_address_capture_replay,
                buffer_device_address_multi_device,
            ]
        );
        enable_members!(
            self,
            vulkan_memory_model,
            vulkan12,
            [
                vulkan_memory_model,
                vulkan_memory_model_device_scope,
                vulkan_memory_model_availability_visibility_chains,
            ]
        );
    }

    /// Enables the members of the structures that `vulkan13` includes.
    fn enable_vulkan13(&mut self, vulkan13: &vk::PhysicalDeviceVulkan13Features<'_>) {
        enable_members!(self, image_robustness, vulkan13, [robust_image_access]);
        enable_members!(
            self,
            subgroup_size_control,
            vulkan13,
            [subgroup_size_control, compute_full_subgroups]
        );
        enable_members!(self, astc_hdr, vulkan13, [texture_compression_astc_hdr]);
        enable_members!(
            self,
            zero_initialize_workgroup_memory,
            vulkan13,
            [shader_zero_initialize_workgroup_memory]
        );
        enable_members!(
            self,
            shader_integer_dot_product,
            vulkan13,
            [shader_integer_dot_product]
        );
        enable_members!(self, maintenance4, vulkan13, [maintenance4]);
    }

    pub fn get_core(&self) -> vk::PhysicalDeviceFeatures {
        self.core
    }

    /// The feature structures that wgpu-hal queries on the physical device that `caps` describes,
    /// with every feature off: those of the extensions it lists, and of the extensions promoted to
    /// its version, whose structures are core there. `InstanceShared::inspect` queries them with
    /// `vkGetPhysicalDeviceFeatures2`, where the instance has it. So an adapter offers what a
    /// device of it can be created with, which `device_from_raw` derives with the same rule.
    fn queried(caps: &PhysicalDeviceProperties) -> Self {
        fn query<T: Default>(condition: bool) -> Option<T> {
            condition.then(T::default)
        }
        let version = caps.device_api_version;
        let has = |extension| caps.supports_extension(extension);
        Self {
            core: vk::PhysicalDeviceFeatures::default(),
            // `VK_EXT_descriptor_indexing` is promoted to 1.2
            descriptor_indexing: query(
                version >= vk::API_VERSION_1_2 || has(ext::descriptor_indexing::NAME),
            ),
            // `VK_KHR_timeline_semaphore` is promoted to 1.2
            timeline_semaphore: query(
                version >= vk::API_VERSION_1_2 || has(khr::timeline_semaphore::NAME),
            ),
            // `VK_EXT_image_robustness` is promoted to 1.3
            image_robustness: query(
                version >= vk::API_VERSION_1_3 || has(ext::image_robustness::NAME),
            ),
            robustness2: query(has(ext::robustness2::NAME)),
            // `VK_KHR_multiview` is promoted to 1.1
            multiview: query(version >= vk::API_VERSION_1_1 || has(khr::multiview::NAME)),
            // `VK_EXT_texture_compression_astc_hdr` is promoted to 1.3
            astc_hdr: query(
                version >= vk::API_VERSION_1_3 || has(ext::texture_compression_astc_hdr::NAME),
            ),
            // `VK_KHR_shader_float16_int8` is promoted to 1.2
            shader_float16_int8: query(
                version >= vk::API_VERSION_1_2 || has(khr::shader_float16_int8::NAME),
            ),
            // `VK_KHR_16bit_storage` is promoted to 1.1
            _16bit_storage: query(version >= vk::API_VERSION_1_1 || has(khr::_16bit_storage::NAME)),
            acceleration_structure: query(has(khr::acceleration_structure::NAME)),
            // `VK_KHR_buffer_device_address` is promoted to 1.2
            buffer_device_address: query(
                version >= vk::API_VERSION_1_2 || has(khr::buffer_device_address::NAME),
            ),
            ray_query: query(has(khr::ray_query::NAME)),
            ray_tracing_pipeline: query(has(khr::ray_tracing_pipeline::NAME)),
            // `VK_KHR_zero_initialize_workgroup_memory` is promoted to 1.3
            zero_initialize_workgroup_memory: query(
                version >= vk::API_VERSION_1_3 || has(khr::zero_initialize_workgroup_memory::NAME),
            ),
            position_fetch: query(has(khr::ray_tracing_position_fetch::NAME)),
            // `VK_KHR_shader_atomic_int64` is promoted to 1.2
            shader_atomic_int64: query(
                version >= vk::API_VERSION_1_2 || has(khr::shader_atomic_int64::NAME),
            ),
            shader_image_atomic_int64: query(has(ext::shader_image_atomic_int64::NAME)),
            shader_atomic_float: query(has(ext::shader_atomic_float::NAME)),
            // `VK_EXT_subgroup_size_control` is promoted to 1.3
            subgroup_size_control: query(
                version >= vk::API_VERSION_1_3 || has(ext::subgroup_size_control::NAME),
            ),
            // `VK_KHR_maintenance4` is promoted to 1.3
            maintenance4: query(version >= vk::API_VERSION_1_3 || has(khr::maintenance4::NAME)),
            mesh_shader: query(has(ext::mesh_shader::NAME)),
            // `VK_KHR_shader_integer_dot_product` is promoted to 1.3
            shader_integer_dot_product: query(
                version >= vk::API_VERSION_1_3 || has(khr::shader_integer_dot_product::NAME),
            ),
            shader_barycentrics: query(has(khr::fragment_shader_barycentric::NAME)),
            portability_subset: query(has(khr::portability_subset::NAME)),
            cooperative_matrix: query(has(khr::cooperative_matrix::NAME)),
            // `VK_KHR_vulkan_memory_model` is promoted to 1.2
            vulkan_memory_model: query(
                version >= vk::API_VERSION_1_2 || has(khr::vulkan_memory_model::NAME),
            ),
            shader_draw_parameters: query(version >= vk::API_VERSION_1_1),
            swapchain_maintenance1: query(caps.swapchain_maintenance1().is_some()),
        }
    }

    chain_structures!(
        descriptor_indexing,
        timeline_semaphore,
        image_robustness,
        robustness2,
        multiview,
        astc_hdr,
        shader_float16_int8,
        _16bit_storage,
        acceleration_structure,
        buffer_device_address,
        ray_query,
        ray_tracing_pipeline,
        zero_initialize_workgroup_memory,
        position_fetch,
        shader_atomic_int64,
        shader_image_atomic_int64,
        shader_atomic_float,
        subgroup_size_control,
        maintenance4,
        mesh_shader,
        shader_integer_dot_product,
        shader_barycentrics,
        portability_subset,
        cooperative_matrix,
        vulkan_memory_model,
        shader_draw_parameters,
        swapchain_maintenance1,
    );

    fn supports_storage_input_output_16(&self) -> bool {
        self._16bit_storage
            .as_ref()
            .map(|features| features.storage_input_output16 != 0)
            .unwrap_or(false)
    }

    /// Create a `PhysicalDeviceFeatures` that can be used to create a logical
    /// device.
    ///
    /// Return a `PhysicalDeviceFeatures` value capturing all the Vulkan
    /// features needed for the given [`Features`], [`DownlevelFlags`], and
    /// [`PrivateCapabilities`]. You can use the returned value's
    /// [`add_to_device_create`] method to configure a
    /// [`vk::DeviceCreateInfo`] to build a logical device providing those
    /// features.
    ///
    /// To ensure that the returned value is able to select all the Vulkan
    /// features needed to express `requested_features`, `downlevel_flags`, and
    /// `private_caps`:
    ///
    /// - The given `enabled_extensions` set must include all the extensions
    ///   selected by [`Adapter::required_device_extensions`] when passed
    ///   `features`.
    ///
    /// - The given `device_api_version` must be the Vulkan API version of the
    ///   physical device we will use to create the logical device.
    ///
    /// [`Features`]: wgt::Features
    /// [`DownlevelFlags`]: wgt::DownlevelFlags
    /// [`PrivateCapabilities`]: super::PrivateCapabilities
    /// [`add_to_device_create`]: PhysicalDeviceFeatures::add_to_device_create
    /// [`Adapter::required_device_extensions`]: super::Adapter::required_device_extensions
    fn from_extensions_and_requested_features(
        phd_capabilities: &PhysicalDeviceProperties,
        phd_features: &PhysicalDeviceFeatures,
        enabled_extensions: &[&'static CStr],
        requested_features: wgt::Features,
        downlevel_flags: wgt::DownlevelFlags,
        private_caps: &super::PrivateCapabilities,
    ) -> Self {
        let device_api_version = phd_capabilities.device_api_version;
        let needs_bindless = requested_features.intersects(
            wgt::Features::TEXTURE_BINDING_ARRAY
                | wgt::Features::BUFFER_BINDING_ARRAY
                | wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY
                | wgt::Features::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING
                | wgt::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        );
        let needs_partially_bound =
            requested_features.intersects(wgt::Features::PARTIALLY_BOUND_BINDING_ARRAY);

        // vk::PhysicalDeviceFeatures is a struct composed of Bool32's while
        // Features is a bitfield so we need to map everything manually
        let mut core =
            vk::PhysicalDeviceFeatures::default()
                .robust_buffer_access(private_caps.robust_buffer_access)
                .draw_indirect_first_instance(
                    requested_features.contains(wgt::Features::INDIRECT_FIRST_INSTANCE),
                )
                //.dual_src_blend(requested_features.contains(wgt::Features::DUAL_SRC_BLENDING))
                .multi_draw_indirect(phd_features.core.multi_draw_indirect != 0)
                .fill_mode_non_solid(requested_features.intersects(
                    wgt::Features::POLYGON_MODE_LINE | wgt::Features::POLYGON_MODE_POINT,
                ))
                //.depth_bounds(requested_features.contains(wgt::Features::DEPTH_BOUNDS))
                //.alpha_to_one(requested_features.contains(wgt::Features::ALPHA_TO_ONE))
                //.multi_viewport(requested_features.contains(wgt::Features::MULTI_VIEWPORTS))
                .texture_compression_etc2(
                    requested_features.contains(wgt::Features::TEXTURE_COMPRESSION_ETC2),
                )
                // The sliced 3D features imply their families' (WebGPU's feature dependencies).
                .texture_compression_astc_ldr(requested_features.intersects(
                    wgt::Features::TEXTURE_COMPRESSION_ASTC
                        | wgt::Features::TEXTURE_COMPRESSION_ASTC_SLICED_3D,
                ))
                .texture_compression_bc(requested_features.intersects(
                    wgt::Features::TEXTURE_COMPRESSION_BC
                        | wgt::Features::TEXTURE_COMPRESSION_BC_SLICED_3D,
                ))
                //.occlusion_query_precise(requested_features.contains(wgt::Features::PRECISE_OCCLUSION_QUERY))
                .pipeline_statistics_query(
                    requested_features.contains(wgt::Features::PIPELINE_STATISTICS_QUERY),
                )
                .vertex_pipeline_stores_and_atomics(
                    requested_features.contains(wgt::Features::VERTEX_WRITABLE_STORAGE),
                )
                //.shader_image_gather_extended(
                //.shader_storage_image_extended_formats(
                // Binding arrays are indexed by dynamically uniform values. Without the dynamic
                // indexing feature of a descriptor type, arrays of it "must be indexed only by
                // constant integral expressions" (the features' text; the capabilities they allow,
                // VUID-StandaloneSpirv-UniformBufferArrayDynamicIndexing-10127,
                // -SampledImageArrayDynamicIndexing-10128, -StorageBufferArrayDynamicIndexing-10129
                // and -StorageImageArrayDynamicIndexing-10130): sampled images for texture arrays,
                // storage buffers and storage images for storage resource arrays. Uniform buffer
                // arrays, which wgpu-core lets through with buffer arrays alone, have the dynamic
                // indexing of uniform buffers where the device offers it; buffer arrays do not need
                // it, by the owner's ruling to keep their acceptance (FORK.md, residual behaviour).
                .shader_uniform_buffer_array_dynamic_indexing(
                    requested_features.contains(wgt::Features::BUFFER_BINDING_ARRAY)
                        && phd_features
                            .core
                            .shader_uniform_buffer_array_dynamic_indexing
                            != 0,
                )
                .shader_sampled_image_array_dynamic_indexing(
                    requested_features.contains(wgt::Features::TEXTURE_BINDING_ARRAY),
                )
                .shader_storage_buffer_array_dynamic_indexing(
                    requested_features.contains(wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY),
                )
                .shader_storage_image_array_dynamic_indexing(
                    requested_features.contains(wgt::Features::STORAGE_RESOURCE_BINDING_ARRAY),
                )
                .shader_clip_distance(requested_features.contains(wgt::Features::CLIP_DISTANCES))
                //.shader_cull_distance(requested_features.contains(wgt::Features::SHADER_CULL_DISTANCE))
                .shader_float64(requested_features.contains(wgt::Features::SHADER_F64))
                .shader_int64(requested_features.contains(wgt::Features::SHADER_INT64))
                .shader_int16(requested_features.contains(wgt::Features::SHADER_I16))
                //.shader_resource_residency(requested_features.contains(wgt::Features::SHADER_RESOURCE_RESIDENCY))
                .geometry_shader(requested_features.contains(wgt::Features::PRIMITIVE_INDEX))
                .depth_clamp(requested_features.contains(wgt::Features::DEPTH_CLIP_CONTROL))
                .dual_src_blend(requested_features.contains(wgt::Features::DUAL_SOURCE_BLENDING));
        for (flag, feature) in DOWNLEVEL_CORE_FEATURES {
            *feature(&mut core) = downlevel_flags.contains(flag).into();
        }

        Self {
            core,
            descriptor_indexing: if requested_features.intersects(INDEXING_FEATURES) {
                Some(
                    vk::PhysicalDeviceDescriptorIndexingFeaturesEXT::default()
                        .shader_sampled_image_array_non_uniform_indexing(needs_bindless)
                        .shader_storage_image_array_non_uniform_indexing(needs_bindless)
                        .shader_storage_buffer_array_non_uniform_indexing(needs_bindless)
                        .descriptor_binding_sampled_image_update_after_bind(needs_bindless)
                        .descriptor_binding_storage_image_update_after_bind(needs_bindless)
                        .descriptor_binding_storage_buffer_update_after_bind(needs_bindless)
                        .descriptor_binding_partially_bound(needs_partially_bound),
                )
            } else {
                None
            },
            timeline_semaphore: if device_api_version >= vk::API_VERSION_1_2
                || enabled_extensions.contains(&khr::timeline_semaphore::NAME)
            {
                Some(
                    vk::PhysicalDeviceTimelineSemaphoreFeaturesKHR::default()
                        .timeline_semaphore(private_caps.timeline_semaphores),
                )
            } else {
                None
            },
            image_robustness: if device_api_version >= vk::API_VERSION_1_3
                || enabled_extensions.contains(&ext::image_robustness::NAME)
            {
                Some(
                    vk::PhysicalDeviceImageRobustnessFeaturesEXT::default()
                        .robust_image_access(private_caps.robust_image_access),
                )
            } else {
                None
            },
            robustness2: if enabled_extensions.contains(&ext::robustness2::NAME) {
                Some(
                    vk::PhysicalDeviceRobustness2FeaturesEXT::default()
                        .robust_buffer_access2(private_caps.robust_buffer_access2)
                        .robust_image_access2(private_caps.robust_image_access2),
                )
            } else {
                None
            },
            multiview: if device_api_version >= vk::API_VERSION_1_1
                || enabled_extensions.contains(&khr::multiview::NAME)
            {
                Some(
                    vk::PhysicalDeviceMultiviewFeatures::default()
                        .multiview(requested_features.intersects(MULTIVIEW_FEATURES)),
                )
            } else {
                None
            },
            astc_hdr: if requested_features.contains(wgt::Features::TEXTURE_COMPRESSION_ASTC_HDR)
                && (device_api_version >= vk::API_VERSION_1_3
                    || enabled_extensions.contains(&ext::texture_compression_astc_hdr::NAME))
            {
                Some(
                    vk::PhysicalDeviceTextureCompressionASTCHDRFeaturesEXT::default()
                        .texture_compression_astc_hdr(true),
                )
            } else {
                None
            },
            shader_float16_int8: match requested_features.contains(wgt::Features::SHADER_F16) {
                shader_float16 if shader_float16 || private_caps.shader_int8 => Some(
                    vk::PhysicalDeviceShaderFloat16Int8Features::default()
                        .shader_float16(shader_float16)
                        .shader_int8(private_caps.shader_int8),
                ),
                _ => None,
            },
            _16bit_storage: if requested_features
                .intersects(wgt::Features::SHADER_F16 | wgt::Features::SHADER_I16)
            {
                Some(
                    vk::PhysicalDevice16BitStorageFeatures::default()
                        .storage_buffer16_bit_access(true)
                        .storage_input_output16(phd_features.supports_storage_input_output_16())
                        .uniform_and_storage_buffer16_bit_access(true),
                )
            } else {
                None
            },
            acceleration_structure: if enabled_extensions
                .contains(&khr::acceleration_structure::NAME)
            {
                Some(
                    vk::PhysicalDeviceAccelerationStructureFeaturesKHR::default()
                        .acceleration_structure(true)
                        .descriptor_binding_acceleration_structure_update_after_bind(
                            requested_features
                                .contains(wgt::Features::ACCELERATION_STRUCTURE_BINDING_ARRAY),
                        ),
                )
            } else {
                None
            },
            buffer_device_address: if enabled_extensions.contains(&khr::buffer_device_address::NAME)
            {
                Some(
                    vk::PhysicalDeviceBufferDeviceAddressFeaturesKHR::default()
                        .buffer_device_address(true),
                )
            } else {
                None
            },
            ray_query: if enabled_extensions.contains(&khr::ray_query::NAME) {
                Some(vk::PhysicalDeviceRayQueryFeaturesKHR::default().ray_query(true))
            } else {
                None
            },
            ray_tracing_pipeline: if enabled_extensions.contains(&khr::ray_tracing_pipeline::NAME) {
                Some(
                    vk::PhysicalDeviceRayTracingPipelineFeaturesKHR::default()
                        .ray_tracing_pipeline(true),
                )
            } else {
                None
            },
            zero_initialize_workgroup_memory: if device_api_version >= vk::API_VERSION_1_3
                || enabled_extensions.contains(&khr::zero_initialize_workgroup_memory::NAME)
            {
                Some(
                    vk::PhysicalDeviceZeroInitializeWorkgroupMemoryFeatures::default()
                        .shader_zero_initialize_workgroup_memory(
                            private_caps.zero_initialize_workgroup_memory,
                        ),
                )
            } else {
                None
            },
            shader_atomic_int64: if device_api_version >= vk::API_VERSION_1_2
                || enabled_extensions.contains(&khr::shader_atomic_int64::NAME)
            {
                let needed = requested_features.intersects(
                    wgt::Features::SHADER_INT64_ATOMIC_ALL_OPS
                        | wgt::Features::SHADER_INT64_ATOMIC_MIN_MAX,
                );
                Some(
                    vk::PhysicalDeviceShaderAtomicInt64Features::default()
                        .shader_buffer_int64_atomics(needed)
                        .shader_shared_int64_atomics(needed),
                )
            } else {
                None
            },
            shader_image_atomic_int64: if enabled_extensions
                .contains(&ext::shader_image_atomic_int64::NAME)
            {
                let needed = requested_features.intersects(wgt::Features::TEXTURE_INT64_ATOMIC);
                Some(
                    vk::PhysicalDeviceShaderImageAtomicInt64FeaturesEXT::default()
                        .shader_image_int64_atomics(needed),
                )
            } else {
                None
            },
            shader_atomic_float: if enabled_extensions.contains(&ext::shader_atomic_float::NAME) {
                let needed = requested_features.contains(wgt::Features::SHADER_FLOAT32_ATOMIC);
                Some(
                    vk::PhysicalDeviceShaderAtomicFloatFeaturesEXT::default()
                        .shader_buffer_float32_atomics(needed)
                        .shader_buffer_float32_atomic_add(needed),
                )
            } else {
                None
            },
            subgroup_size_control: if device_api_version >= vk::API_VERSION_1_3
                || enabled_extensions.contains(&ext::subgroup_size_control::NAME)
            {
                Some(
                    vk::PhysicalDeviceSubgroupSizeControlFeatures::default()
                        .subgroup_size_control(true),
                )
            } else {
                None
            },
            position_fetch: if enabled_extensions.contains(&khr::ray_tracing_position_fetch::NAME) {
                Some(
                    vk::PhysicalDeviceRayTracingPositionFetchFeaturesKHR::default()
                        .ray_tracing_position_fetch(true),
                )
            } else {
                None
            },
            mesh_shader: if enabled_extensions.contains(&ext::mesh_shader::NAME) {
                let needed = requested_features.intersects(MESH_SHADER_FEATURES);
                let multiview_needed =
                    requested_features.contains(wgt::Features::EXPERIMENTAL_MESH_SHADER_MULTIVIEW);
                Some(
                    vk::PhysicalDeviceMeshShaderFeaturesEXT::default()
                        .mesh_shader(needed)
                        .task_shader(needed)
                        .multiview_mesh_shader(multiview_needed),
                )
            } else {
                None
            },
            maintenance4: if device_api_version >= vk::API_VERSION_1_3
                || enabled_extensions.contains(&khr::maintenance4::NAME)
            {
                let needed = requested_features.intersects(MESH_SHADER_FEATURES);
                Some(vk::PhysicalDeviceMaintenance4Features::default().maintenance4(needed))
            } else {
                None
            },
            shader_integer_dot_product: if device_api_version >= vk::API_VERSION_1_3
                || enabled_extensions.contains(&khr::shader_integer_dot_product::NAME)
            {
                Some(
                    vk::PhysicalDeviceShaderIntegerDotProductFeaturesKHR::default()
                        .shader_integer_dot_product(private_caps.shader_integer_dot_product),
                )
            } else {
                None
            },
            shader_barycentrics: if enabled_extensions
                .contains(&khr::fragment_shader_barycentric::NAME)
            {
                let needed = requested_features.intersects(
                    wgt::Features::SHADER_BARYCENTRICS | wgt::Features::SHADER_PER_VERTEX,
                );
                Some(
                    vk::PhysicalDeviceFragmentShaderBarycentricFeaturesKHR::default()
                        .fragment_shader_barycentric(needed),
                )
            } else {
                None
            },
            portability_subset: if enabled_extensions.contains(&khr::portability_subset::NAME) {
                let image_view_format_swizzle_needed =
                    requested_features.intersects(wgt::Features::TEXTURE_COMPONENT_SWIZZLE);
                let multisample_array_needed =
                    requested_features.intersects(wgt::Features::MULTISAMPLE_ARRAY);

                Some(
                    vk::PhysicalDevicePortabilitySubsetFeaturesKHR::default()
                        .image_view_format_swizzle(image_view_format_swizzle_needed)
                        .multisample_array_image(multisample_array_needed),
                )
            } else {
                None
            },
            cooperative_matrix: if enabled_extensions.contains(&khr::cooperative_matrix::NAME) {
                let needed =
                    requested_features.contains(wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX);
                Some(
                    vk::PhysicalDeviceCooperativeMatrixFeaturesKHR::default()
                        .cooperative_matrix(needed),
                )
            } else {
                None
            },
            vulkan_memory_model: if device_api_version >= vk::API_VERSION_1_2
                || enabled_extensions.contains(&khr::vulkan_memory_model::NAME)
            {
                let needed =
                    requested_features.contains(wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX);
                Some(
                    vk::PhysicalDeviceVulkanMemoryModelFeaturesKHR::default()
                        .vulkan_memory_model(needed)
                        // The SPIR-V backend emits storage atomics with `Device`
                        // memory scope, so whenever the Vulkan memory model is
                        // enabled we must also enable device scope, otherwise the
                        // validation layers report
                        // `VUID-RuntimeSpirv-vulkanMemoryModel-06265`.
                        .vulkan_memory_model_device_scope(needed),
                )
            } else {
                None
            },
            shader_draw_parameters: if device_api_version >= vk::API_VERSION_1_1 {
                let needed = requested_features.contains(wgt::Features::SHADER_DRAW_INDEX);
                Some(
                    vk::PhysicalDeviceShaderDrawParametersFeatures::default()
                        .shader_draw_parameters(needed),
                )
            } else {
                None
            },
            // Presents signal fences, on which a swapchain's teardown waits.
            swapchain_maintenance1: if enables_swapchain_maintenance1(
                phd_features,
                enabled_extensions,
            ) {
                Some(
                    vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default()
                        .swapchain_maintenance1(true),
                )
            } else {
                None
            },
        }
    }

    /// Of the [`DEVICE_DOWNLEVEL_FLAGS`], those of a device of the physical device that `caps`
    /// describes, with the features `self` and the device extensions that `has_extension` says it
    /// has.
    fn device_downlevel_flags(
        &self,
        caps: &PhysicalDeviceProperties,
        has_extension: impl Fn(&'static CStr) -> bool,
    ) -> wgt::DownlevelFlags {
        use wgt::DownlevelFlags as Df;
        let mut flags = Df::empty();
        // `VK_IMAGE_LAYOUT_DEPTH_READ_ONLY_STENCIL_ATTACHMENT_OPTIMAL`
        // and `VK_IMAGE_LAYOUT_DEPTH_ATTACHMENT_STENCIL_READ_ONLY_OPTIMAL`
        // is required for separate read-only depth stencil.
        flags.set(
            Df::READ_ONLY_DEPTH_STENCIL,
            caps.device_api_version >= vk::API_VERSION_1_1
                || has_extension(khr::maintenance2::NAME),
        );
        flags.set(
            Df::SURFACE_VIEW_FORMATS,
            has_extension(khr::swapchain_mutable_format::NAME)
                || !has_extension(khr::swapchain::NAME),
        );
        let mut core = self.core;
        for (flag, feature) in DOWNLEVEL_CORE_FEATURES {
            flags.set(flag, *feature(&mut core) != 0);
        }
        flags
    }

    /// The wgpu [`Features`] and [`DownlevelFlags`] of a device of the physical device that `caps`
    /// and `formats` describe, with the features `self`, the device extensions that
    /// `has_extension` says it has, and a queue of the family that `queue_props` describes.
    ///
    /// For an adapter, `self` is what it offers and `has_extension` tells the extensions it
    /// supports.
    ///
    /// A feature is derived from the members of the feature structures that enable what it uses,
    /// never from a structure's presence, and from an extension where wgpu-hal uses the extension's
    /// functions or the feature is in no structure. A feature promoted to core needs its extension
    /// on a device older than the promotion.
    ///
    /// [`Features`]: wgt::Features
    /// [`DownlevelFlags`]: wgt::DownlevelFlags
    fn to_wgpu(
        &self,
        caps: &PhysicalDeviceProperties,
        has_extension: impl Fn(&'static CStr) -> bool,
        formats: &FormatSupport,
        queue_props: &vk::QueueFamilyProperties,
    ) -> (wgt::Features, wgt::DownlevelFlags) {
        use wgt::{DownlevelFlags as Df, Features as F};
        // Whether the device has what an extension promoted to core in `version` gives.
        let promoted = |version: u32, extension: &'static CStr| {
            caps.device_api_version >= version || has_extension(extension)
        };
        let mut features = F::empty()
            | F::ADDRESS_MODE_CLAMP_TO_BORDER
            | F::ADDRESS_MODE_CLAMP_TO_ZERO
            | F::CLEAR_TEXTURE
            | F::IMMEDIATES
            | F::MAPPABLE_PRIMARY_BUFFERS
            | F::MEMORY_DECORATION_COHERENT
            | F::MEMORY_DECORATION_VOLATILE
            | F::PASSTHROUGH_SHADERS
            | F::PIPELINE_CACHE
            | F::SHADER_EARLY_DEPTH_TEST
            | F::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
            | F::TEXTURE_ATOMIC
            | F::UNRESTRICTED_BYTES_PER_ROW;

        let mut dl_flags = Df::empty()
            | Df::BASE_VERTEX
            | Df::BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED
            | Df::COMPARISON_SAMPLERS
            | Df::COMPUTE_SHADERS
            | Df::DEPTH_TEXTURE_AND_BUFFER_COPIES
            | Df::FRAGMENT_STORAGE
            | Df::INDIRECT_EXECUTION
            | Df::LINEAR_INTERPOLATION
            | Df::MSL2_1
            | Df::NONBLOCKING_QUERY_RESOLVE
            | Df::NON_POWER_OF_TWO_MIPMAPPED_TEXTURES
            | Df::SHADER_F16_IN_F32
            | Df::UNRESTRICTED_EXTERNAL_TEXTURE_COPIES
            | Df::UNRESTRICTED_INDEX_BUFFER
            | Df::VERTEX_STORAGE
            | Df::VIEW_FORMATS;

        dl_flags |= self.device_downlevel_flags(caps, &has_extension);

        features.set(
            F::TIMESTAMP_QUERY
                | F::TIMESTAMP_QUERY_INSIDE_ENCODERS
                | F::TIMESTAMP_QUERY_INSIDE_PASSES,
            // Vulkan strictly defines this as either 36-64, or zero.
            queue_props.timestamp_valid_bits >= 36,
        );
        features.set(
            F::INDIRECT_FIRST_INSTANCE,
            self.core.draw_indirect_first_instance != 0,
        );
        //if self.core.dual_src_blend != 0
        features.set(F::POLYGON_MODE_LINE, self.core.fill_mode_non_solid != 0);
        features.set(F::POLYGON_MODE_POINT, self.core.fill_mode_non_solid != 0);
        //if self.core.depth_bounds != 0 {
        //if self.core.alpha_to_one != 0 {
        //if self.core.multi_viewport != 0 {
        features.set(
            F::TEXTURE_COMPRESSION_ETC2,
            self.core.texture_compression_etc2 != 0,
        );
        features.set(
            F::TEXTURE_COMPRESSION_ASTC,
            self.core.texture_compression_astc_ldr != 0,
        );
        features.set(
            F::TEXTURE_COMPRESSION_BC,
            self.core.texture_compression_bc != 0,
        );
        // The 3D images of compressed formats are a property of the physical device: no feature
        // of the specification ties them to `textureCompressionBC` or
        // `textureCompressionASTC_LDR`. A device has the sliced 3D features with its families'
        // formats, as WebGPU's feature dependencies make `texture-compression-bc-sliced-3d` imply
        // `texture-compression-bc` (and the same for ASTC).
        features.set(
            F::TEXTURE_COMPRESSION_BC_SLICED_3D,
            self.core.texture_compression_bc != 0 && formats.bc_3d,
        );
        features.set(
            F::TEXTURE_COMPRESSION_ASTC_SLICED_3D,
            self.core.texture_compression_astc_ldr != 0 && formats.astc_3d,
        );
        features.set(
            F::PIPELINE_STATISTICS_QUERY,
            self.core.pipeline_statistics_query != 0,
        );
        features.set(
            F::VERTEX_WRITABLE_STORAGE,
            self.core.vertex_pipeline_stores_and_atomics != 0,
        );

        features.set(F::SHADER_F64, self.core.shader_float64 != 0);
        features.set(F::SHADER_INT64, self.core.shader_int64 != 0);
        let storage16 = promoted(vk::API_VERSION_1_1, khr::_16bit_storage::NAME)
            && self._16bit_storage.is_some_and(|bit16| {
                bit16.storage_buffer16_bit_access != 0
                    && bit16.uniform_and_storage_buffer16_bit_access != 0
            });
        features.set(F::SHADER_I16, self.core.shader_int16 != 0 && storage16);

        features.set(F::PRIMITIVE_INDEX, self.core.geometry_shader != 0);

        features.set(
            F::SHADER_INT64_ATOMIC_ALL_OPS | F::SHADER_INT64_ATOMIC_MIN_MAX,
            promoted(vk::API_VERSION_1_2, khr::shader_atomic_int64::NAME)
                && self.shader_atomic_int64.is_some_and(|atomics| {
                    atomics.shader_buffer_int64_atomics != 0
                        && atomics.shader_shared_int64_atomics != 0
                }),
        );

        features.set(
            F::TEXTURE_INT64_ATOMIC,
            has_extension(ext::shader_image_atomic_int64::NAME)
                && self
                    .shader_image_atomic_int64
                    .is_some_and(|atomics| atomics.shader_image_int64_atomics != 0),
        );

        features.set(
            F::SHADER_FLOAT32_ATOMIC,
            has_extension(ext::shader_atomic_float::NAME)
                && self.shader_atomic_float.is_some_and(|atomics| {
                    atomics.shader_buffer_float32_atomics != 0
                        && atomics.shader_buffer_float32_atomic_add != 0
                }),
        );

        features.set(
            F::SHADER_BARYCENTRICS | F::SHADER_PER_VERTEX,
            has_extension(khr::fragment_shader_barycentric::NAME)
                && self
                    .shader_barycentrics
                    .is_some_and(|barycentrics| barycentrics.fragment_shader_barycentric != 0),
        );

        //if caps.supports_extension(khr::sampler_mirror_clamp_to_edge::NAME) {
        //if caps.supports_extension(ext::sampler_filter_minmax::NAME) {
        features.set(
            F::MULTI_DRAW_INDIRECT_COUNT,
            has_extension(khr::draw_indirect_count::NAME),
        );
        features.set(
            F::CONSERVATIVE_RASTERIZATION,
            has_extension(ext::conservative_rasterization::NAME),
        );

        let descriptor_indexing = self
            .descriptor_indexing
            .filter(|_| promoted(vk::API_VERSION_1_2, ext::descriptor_indexing::NAME));
        if let Some(ref descriptor_indexing) = descriptor_indexing {
            // We use update-after-bind descriptors for all bind groups containing binding arrays.
            //
            // In those bind groups, we allow all binding types except uniform buffers to be present.
            //
            // As we can only switch between update-after-bind and not on a per bind group basis,
            // all supported binding types need to be able to be marked update after bind.
            //
            // As such, we enable all features as a whole, rather individually.
            let supports_descriptor_indexing =
                // Sampled Images
                descriptor_indexing.shader_sampled_image_array_non_uniform_indexing != 0
                    && descriptor_indexing.descriptor_binding_sampled_image_update_after_bind != 0
                    // Storage Images
                    && descriptor_indexing.shader_storage_image_array_non_uniform_indexing != 0
                    && descriptor_indexing.descriptor_binding_storage_image_update_after_bind != 0
                    // Storage Buffers
                    && descriptor_indexing.shader_storage_buffer_array_non_uniform_indexing != 0
                    && descriptor_indexing.descriptor_binding_storage_buffer_update_after_bind != 0;

            features.set(
                F::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
                    | F::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING,
                supports_descriptor_indexing,
            );
            // Texture and storage resource arrays also need the dynamic indexing of the
            // descriptors in them. Buffer arrays do not need uniform buffers' (it is enabled where
            // offered), so that no uniform or dynamic-offset buffer array that wgpu-core accepted
            // is refused (FORK.md, residual behaviour).
            features.set(F::BUFFER_BINDING_ARRAY, supports_descriptor_indexing);
            features.set(
                F::TEXTURE_BINDING_ARRAY,
                supports_descriptor_indexing
                    && self.core.shader_sampled_image_array_dynamic_indexing != 0,
            );
            features.set(
                F::STORAGE_RESOURCE_BINDING_ARRAY,
                supports_descriptor_indexing
                    && self.core.shader_storage_buffer_array_dynamic_indexing != 0
                    && self.core.shader_storage_image_array_dynamic_indexing != 0,
            );

            let supports_partially_bound =
                descriptor_indexing.descriptor_binding_partially_bound != 0;

            features.set(F::PARTIALLY_BOUND_BINDING_ARRAY, supports_partially_bound);
        }

        features.set(F::DEPTH_CLIP_CONTROL, self.core.depth_clamp != 0);
        features.set(F::DUAL_SOURCE_BLENDING, self.core.dual_src_blend != 0);
        features.set(F::CLIP_DISTANCES, self.core.shader_clip_distance != 0);

        let multiview = promoted(vk::API_VERSION_1_1, khr::multiview::NAME)
            && self
                .multiview
                .is_some_and(|multiview| multiview.multiview != 0);
        features.set(F::MULTIVIEW | F::SELECTIVE_MULTIVIEW, multiview);

        features.set(F::TEXTURE_FORMAT_16BIT_NORM, formats.norm16);

        features.set(
            F::TEXTURE_COMPRESSION_ASTC_HDR,
            promoted(vk::API_VERSION_1_3, ext::texture_compression_astc_hdr::NAME)
                && self
                    .astc_hdr
                    .is_some_and(|astc_hdr| astc_hdr.texture_compression_astc_hdr != 0),
        );

        // Note `storage_input_output16` is not required, we polyfill `f16` I/O using `f32`
        // types when this capability is not available
        features.set(
            F::SHADER_F16,
            promoted(vk::API_VERSION_1_2, khr::shader_float16_int8::NAME)
                && self
                    .shader_float16_int8
                    .is_some_and(|f16_i8| f16_i8.shader_float16 != 0)
                && storage16,
        );

        // Subgroup operations need `subgroupSizeControl`: the pipelines that use them allow a
        // varying subgroup size (VUID-VkPipelineShaderStageCreateInfo-flags-02784).
        let subgroup_size_control = promoted(vk::API_VERSION_1_3, ext::subgroup_size_control::NAME)
            && self
                .subgroup_size_control
                .is_some_and(|control| control.subgroup_size_control != 0);
        if let Some(ref subgroup) = caps.subgroup {
            if subgroup_size_control
                && subgroup.supported_operations.contains(
                    vk::SubgroupFeatureFlags::BASIC
                        | vk::SubgroupFeatureFlags::VOTE
                        | vk::SubgroupFeatureFlags::ARITHMETIC
                        | vk::SubgroupFeatureFlags::BALLOT
                        | vk::SubgroupFeatureFlags::SHUFFLE
                        | vk::SubgroupFeatureFlags::SHUFFLE_RELATIVE
                        | vk::SubgroupFeatureFlags::QUAD,
                )
            {
                features.set(
                    F::SUBGROUP,
                    subgroup
                        .supported_stages
                        .contains(vk::ShaderStageFlags::COMPUTE | vk::ShaderStageFlags::FRAGMENT),
                );
                features.set(
                    F::SUBGROUP_VERTEX,
                    subgroup
                        .supported_stages
                        .contains(vk::ShaderStageFlags::VERTEX),
                );
                features.insert(F::SUBGROUP_BARRIER);
            }
        }

        let stencil8 = formats.s8 || formats.d24_s8;
        let depth24_plus_stencil8 = formats.d24_s8 || formats.d32_s8;

        dl_flags.set(
            Df::WEBGPU_TEXTURE_FORMAT_SUPPORT,
            stencil8 && depth24_plus_stencil8 && formats.d32,
        );

        features.set(F::DEPTH32FLOAT_STENCIL8, formats.d32_s8);

        let supports_acceleration_structures = has_extension(khr::deferred_host_operations::NAME)
            && has_extension(khr::acceleration_structure::NAME)
            && has_extension(khr::buffer_device_address::NAME)
            && self
                .acceleration_structure
                .is_some_and(|features| features.acceleration_structure != 0)
            && self
                .buffer_device_address
                .is_some_and(|features| features.buffer_device_address != 0);

        let supports_ray_query = supports_acceleration_structures
            && has_extension(khr::ray_query::NAME)
            && self
                .ray_query
                .is_some_and(|features| features.ray_query != 0);
        let supports_acceleration_structure_binding_array = supports_ray_query
            && self
                .acceleration_structure
                .as_ref()
                .is_some_and(|features| {
                    features.descriptor_binding_acceleration_structure_update_after_bind != 0
                });

        features.set(F::EXPERIMENTAL_RAY_QUERY, supports_ray_query);
        // Vertex formats of acceleration structures, which need acceleration structures alone.
        features.set(
            F::EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS,
            supports_acceleration_structures,
        );
        // `VK_KHR_ray_tracing_position_fetch` requires `VK_KHR_acceleration_structure`.
        features.set(
            F::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN,
            supports_acceleration_structures
                && has_extension(khr::ray_tracing_position_fetch::NAME)
                && self
                    .position_fetch
                    .is_some_and(|fetch| fetch.ray_tracing_position_fetch != 0),
        );

        // Binding arrays of TLAS are supported on Vulkan when ray queries are supported.
        //
        // Note: this flag is used for shader-side `binding_array<acceleration_structure>` as well as
        // allowing `BindGroupLayoutEntry::count = Some(...)` for `BindingType::AccelerationStructure`.
        features.set(
            F::ACCELERATION_STRUCTURE_BINDING_ARRAY,
            supports_acceleration_structure_binding_array,
        );

        if supports_acceleration_structures
            && has_extension(khr::ray_tracing_pipeline::NAME)
            && promoted(vk::API_VERSION_1_2, khr::spirv_1_4::NAME)
            && self
                .ray_tracing_pipeline
                .is_some_and(|features| features.ray_tracing_pipeline != 0)
        {
            features.insert(F::EXPERIMENTAL_RAY_TRACING_PIPELINES);
        }

        features.set(
            F::RG11B10UFLOAT_RENDERABLE,
            formats.rg11b10ufloat_renderable,
        );

        features.set(F::BGRA8UNORM_STORAGE, formats.bgra8unorm_storage);

        features.set(F::FLOAT32_FILTERABLE, formats.float32_filterable);

        features.set(F::FLOAT32_BLENDABLE, formats.float32_blendable);

        // The two-plane formats come with `VK_KHR_sampler_ycbcr_conversion`, promoted to 1.1. Their
        // images, the views of their planes, and copies need no `samplerYcbcrConversion`, which
        // only conversion objects do (VUID-vkCreateSamplerYcbcrConversion-None-01648).
        let two_plane_formats = promoted(vk::API_VERSION_1_1, khr::sampler_ycbcr_conversion::NAME)
            && !caps.is_driver(vk::DriverId::MOLTENVK);
        features.set(F::TEXTURE_FORMAT_NV12, two_plane_formats && formats.nv12);
        features.set(F::TEXTURE_FORMAT_P010, two_plane_formats && formats.p010);

        // Mesh and task shader stages need their features (VUID-VkPipelineShaderStageCreateInfo-
        // stage-02091, -02092).
        // `VK_EXT_mesh_shader` and `VK_KHR_ray_tracing_pipeline` require `VK_KHR_spirv_1_4`
        // before Vulkan 1.2 (VUID-vkCreateDevice-ppEnabledExtensionNames-01387).
        let spirv_1_4 = promoted(vk::API_VERSION_1_2, khr::spirv_1_4::NAME);
        let mesh_shader = has_extension(ext::mesh_shader::NAME)
            && spirv_1_4
            && self
                .mesh_shader
                .is_some_and(|mesh| mesh.mesh_shader != 0 && mesh.task_shader != 0);
        features.set(
            F::EXPERIMENTAL_MESH_SHADER | F::EXPERIMENTAL_MESH_SHADER_POINTS,
            mesh_shader,
        );
        features.set(
            F::EXPERIMENTAL_MESH_SHADER_MULTIVIEW,
            mesh_shader
                && multiview
                && self
                    .mesh_shader
                    .is_some_and(|mesh| mesh.multiview_mesh_shader != 0),
        );

        // Not supported by default by `VK_KHR_portability_subset`, which we use on apple platforms:
        // a device with the extension has these only with its features.
        let portability = has_extension(khr::portability_subset::NAME);
        features.set(
            F::TEXTURE_COMPONENT_SWIZZLE,
            !portability
                || self
                    .portability_subset
                    .is_some_and(|p| p.image_view_format_swizzle == vk::TRUE),
        );
        features.set(
            F::MULTISAMPLE_ARRAY,
            !portability
                || self
                    .portability_subset
                    .is_some_and(|p| p.multisample_array_image == vk::TRUE),
        );

        // Enable cooperative matrix if any configuration is supported. The SPIR-V
        // we emit for it uses `Device`-scope atomics under the Vulkan memory model,
        // so the device must also support `vulkanMemoryModelDeviceScope`, otherwise
        // those shaders trip `VUID-RuntimeSpirv-vulkanMemoryModel-06265`.
        features.set(
            F::EXPERIMENTAL_COOPERATIVE_MATRIX,
            !caps.cooperative_matrix_properties.is_empty()
                && has_extension(khr::cooperative_matrix::NAME)
                && self
                    .cooperative_matrix
                    .is_some_and(|matrix| matrix.cooperative_matrix == vk::TRUE)
                && promoted(vk::API_VERSION_1_2, khr::vulkan_memory_model::NAME)
                && self.vulkan_memory_model.is_some_and(|m| {
                    m.vulkan_memory_model == vk::TRUE
                        && m.vulkan_memory_model_device_scope == vk::TRUE
                }),
        );

        features.set(
            F::SHADER_DRAW_INDEX,
            self.shader_draw_parameters
                .is_some_and(|a| a.shader_draw_parameters != 0)
                || has_extension(c"VK_KHR_shader_draw_parameters"),
        );

        features.set(
            F::DEBUG_PRINTF,
            caps.device_api_version >= vk::API_VERSION_1_3
                || has_extension(khr::shader_non_semantic_info::NAME),
        );

        (features, dl_flags)
    }
}

/// The support of a physical device for the formats on which wgpu features and downlevel flags
/// depend ([`PhysicalDeviceFeatures::to_wgpu`]). They are properties of the physical device, which
/// do not depend on the features or extensions that a device of it enables.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FormatSupport {
    /// `S8_UINT` is a depth-stencil attachment and samples.
    s8: bool,
    /// `D32_SFLOAT` is a depth-stencil attachment and samples.
    d32: bool,
    /// `D24_UNORM_S8_UINT` is a depth-stencil attachment and samples.
    d24_s8: bool,
    /// `D32_SFLOAT_S8_UINT` is a depth-stencil attachment and samples.
    d32_s8: bool,
    /// `B10G11R11_UFLOAT_PACK32` is a blendable color attachment.
    rg11b10ufloat_renderable: bool,
    /// `B8G8R8A8_UNORM` is a storage image that shaders write without a format.
    bgra8unorm_storage: bool,
    /// The 32-bit float formats filter linearly.
    float32_filterable: bool,
    /// The 32-bit float formats blend.
    float32_blendable: bool,
    /// The 16-bit normalized formats ([`is_format_16bit_norm_supported`]).
    norm16: bool,
    /// The physical device offers `textureCompressionBC`, and every BC format has 3D images.
    bc_3d: bool,
    /// The physical device offers `textureCompressionASTC_LDR`, and every ASTC LDR format has 3D
    /// images.
    astc_3d: bool,
    /// `G8_B8R8_2PLANE_420_UNORM` samples and is copied.
    nv12: bool,
    /// `G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16` samples and is copied.
    p010: bool,
}

impl FormatSupport {
    /// Queries the format support of `phd`, a physical device of `instance` that `caps`
    /// describes, and that offers `offered`.
    fn query(
        instance: &ash::Instance,
        phd: vk::PhysicalDevice,
        caps: &PhysicalDeviceProperties,
        offered: &PhysicalDeviceFeatures,
    ) -> Self {
        let depth_format = |format| {
            supports_format(
                instance,
                phd,
                format,
                vk::ImageTiling::OPTIMAL,
                depth_stencil_required_flags(),
            )
        };
        let planar_format = |format| {
            supports_format(
                instance,
                phd,
                format,
                vk::ImageTiling::OPTIMAL,
                vk::FormatFeatureFlags::SAMPLED_IMAGE
                    | vk::FormatFeatureFlags::TRANSFER_SRC
                    | vk::FormatFeatureFlags::TRANSFER_DST,
            )
        };
        Self {
            s8: depth_format(vk::Format::S8_UINT),
            d32: depth_format(vk::Format::D32_SFLOAT),
            d24_s8: depth_format(vk::Format::D24_UNORM_S8_UINT),
            d32_s8: depth_format(vk::Format::D32_SFLOAT_S8_UINT),
            rg11b10ufloat_renderable: supports_format(
                instance,
                phd,
                vk::Format::B10G11R11_UFLOAT_PACK32,
                vk::ImageTiling::OPTIMAL,
                vk::FormatFeatureFlags::COLOR_ATTACHMENT
                    | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND,
            ),
            bgra8unorm_storage: supports_bgra8unorm_storage(instance, phd, caps.device_api_version),
            float32_filterable: is_float32_filterable_supported(instance, phd),
            float32_blendable: is_float32_blendable_supported(instance, phd),
            norm16: is_format_16bit_norm_supported(instance, phd),
            bc_3d: offered.core.texture_compression_bc != 0
                && supports_3d_images(instance, phd, &BC_FORMATS),
            astc_3d: offered.core.texture_compression_astc_ldr != 0
                && supports_3d_images(instance, phd, &ASTC_LDR_FORMATS),
            nv12: planar_format(vk::Format::G8_B8R8_2PLANE_420_UNORM),
            p010: planar_format(vk::Format::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16),
        }
    }
}

/// Vulkan "properties" structures gathered about a physical device.
///
/// This structure holds the properties of a [`vk::PhysicalDevice`]:
/// - the standard Vulkan device properties
/// - the `VkExtensionProperties` structs for all available extensions, and
/// - the per-extension properties structures for the available extensions that
///   `wgpu` cares about.
///
/// Generally, if you get it from any of these functions, it's stored
/// here:
/// - `vkEnumerateDeviceExtensionProperties`
/// - `vkGetPhysicalDeviceProperties`
/// - `vkGetPhysicalDeviceProperties2`
///
/// This also includes a copy of the device API version, since we can
/// use that as a shortcut for searching for an extension, if the
/// extension has been promoted to core in the current version.
///
/// This does not include device features; for those, see
/// [`PhysicalDeviceFeatures`].
#[derive(Default, Debug)]
pub struct PhysicalDeviceProperties {
    /// Extensions supported by the `vk::PhysicalDevice`,
    /// as returned by `vkEnumerateDeviceExtensionProperties`.
    supported_extensions: Vec<vk::ExtensionProperties>,

    /// Properties of the `vk::PhysicalDevice`, as returned by
    /// `vkGetPhysicalDeviceProperties`.
    properties: vk::PhysicalDeviceProperties,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_maintenance3` extension, promoted to Vulkan 1.1.
    maintenance_3: Option<vk::PhysicalDeviceMaintenance3Properties<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_maintenance4` extension, promoted to Vulkan 1.3.
    maintenance_4: Option<vk::PhysicalDeviceMaintenance4Properties<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_maintenance5` extension, promoted to Vulkan 1.4.
    maintenance_5: Option<vk::PhysicalDeviceMaintenance5PropertiesKHR<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_EXT_descriptor_indexing` extension, promoted to Vulkan 1.2.
    descriptor_indexing: Option<vk::PhysicalDeviceDescriptorIndexingPropertiesEXT<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_acceleration_structure` extension.
    acceleration_structure: Option<vk::PhysicalDeviceAccelerationStructurePropertiesKHR<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_ray_tracing_pipeline` extension.
    ray_tracing_pipeline: Option<vk::PhysicalDeviceRayTracingPipelinePropertiesKHR<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_driver_properties` extension, promoted to Vulkan 1.2.
    driver: Option<vk::PhysicalDeviceDriverPropertiesKHR<'static>>,

    /// Additional `vk::PhysicalDevice` properties from Vulkan 1.1.
    subgroup: Option<vk::PhysicalDeviceSubgroupProperties<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_EXT_subgroup_size_control` extension, promoted to Vulkan 1.3.
    subgroup_size_control: Option<vk::PhysicalDeviceSubgroupSizeControlProperties<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_EXT_robustness2` extension.
    robustness2: Option<vk::PhysicalDeviceRobustness2PropertiesEXT<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_EXT_mesh_shader` extension.
    mesh_shader: Option<vk::PhysicalDeviceMeshShaderPropertiesEXT<'static>>,

    /// Additional `vk::PhysicalDevice` properties from the
    /// `VK_KHR_multiview` extension.
    multiview: Option<vk::PhysicalDeviceMultiviewPropertiesKHR<'static>>,

    /// `VK_EXT_pci_bus_info` extension.
    pci_bus_info: Option<vk::PhysicalDevicePCIBusInfoPropertiesEXT<'static>>,

    /// The device API version.
    ///
    /// Which is the version of Vulkan supported for device-level functionality.
    ///
    /// It is associated with a `VkPhysicalDevice` and its children.
    pub(super) device_api_version: u32,

    /// Supported cooperative matrix configurations.
    ///
    /// This is determined by querying `vkGetPhysicalDeviceCooperativeMatrixPropertiesKHR`.
    cooperative_matrix_properties: Vec<wgt::CooperativeMatrixProperties>,
}

impl PhysicalDeviceProperties {
    pub fn properties(&self) -> vk::PhysicalDeviceProperties {
        self.properties
    }

    pub fn supports_extension(&self, extension: &CStr) -> bool {
        self.supported_extensions
            .iter()
            .any(|ep| ep.extension_name_as_c_str() == Ok(extension))
    }

    pub fn is_driver(&self, id: vk::DriverId) -> bool {
        self.driver.is_some_and(|driver| driver.driver_id == id)
    }

    /// Drops the extensions that the device lists but that a device of an instance with
    /// `instance_extensions` cannot enable. A device extension that depends on an instance
    /// extension that the instance has not enabled is unsupported, whatever the device lists (the
    /// specification's "Extension Dependencies"). The swapchain maintenance extensions also depend
    /// on `VK_KHR_get_physical_device_properties2` (or Vulkan 1.1), through which wgpu-hal queries
    /// and enables their feature: `gpdp2` says whether the instance has its functions.
    fn drop_extensions_unusable_with(
        &mut self,
        instance_extensions: &[&'static CStr],
        gpdp2: bool,
    ) {
        self.supported_extensions.retain(|extension| {
            SWAPCHAIN_MAINTENANCE1
                .iter()
                .all(|&(device_extension, instance_extension)| {
                    extension.extension_name_as_c_str() != Ok(device_extension)
                        || (gpdp2 && instance_extensions.contains(&instance_extension))
                })
        });
    }

    /// The extension with which presents can signal fences that the device supports:
    /// `VK_KHR_swapchain_maintenance1`, else `VK_EXT_swapchain_maintenance1`.
    fn swapchain_maintenance1(&self) -> Option<&'static CStr> {
        SWAPCHAIN_MAINTENANCE1
            .iter()
            .map(|&(extension, _)| extension)
            .find(|&extension| self.supports_extension(extension))
    }

    /// Map `requested_features` to the list of Vulkan extension strings required to create the logical device.
    fn get_required_extensions(&self, requested_features: wgt::Features) -> Vec<&'static CStr> {
        let mut extensions = Vec::new();

        // Note that quite a few extensions depend on the `VK_KHR_get_physical_device_properties2` instance extension.
        // We enable `VK_KHR_get_physical_device_properties2` unconditionally (if available).

        // Require `VK_KHR_swapchain`
        extensions.push(khr::swapchain::NAME);

        if self.device_api_version < vk::API_VERSION_1_1 {
            // Require `VK_KHR_maintenance1`
            extensions.push(khr::maintenance1::NAME);

            // Optional `VK_KHR_maintenance2`
            if self.supports_extension(khr::maintenance2::NAME) {
                extensions.push(khr::maintenance2::NAME);
            }

            // Optional `VK_KHR_maintenance3`
            if self.supports_extension(khr::maintenance3::NAME) {
                extensions.push(khr::maintenance3::NAME);
            }

            // Require `VK_KHR_storage_buffer_storage_class`
            extensions.push(khr::storage_buffer_storage_class::NAME);

            // Require `VK_KHR_multiview` if a feature that uses it was requested
            if requested_features.intersects(MULTIVIEW_FEATURES) {
                extensions.push(khr::multiview::NAME);
            }

            // Require `VK_KHR_sampler_ycbcr_conversion`, which adds the two-plane formats, if a
            // feature that uses them was requested: a device may use an enumerant that an
            // extension adds only with the extension.
            if requested_features
                .intersects(wgt::Features::TEXTURE_FORMAT_NV12 | wgt::Features::TEXTURE_FORMAT_P010)
            {
                extensions.push(khr::sampler_ycbcr_conversion::NAME);
            }

            // Require `VK_KHR_16bit_storage` if `SHADER_F16` or `SHADER_I16` was requested
            if requested_features.intersects(wgt::Features::SHADER_F16 | wgt::Features::SHADER_I16)
            {
                // - Feature `SHADER_F16` also requires `VK_KHR_shader_float16_int8`, but we always
                //   require that anyway (if it is available) below.
                // - `VK_KHR_16bit_storage` requires `VK_KHR_storage_buffer_storage_class`, however
                //   we require that one already.
                extensions.push(khr::_16bit_storage::NAME);
            }

            if requested_features.contains(wgt::Features::SHADER_DRAW_INDEX) {
                extensions.push(khr::shader_draw_parameters::NAME);
            }
        }

        if self.device_api_version < vk::API_VERSION_1_2 {
            // Optional `VK_KHR_image_format_list`
            if self.supports_extension(khr::image_format_list::NAME) {
                extensions.push(khr::image_format_list::NAME);
            }

            // Optional `VK_KHR_driver_properties`
            if self.supports_extension(khr::driver_properties::NAME) {
                extensions.push(khr::driver_properties::NAME);
            }

            // Optional `VK_KHR_timeline_semaphore`
            if self.supports_extension(khr::timeline_semaphore::NAME) {
                extensions.push(khr::timeline_semaphore::NAME);
            }

            // Require `VK_EXT_descriptor_indexing` if one of the associated features was requested
            if requested_features.intersects(INDEXING_FEATURES) {
                extensions.push(ext::descriptor_indexing::NAME);
            }

            // Always require `VK_KHR_shader_float16_int8` if available as it enables
            // Int8 optimizations. Also require it even if it's not available but
            // requested so that we get a corresponding error message.
            if requested_features.contains(wgt::Features::SHADER_F16)
                || self.supports_extension(khr::shader_float16_int8::NAME)
            {
                extensions.push(khr::shader_float16_int8::NAME);
            }

            // `SPV_EXT_mesh_shader` and `SPV_KHR_ray_tracing` both require SPIR-V 1.4.
            if requested_features.intersects(
                MESH_SHADER_FEATURES | wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES,
            ) {
                extensions.push(khr::spirv_1_4::NAME);
            }

            // Require `VK_KHR_vulkan_memory_model` if cooperative matrices were requested: their
            // SPIR-V uses the Vulkan memory model, whose features structure a device created
            // before Vulkan 1.2 may have only with the extension.
            if requested_features.contains(wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX) {
                extensions.push(khr::vulkan_memory_model::NAME);
            }

            //extensions.push(khr::sampler_mirror_clamp_to_edge::NAME);
            //extensions.push(ext::sampler_filter_minmax::NAME);
        }

        if self.device_api_version < vk::API_VERSION_1_3 {
            // Optional `VK_KHR_maintenance4`
            if self.supports_extension(khr::maintenance4::NAME) {
                extensions.push(khr::maintenance4::NAME);
            }

            // Optional `VK_EXT_image_robustness`
            if self.supports_extension(ext::image_robustness::NAME) {
                extensions.push(ext::image_robustness::NAME);
            }

            // Require `VK_EXT_subgroup_size_control` if a subgroup feature was requested
            if requested_features.intersects(SUBGROUP_FEATURES) {
                extensions.push(ext::subgroup_size_control::NAME);
            }

            // Optional `VK_KHR_shader_integer_dot_product`
            if self.supports_extension(khr::shader_integer_dot_product::NAME) {
                extensions.push(khr::shader_integer_dot_product::NAME);
            }

            // Optional `VK_KHR_zero_initialize_workgroup_memory`: where the adapter offers its
            // feature, the device enables it, and Naga zero-initializes workgroup memory natively.
            if self.supports_extension(khr::zero_initialize_workgroup_memory::NAME) {
                extensions.push(khr::zero_initialize_workgroup_memory::NAME);
            }

            // Optional `VK_KHR_dynamic_rendering`.
            // Depends on:
            // - `VK_KHR_get_physical_device_properties2` or Vulkan 1.1, and `VK_KHR_depth_stencil_resolve`
            // - or Vulkan 1.2
            //
            // We only check Vulkan 1.2 for now, as `VK_KHR_depth_stencil_resolve`
            // also depends a bunch of extensions.
            if self.device_api_version >= vk::API_VERSION_1_2
                && self.supports_extension(khr::dynamic_rendering::NAME)
            {
                extensions.push(khr::dynamic_rendering::NAME);
            }

            // Optional `VK_KHR_load_store_op_none`
            if self.supports_extension(khr::load_store_op_none::NAME) {
                extensions.push(khr::load_store_op_none::NAME);
            }

            // Optional `VK_QCOM_render_pass_store_ops`
            if self.supports_extension(ash::qcom::render_pass_store_ops::NAME) {
                extensions.push(ash::qcom::render_pass_store_ops::NAME);
            }

            // Optional `VK_EXT_load_store_op_none`
            if self.supports_extension(ext::load_store_op_none::NAME) {
                extensions.push(ext::load_store_op_none::NAME);
            }

            // Require `VK_KHR_shader_non_semantic_info` if the associated feature was requested
            if requested_features.contains(wgt::Features::DEBUG_PRINTF) {
                extensions.push(khr::shader_non_semantic_info::NAME);
            }
        }

        // Optional `VK_KHR_swapchain_mutable_format`
        if self.supports_extension(khr::swapchain_mutable_format::NAME) {
            extensions.push(khr::swapchain_mutable_format::NAME);
        }

        // Optional `VK_KHR_swapchain_maintenance1`, else `VK_EXT_swapchain_maintenance1`: presents
        // signal fences, on which a swapchain's teardown waits.
        if let Some(extension) = self.swapchain_maintenance1() {
            extensions.push(extension);
        }

        // Optional `VK_EXT_robustness2`
        if self.supports_extension(ext::robustness2::NAME) {
            extensions.push(ext::robustness2::NAME);
        }

        // Optional `VK_KHR_external_memory_win32`
        if self.supports_extension(khr::external_memory_win32::NAME) {
            extensions.push(khr::external_memory_win32::NAME);
        }

        // Optional `VK_KHR_external_memory_fd`
        if self.supports_extension(khr::external_memory_fd::NAME) {
            extensions.push(khr::external_memory_fd::NAME);
        }

        // Optional `VK_EXT_external_memory_dma`
        if self.supports_extension(ext::external_memory_dma_buf::NAME) {
            extensions.push(ext::external_memory_dma_buf::NAME);
        }

        // Optional `VK_EXT_image_drm_format_modifier`
        if self.supports_extension(ext::image_drm_format_modifier::NAME) {
            extensions.push(ext::image_drm_format_modifier::NAME);
        }

        // Optional `VK_GOOGLE_display_timing`
        if self.supports_extension(google::display_timing::NAME) {
            extensions.push(google::display_timing::NAME);
        }

        // Optional `VK_EXT_memory_budget`
        if self.supports_extension(ext::memory_budget::NAME) {
            extensions.push(ext::memory_budget::NAME);
        } else {
            log::debug!("VK_EXT_memory_budget is not available.")
        }

        // Require `VK_KHR_draw_indirect_count` if the associated feature was requested
        // Even though Vulkan 1.2 has promoted the extension to core, we must require the extension to avoid
        // large amounts of spaghetti involved with using PhysicalDeviceVulkan12Features.
        if requested_features.contains(wgt::Features::MULTI_DRAW_INDIRECT_COUNT) {
            extensions.push(khr::draw_indirect_count::NAME);
        }

        // Require `VK_KHR_deferred_host_operations`, `VK_KHR_acceleration_structure` and
        // `VK_KHR_buffer_device_address` if a feature that uses acceleration structures was
        // requested. `VK_KHR_ray_tracing_position_fetch` requires `VK_KHR_acceleration_structure`
        // (VUID-vkCreateDevice-ppEnabledExtensionNames-01387).
        if requested_features.intersects(ACCELERATION_STRUCTURE_FEATURES) {
            extensions.push(khr::deferred_host_operations::NAME);
            extensions.push(khr::acceleration_structure::NAME);
            extensions.push(khr::buffer_device_address::NAME);
        }

        // Require `VK_KHR_ray_query` if a feature that uses ray queries was requested
        if requested_features.intersects(RAY_QUERY_FEATURES) {
            extensions.push(khr::ray_query::NAME);
        }

        // Require `VK_KHR_ray_tracing_pipeline` if `EXPERIMENTAL_RAY_TRACING_PIPELINES` was requested
        if requested_features.contains(wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES) {
            extensions.push(khr::ray_tracing_pipeline::NAME);
        }

        if requested_features.contains(wgt::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN) {
            extensions.push(khr::ray_tracing_position_fetch::NAME)
        }

        // Require `VK_EXT_conservative_rasterization` if the associated feature was requested
        if requested_features.contains(wgt::Features::CONSERVATIVE_RASTERIZATION) {
            extensions.push(ext::conservative_rasterization::NAME);
        }

        // Require `VK_KHR_portability_subset` on macOS/iOS
        #[cfg(target_vendor = "apple")]
        extensions.push(khr::portability_subset::NAME);

        // Require `VK_EXT_texture_compression_astc_hdr` if the associated feature was requested,
        // before Vulkan 1.3, which has it in core.
        if requested_features.contains(wgt::Features::TEXTURE_COMPRESSION_ASTC_HDR)
            && self.device_api_version < vk::API_VERSION_1_3
        {
            extensions.push(ext::texture_compression_astc_hdr::NAME);
        }

        // Require `VK_KHR_shader_atomic_int64` if the associated feature was requested
        if requested_features.intersects(
            wgt::Features::SHADER_INT64_ATOMIC_ALL_OPS | wgt::Features::SHADER_INT64_ATOMIC_MIN_MAX,
        ) {
            extensions.push(khr::shader_atomic_int64::NAME);
        }

        // Require `VK_EXT_shader_image_atomic_int64` if the associated feature was requested
        if requested_features.intersects(wgt::Features::TEXTURE_INT64_ATOMIC) {
            extensions.push(ext::shader_image_atomic_int64::NAME);
        }

        // Require `VK_EXT_shader_atomic_float` if the associated feature was requested
        if requested_features.contains(wgt::Features::SHADER_FLOAT32_ATOMIC) {
            extensions.push(ext::shader_atomic_float::NAME);
        }

        if requested_features.intersects(MESH_SHADER_FEATURES) {
            extensions.push(ext::mesh_shader::NAME);
        }

        // Require `VK_KHR_fragment_shader_barycentric` if an associated feature was requested
        // Vulkan bundles both barycentrics and per-vertex attributes under the same feature.
        if requested_features
            .intersects(wgt::Features::SHADER_BARYCENTRICS | wgt::Features::SHADER_PER_VERTEX)
        {
            extensions.push(khr::fragment_shader_barycentric::NAME);
        }

        // Require `VK_KHR_cooperative_matrix` if the associated feature was requested
        if requested_features.contains(wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX) {
            extensions.push(khr::cooperative_matrix::NAME);
        }

        extensions
    }

    fn to_wgpu_limits(&self) -> wgt::Limits {
        let limits = &self.properties.limits;

        // Default is only implemented for tuples up to a certain size.
        let (
            mut max_task_workgroup_total_count,
            mut max_task_workgroups_per_dimension,
            mut max_mesh_workgroup_total_count,
            mut max_mesh_workgroups_per_dimension,
        ) = Default::default();
        let (
            mut max_task_invocations_per_workgroup,
            mut max_task_invocations_per_dimension,
            mut max_mesh_invocations_per_workgroup,
            mut max_mesh_invocations_per_dimension,
            mut max_task_payload_size,
            mut max_mesh_output_vertices,
            mut max_mesh_output_primitives,
            mut max_mesh_output_layers,
            mut max_mesh_multiview_view_count,
        ) = Default::default();
        if let Some(m) = self.mesh_shader {
            max_task_workgroup_total_count = m.max_task_work_group_total_count;
            max_task_workgroups_per_dimension =
                m.max_task_work_group_count.into_iter().min().unwrap();
            max_mesh_workgroup_total_count = m.max_mesh_work_group_total_count;
            max_mesh_workgroups_per_dimension =
                m.max_mesh_work_group_count.into_iter().min().unwrap();
            max_task_invocations_per_workgroup = m.max_task_work_group_invocations;
            max_task_invocations_per_dimension =
                m.max_task_work_group_size.into_iter().min().unwrap();
            max_mesh_invocations_per_workgroup = m.max_mesh_work_group_invocations;
            max_mesh_invocations_per_dimension =
                m.max_mesh_work_group_size.into_iter().min().unwrap();
            max_task_payload_size = m.max_task_payload_size;
            max_mesh_output_vertices = m.max_mesh_output_vertices;
            max_mesh_output_primitives = m.max_mesh_output_primitives;
            max_mesh_output_layers = m.max_mesh_output_layers;
            max_mesh_multiview_view_count = m.max_mesh_multiview_view_count;
        }

        let max_memory_allocation_size = self
            .maintenance_3
            .map(|maintenance_3| maintenance_3.max_memory_allocation_size)
            .unwrap_or(u64::MAX);
        let max_buffer_size = self
            .maintenance_4
            .map(|maintenance_4| maintenance_4.max_buffer_size)
            .unwrap_or(u64::MAX);
        let max_buffer_size = max_buffer_size.min(max_memory_allocation_size);

        // Prevent very large buffers on mesa and most android devices, and in all cases
        // don't risk confusing JS by exceeding the range of a double.
        let is_nvidia = self.properties.vendor_id == crate::auxil::db::nvidia::VENDOR;
        let max_buffer_size_cap =
            if (cfg!(target_os = "linux") || cfg!(target_os = "android")) && !is_nvidia {
                i32::MAX as u64
            } else {
                1u64 << 52
            };

        let max_buffer_size = max_buffer_size.min(max_buffer_size_cap);

        let mut max_binding_array_elements = 0;
        let mut max_sampler_binding_array_elements = 0;
        if let Some(ref descriptor_indexing) = self.descriptor_indexing {
            max_binding_array_elements = descriptor_indexing
                .max_descriptor_set_update_after_bind_sampled_images
                .min(descriptor_indexing.max_descriptor_set_update_after_bind_storage_images)
                .min(descriptor_indexing.max_descriptor_set_update_after_bind_storage_buffers)
                .min(descriptor_indexing.max_per_stage_descriptor_update_after_bind_sampled_images)
                .min(descriptor_indexing.max_per_stage_descriptor_update_after_bind_storage_images)
                .min(
                    descriptor_indexing.max_per_stage_descriptor_update_after_bind_storage_buffers,
                );

            max_sampler_binding_array_elements = descriptor_indexing
                .max_descriptor_set_update_after_bind_samplers
                .min(descriptor_indexing.max_per_stage_descriptor_update_after_bind_samplers);
        }

        const MAX_SHADER_STAGES_PER_PIPELINE: u32 = 2;

        // When summed, the 3 limits below must be under Vulkan's maxFragmentCombinedOutputResources.
        // https://gpuweb.github.io/gpuweb/correspondence/#vulkan-maxFragmentCombinedOutputResources
        //
        // - maxStorageTexturesPerShaderStage, WebGPU default: 4
        // - maxStorageBuffersPerShaderStage, WebGPU default: 8
        // - maxColorAttachments, WebGPU default: 8
        //
        // However, maxFragmentCombinedOutputResources should be ignored on
        // intel/nvidia/amd/imgtec since it's not reported correctly.
        //
        // https://github.com/gpuweb/gpuweb/issues/3631#issuecomment-1498747606
        // https://github.com/gpuweb/gpuweb/issues/4018
        let mut max_storage_textures_per_shader_stage = limits
            .max_per_stage_descriptor_storage_images
            .min(limits.max_descriptor_set_storage_images / MAX_SHADER_STAGES_PER_PIPELINE);
        let mut max_storage_buffers_per_shader_stage = limits
            .max_per_stage_descriptor_storage_buffers
            .min(limits.max_descriptor_set_storage_buffers / MAX_SHADER_STAGES_PER_PIPELINE);
        let mut max_color_attachments = limits
            .max_color_attachments
            .min(limits.max_fragment_output_attachments);

        let ignore_max_fragment_combined_output_resources_by_device = [
            crate::auxil::db::intel::VENDOR,
            crate::auxil::db::nvidia::VENDOR,
            crate::auxil::db::amd::VENDOR,
            crate::auxil::db::imgtec::VENDOR,
        ]
        .contains(&self.properties.vendor_id);
        let ignore_max_fragment_combined_output_resources_by_driver =
            self.is_driver(vk::DriverId::MESA_AGXV);
        let ignore_max_fragment_combined_output_resources =
            ignore_max_fragment_combined_output_resources_by_device
                || ignore_max_fragment_combined_output_resources_by_driver;

        if !ignore_max_fragment_combined_output_resources {
            crate::auxil::cap_limits_to_be_under_the_sum_limit(
                [
                    &mut max_storage_textures_per_shader_stage,
                    &mut max_storage_buffers_per_shader_stage,
                    &mut max_color_attachments,
                ],
                limits.max_fragment_combined_output_resources,
            );
        }

        // When summed, the 5 limits below must be under Vulkan's maxPerStageResources.
        //
        // - maxUniformBuffersPerShaderStage, WebGPU default: 12
        // - maxSampledTexturesPerShaderStage, WebGPU default: 16
        // - maxStorageTexturesPerShaderStage, WebGPU default: 4
        // - maxStorageBuffersPerShaderStage, WebGPU default: 8
        // - maxColorAttachments, WebGPU default: 8
        //
        // Note: Vulkan's texel buffers and input attachments also count towards
        // maxPerStageResources but we don't make use of them.
        let mut max_sampled_textures_per_shader_stage = limits
            .max_per_stage_descriptor_sampled_images
            .min(limits.max_descriptor_set_sampled_images / MAX_SHADER_STAGES_PER_PIPELINE);
        let mut max_uniform_buffers_per_shader_stage = limits
            .max_per_stage_descriptor_uniform_buffers
            .min(limits.max_descriptor_set_uniform_buffers / MAX_SHADER_STAGES_PER_PIPELINE);

        crate::auxil::cap_limits_to_be_under_the_sum_limit(
            [
                &mut max_sampled_textures_per_shader_stage,
                &mut max_uniform_buffers_per_shader_stage,
                &mut max_storage_textures_per_shader_stage,
                &mut max_storage_buffers_per_shader_stage,
                &mut max_color_attachments,
            ],
            limits.max_per_stage_resources,
        );

        // Acceleration structure limits
        let mut max_blas_geometry_count = 0;
        let mut max_blas_primitive_count = 0;
        let mut max_tlas_instance_count = 0;
        let mut max_acceleration_structures_per_shader_stage = 0;
        if let Some(properties) = self.acceleration_structure {
            max_blas_geometry_count = properties.max_geometry_count as u32;
            max_blas_primitive_count = properties.max_primitive_count as u32;
            max_tlas_instance_count = properties.max_instance_count as u32;
            max_acceleration_structures_per_shader_stage = properties
                .max_per_stage_descriptor_acceleration_structures
                .min(
                    properties.max_descriptor_set_acceleration_structures
                        / MAX_SHADER_STAGES_PER_PIPELINE,
                );
        }

        // When summed, the 6 limits below must be under Vulkan's
        // maxPerSetDescriptors / MAX_SHADER_STAGES_PER_PIPELINE.
        //
        // - maxUniformBuffersPerShaderStage, WebGPU default: 12
        // - maxSampledTexturesPerShaderStage, WebGPU default: 16
        // - maxStorageTexturesPerShaderStage, WebGPU default: 4
        // - maxStorageBuffersPerShaderStage, WebGPU default: 8
        // - maxSamplersPerShaderStage, WebGPU default: 16
        // - maxAccelerationStructuresPerShaderStage, Native only
        //
        // Note: All Vulkan's descriptor types count towards maxPerSetDescriptors but
        // we don't use all of them.
        // See https://registry.khronos.org/vulkan/specs/latest/html/vkspec.html#interfaces-resources-limits
        let max_per_set_descriptors = self
            .maintenance_3
            .map(|maintenance_3| maintenance_3.max_per_set_descriptors)
            // The lowest value seen in reports is 312, use 256 as a safe default.
            // https://vulkan.gpuinfo.org/displayextensionproperty.php?extensionname=VK_KHR_maintenance3&extensionproperty=maxPerSetDescriptors&platform=all
            // https://vulkan.gpuinfo.org/displaycoreproperty.php?core=1.1&name=maxPerSetDescriptors&platform=all
            .unwrap_or(256);

        let mut max_samplers_per_shader_stage = limits
            .max_per_stage_descriptor_samplers
            .min(limits.max_descriptor_set_samplers / MAX_SHADER_STAGES_PER_PIPELINE);

        crate::auxil::cap_limits_to_be_under_the_sum_limit(
            [
                &mut max_sampled_textures_per_shader_stage,
                &mut max_uniform_buffers_per_shader_stage,
                &mut max_storage_textures_per_shader_stage,
                &mut max_storage_buffers_per_shader_stage,
                &mut max_samplers_per_shader_stage,
                &mut max_acceleration_structures_per_shader_stage,
            ],
            max_per_set_descriptors / MAX_SHADER_STAGES_PER_PIPELINE,
        );

        // Use max(default, maxPerSetDescriptors) since the spec requires this
        // limit to be at least 1000. This is ok because we already lowered
        // all the other relevant per stage limits so their sum is lower
        // than maxPerSetDescriptors.
        let max_bindings_per_bind_group = 1000.max(max_per_set_descriptors);

        // TODO: programmatically determine this, if possible. It's unclear whether we can
        // as of https://github.com/gpuweb/gpuweb/issues/2965#issuecomment-1361315447.
        //
        // In theory some tilers may not support this much. We can't tell however, and
        // the driver will throw a DEVICE_REMOVED if it goes too high in usage. This is fine.
        let max_color_attachment_bytes_per_sample =
            max_color_attachments * wgt::TextureFormat::MAX_TARGET_PIXEL_BYTE_COST;

        let mut max_ray_dispatch_count = 0;
        let mut max_ray_recursion_depth = 0;

        if let Some(properties) = self.ray_tracing_pipeline {
            max_ray_dispatch_count = properties.max_ray_dispatch_invocation_count;
            max_ray_recursion_depth = properties.max_ray_recursion_depth;
        }

        let max_multiview_view_count = self
            .multiview
            .map(|a| a.max_multiview_view_count.min(32))
            .unwrap_or(0);

        crate::auxil::adjust_raw_limits(wgt::Limits {
            //
            // WebGPU LIMITS:
            // Based on https://gpuweb.github.io/gpuweb/correspondence/#limits
            //
            max_texture_dimension_1d: limits.max_image_dimension1_d,
            max_texture_dimension_2d: limits
                .max_image_dimension2_d
                .min(limits.max_image_dimension_cube)
                .min(limits.max_framebuffer_width)
                .min(limits.max_framebuffer_height),
            max_texture_dimension_3d: limits.max_image_dimension3_d,
            max_texture_array_layers: limits.max_image_array_layers,
            max_bind_groups: limits.max_bound_descriptor_sets,
            // No limit.
            max_bind_groups_plus_vertex_buffers: u32::MAX,
            max_bindings_per_bind_group,
            max_dynamic_uniform_buffers_per_pipeline_layout: limits
                .max_descriptor_set_uniform_buffers_dynamic,
            max_dynamic_storage_buffers_per_pipeline_layout: limits
                .max_descriptor_set_storage_buffers_dynamic,
            max_samplers_per_shader_stage,
            max_sampled_textures_per_shader_stage,
            max_storage_buffers_per_shader_stage,
            max_storage_buffers_in_vertex_stage: 0,
            max_storage_buffers_in_fragment_stage: 0,
            max_storage_textures_per_shader_stage,
            max_storage_textures_in_vertex_stage: 0,
            max_storage_textures_in_fragment_stage: 0,
            max_uniform_buffers_per_shader_stage,
            max_vertex_buffers: limits.max_vertex_input_bindings,
            max_buffer_size,
            max_uniform_buffer_binding_size: limits
                .max_uniform_buffer_range
                .min(crate::auxil::MAX_I32_BINDING_SIZE)
                .into(),
            max_storage_buffer_binding_size: limits
                .max_storage_buffer_range
                .min(crate::auxil::MAX_I32_BINDING_SIZE)
                .into(),
            min_uniform_buffer_offset_alignment: limits.min_uniform_buffer_offset_alignment as u32,
            min_storage_buffer_offset_alignment: limits.min_storage_buffer_offset_alignment as u32,
            max_vertex_attributes: limits.max_vertex_input_attributes,
            max_vertex_buffer_array_stride: limits.max_vertex_input_binding_stride,
            max_inter_stage_shader_variables: limits
                .max_vertex_output_components
                .min(limits.max_fragment_input_components)
                / 4
                - 1, // -1 for position
            max_color_attachments,
            max_color_attachment_bytes_per_sample,
            max_compute_workgroup_storage_size: limits.max_compute_shared_memory_size,
            max_compute_invocations_per_workgroup: limits.max_compute_work_group_invocations,
            max_compute_workgroup_size_x: limits.max_compute_work_group_size[0],
            max_compute_workgroup_size_y: limits.max_compute_work_group_size[1],
            max_compute_workgroup_size_z: limits.max_compute_work_group_size[2],
            max_compute_workgroups_per_dimension: limits.max_compute_work_group_count[0]
                .min(limits.max_compute_work_group_count[1])
                .min(limits.max_compute_work_group_count[2]),
            max_immediate_size: limits.max_push_constants_size,
            //
            // NATIVE (Non-WebGPU) LIMITS:
            //
            max_non_sampler_bindings: u32::MAX,

            max_binding_array_elements_per_shader_stage: max_binding_array_elements,
            max_binding_array_sampler_elements_per_shader_stage: max_sampler_binding_array_elements,
            max_binding_array_acceleration_structure_elements_per_shader_stage: if self
                .descriptor_indexing
                .is_some()
            {
                max_acceleration_structures_per_shader_stage
            } else {
                0
            },

            max_task_workgroup_total_count,
            max_task_workgroups_per_dimension,
            max_mesh_workgroup_total_count,
            max_mesh_workgroups_per_dimension,

            max_task_invocations_per_workgroup,
            max_task_invocations_per_dimension,

            max_mesh_invocations_per_workgroup,
            max_mesh_invocations_per_dimension,

            max_task_payload_size,
            max_mesh_output_vertices,
            max_mesh_output_primitives,
            max_mesh_output_layers,
            max_mesh_multiview_view_count,

            max_blas_primitive_count,
            max_blas_geometry_count,
            max_tlas_instance_count,
            max_acceleration_structures_per_shader_stage,
            max_buffers_and_acceleration_structures_per_shader_stage: u32::MAX,

            max_multiview_view_count,

            max_ray_dispatch_count,
            max_ray_recursion_depth,
        })
    }

    /// Return a `wgpu_hal::Alignments` structure describing this adapter.
    ///
    /// The `using_robustness2` argument says how this adapter will implement
    /// `wgpu_hal`'s guarantee that shaders can only read the [accessible
    /// region][ar] of bindgroup's buffer bindings:
    ///
    /// - If this adapter will depend on `VK_EXT_robustness2`'s
    ///   `robustBufferAccess2` feature to apply bounds checks to shader buffer
    ///   access, `using_robustness2` must be `true`.
    ///
    /// - Otherwise, this adapter must use Naga to inject bounds checks on
    ///   buffer accesses, and `using_robustness2` must be `false`.
    ///
    /// [ar]: ../../struct.BufferBinding.html#accessible-region
    fn to_hal_alignments(&self, using_robustness2: bool) -> crate::Alignments {
        let limits = &self.properties.limits;
        crate::Alignments {
            buffer_copy_offset: wgt::BufferSize::new(limits.optimal_buffer_copy_offset_alignment)
                .unwrap(),
            buffer_copy_pitch: wgt::BufferSize::new(limits.optimal_buffer_copy_row_pitch_alignment)
                .unwrap(),
            uniform_bounds_check_alignment: {
                let alignment = if using_robustness2 {
                    self.robustness2
                        .unwrap() // if we're using it, we should have its properties
                        .robust_uniform_buffer_access_size_alignment
                } else {
                    // If the `robustness2` properties are unavailable, then `robustness2` is not available either Naga-injected bounds checks are precise.
                    1
                };
                wgt::BufferSize::new(alignment).unwrap()
            },
            raw_tlas_instance_size: 64,
            ray_tracing_scratch_buffer_alignment: self.acceleration_structure.map_or(
                0,
                |acceleration_structure| {
                    acceleration_structure.min_acceleration_structure_scratch_offset_alignment
                },
            ),
            ray_tracing_pipeline_group_data_size: self
                .ray_tracing_pipeline
                .map_or(0, |ray_tracing_pipeline| {
                    ray_tracing_pipeline.shader_group_handle_size
                }),
            ray_tracing_pipeline_group_data_alignment: self
                .ray_tracing_pipeline
                .map_or(0, |ray_tracing_pipeline| {
                    ray_tracing_pipeline.shader_group_handle_alignment
                }),
            ray_tracing_pipeline_data_offset_alignment: self
                .ray_tracing_pipeline
                .map_or(0, |ray_tracing_pipeline| {
                    ray_tracing_pipeline.shader_group_base_alignment
                }),
        }
    }
}

impl super::InstanceShared {
    fn inspect(
        &self,
        phd: vk::PhysicalDevice,
    ) -> (PhysicalDeviceProperties, PhysicalDeviceFeatures) {
        let capabilities = {
            let mut capabilities = PhysicalDeviceProperties::default();
            capabilities.supported_extensions =
                unsafe { self.raw.enumerate_device_extension_properties(phd).unwrap() };
            capabilities.properties = unsafe { self.raw.get_physical_device_properties(phd) };
            capabilities.device_api_version = capabilities.properties.api_version;
            capabilities.drop_extensions_unusable_with(
                &self.extensions,
                self.get_physical_device_properties.is_some(),
            );

            let supports_multiview = capabilities.device_api_version >= vk::API_VERSION_1_1
                || capabilities.supports_extension(khr::multiview::NAME);

            if let Some(ref get_device_properties) = self.get_physical_device_properties {
                // Get these now to avoid borrowing conflicts later
                let supports_maintenance3 = capabilities.device_api_version >= vk::API_VERSION_1_1
                    || capabilities.supports_extension(khr::maintenance3::NAME);
                let supports_maintenance4 = capabilities.device_api_version >= vk::API_VERSION_1_3
                    || capabilities.supports_extension(khr::maintenance4::NAME);
                let supports_maintenance5 = capabilities.device_api_version
                    >= vk::make_api_version(0, 1, 4, 0) // TODO: Use `vk::API_VERSION_1_4` after `ash` is updated.
                    || capabilities.supports_extension(khr::maintenance5::NAME);
                let supports_descriptor_indexing = capabilities.device_api_version
                    >= vk::API_VERSION_1_2
                    || capabilities.supports_extension(ext::descriptor_indexing::NAME);
                let supports_driver_properties = capabilities.device_api_version
                    >= vk::API_VERSION_1_2
                    || capabilities.supports_extension(khr::driver_properties::NAME);
                let supports_subgroup_size_control = capabilities.device_api_version
                    >= vk::API_VERSION_1_3
                    || capabilities.supports_extension(ext::subgroup_size_control::NAME);
                let supports_robustness2 = capabilities.supports_extension(ext::robustness2::NAME);
                let supports_pci_bus_info =
                    capabilities.supports_extension(ext::pci_bus_info::NAME);

                let supports_acceleration_structure =
                    capabilities.supports_extension(khr::acceleration_structure::NAME);

                let supports_ray_tracing_pipeline =
                    capabilities.supports_extension(khr::ray_tracing_pipeline::NAME);

                let supports_mesh_shader = capabilities.supports_extension(ext::mesh_shader::NAME);

                let mut properties2 = vk::PhysicalDeviceProperties2KHR::default();
                if supports_maintenance3 {
                    let next = capabilities
                        .maintenance_3
                        .insert(vk::PhysicalDeviceMaintenance3Properties::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_maintenance4 {
                    let next = capabilities
                        .maintenance_4
                        .insert(vk::PhysicalDeviceMaintenance4Properties::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_maintenance5 {
                    let next = capabilities
                        .maintenance_5
                        .insert(vk::PhysicalDeviceMaintenance5PropertiesKHR::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_descriptor_indexing {
                    let next = capabilities
                        .descriptor_indexing
                        .insert(vk::PhysicalDeviceDescriptorIndexingPropertiesEXT::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_acceleration_structure {
                    let next = capabilities
                        .acceleration_structure
                        .insert(vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_ray_tracing_pipeline {
                    let next = capabilities
                        .ray_tracing_pipeline
                        .insert(vk::PhysicalDeviceRayTracingPipelinePropertiesKHR::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_driver_properties {
                    let next = capabilities
                        .driver
                        .insert(vk::PhysicalDeviceDriverPropertiesKHR::default());
                    properties2 = properties2.push_next(next);
                }

                if capabilities.device_api_version >= vk::API_VERSION_1_1 {
                    let next = capabilities
                        .subgroup
                        .insert(vk::PhysicalDeviceSubgroupProperties::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_subgroup_size_control {
                    let next = capabilities
                        .subgroup_size_control
                        .insert(vk::PhysicalDeviceSubgroupSizeControlProperties::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_robustness2 {
                    let next = capabilities
                        .robustness2
                        .insert(vk::PhysicalDeviceRobustness2PropertiesEXT::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_pci_bus_info {
                    let next = capabilities
                        .pci_bus_info
                        .insert(vk::PhysicalDevicePCIBusInfoPropertiesEXT::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_mesh_shader {
                    let next = capabilities
                        .mesh_shader
                        .insert(vk::PhysicalDeviceMeshShaderPropertiesEXT::default());
                    properties2 = properties2.push_next(next);
                }

                if supports_multiview {
                    let next = capabilities
                        .multiview
                        .insert(vk::PhysicalDeviceMultiviewProperties::default());
                    properties2 = properties2.push_next(next);
                }

                unsafe {
                    get_device_properties.get_physical_device_properties2(phd, &mut properties2)
                };

                // Query cooperative matrix properties
                if capabilities.supports_extension(khr::cooperative_matrix::NAME) {
                    let coop_matrix =
                        khr::cooperative_matrix::Instance::new(&self.entry, &self.raw);
                    capabilities.cooperative_matrix_properties =
                        query_cooperative_matrix_properties(&coop_matrix, phd);
                }

                // Suppress some capabilities to avoid known problems
                if is_intel_igpu_outdated_for_robustness2(&capabilities) {
                    capabilities
                        .supported_extensions
                        .retain(|&x| x.extension_name_as_c_str() != Ok(ext::robustness2::NAME));
                    capabilities.robustness2 = None;
                }

                // Due to https://gitlab.freedesktop.org/mesa/mesa/-/work_items/15725
                // TODO(https://github.com/gfx-rs/wgpu/issues/9742): enable on
                // fixed driver versions, when available
                if capabilities.is_driver(vk::DriverId::MESA_RADV) {
                    capabilities
                        .supported_extensions
                        .retain(|&x| x.extension_name_as_c_str() != Ok(ext::memory_budget::NAME));
                }
            };
            capabilities
        };

        let mut features = PhysicalDeviceFeatures::default();
        let core = if let Some(ref get_device_properties) = self.get_physical_device_properties {
            features = PhysicalDeviceFeatures::queried(&capabilities);
            let mut features2 =
                features.add_to_features2(vk::PhysicalDeviceFeatures2KHR::default());
            unsafe { get_device_properties.get_physical_device_features2(phd, &mut features2) };
            features2.features
        } else {
            unsafe { self.raw.get_physical_device_features(phd) }
        };
        features.core = core;

        (capabilities, features)
    }
}

impl super::Instance {
    pub fn expose_adapter(
        &self,
        phd: vk::PhysicalDevice,
    ) -> Option<crate::ExposedAdapter<super::Api>> {
        use crate::auxil::db;

        let (phd_capabilities, phd_features) = self.shared.inspect(phd);

        let mem_properties = {
            profiling::scope!("vkGetPhysicalDeviceMemoryProperties");
            unsafe { self.shared.raw.get_physical_device_memory_properties(phd) }
        };
        let memory_types = &mem_properties.memory_types_as_slice();
        let supports_lazily_allocated = memory_types.iter().any(|mem| {
            mem.property_flags
                .contains(vk::MemoryPropertyFlags::LAZILY_ALLOCATED)
        });

        let device_type = match phd_capabilities.properties.device_type {
            vk::PhysicalDeviceType::OTHER => wgt::DeviceType::Other,
            vk::PhysicalDeviceType::INTEGRATED_GPU => wgt::DeviceType::IntegratedGpu,
            vk::PhysicalDeviceType::DISCRETE_GPU => wgt::DeviceType::DiscreteGpu,
            vk::PhysicalDeviceType::VIRTUAL_GPU => wgt::DeviceType::VirtualGpu,
            vk::PhysicalDeviceType::CPU => wgt::DeviceType::Cpu,
            _ => wgt::DeviceType::Other,
        };
        let info = wgt::AdapterInfo {
            name: {
                phd_capabilities
                    .properties
                    .device_name_as_c_str()
                    .ok()
                    .and_then(|name| name.to_str().ok())
                    .unwrap_or("?")
                    .to_owned()
            },
            vendor: phd_capabilities.properties.vendor_id,
            device: phd_capabilities.properties.device_id,
            device_pci_bus_id: phd_capabilities
                .pci_bus_info
                .filter(|info| info.pci_bus != 0 || info.pci_device != 0)
                .map(|info| {
                    format!(
                        "{:04x}:{:02x}:{:02x}.{}",
                        info.pci_domain, info.pci_bus, info.pci_device, info.pci_function
                    )
                })
                .unwrap_or_default(),
            driver: {
                phd_capabilities
                    .driver
                    .as_ref()
                    .and_then(|driver| driver.driver_name_as_c_str().ok())
                    .and_then(|name| name.to_str().ok())
                    .unwrap_or("?")
                    .to_owned()
            },
            driver_info: {
                phd_capabilities
                    .driver
                    .as_ref()
                    .and_then(|driver| driver.driver_info_as_c_str().ok())
                    .and_then(|name| name.to_str().ok())
                    .unwrap_or("?")
                    .to_owned()
            },
            subgroup_min_size: phd_capabilities
                .subgroup_size_control
                .map(|subgroup_size| subgroup_size.min_subgroup_size)
                .unwrap_or(wgt::MINIMUM_SUBGROUP_MIN_SIZE),
            subgroup_max_size: phd_capabilities
                .subgroup_size_control
                .map(|subgroup_size| subgroup_size.max_subgroup_size)
                .unwrap_or(wgt::MAXIMUM_SUBGROUP_MAX_SIZE),
            transient_saves_memory: Some(supports_lazily_allocated),
            ..wgt::AdapterInfo::new(device_type, wgt::Backend::Vulkan)
        };
        let mut workarounds = super::Workarounds::empty();
        {
            // TODO: only enable for particular devices
            workarounds |= super::Workarounds::SEPARATE_ENTRY_POINTS;
            workarounds.set(
                super::Workarounds::EMPTY_RESOLVE_ATTACHMENT_LISTS,
                phd_capabilities.properties.vendor_id == db::qualcomm::VENDOR,
            );
            workarounds.set(
                super::Workarounds::FORCE_FILL_BUFFER_WITH_SIZE_GREATER_4096_ALIGNED_OFFSET_16,
                phd_capabilities.properties.vendor_id == db::nvidia::VENDOR,
            );
        };

        if let Some(driver) = phd_capabilities.driver {
            if driver.conformance_version.major == 0 {
                if driver.driver_id == vk::DriverId::MOLTENVK {
                    log::debug!("Adapter is not Vulkan compliant, but is MoltenVK, continuing");
                } else if self
                    .shared
                    .flags
                    .contains(wgt::InstanceFlags::ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER)
                {
                    log::debug!("Adapter is not Vulkan compliant: {}", info.name);
                } else {
                    log::debug!(
                        "Adapter is not Vulkan compliant, hiding adapter: {}",
                        info.name
                    );
                    return None;
                }
            }
        }
        if phd_capabilities.device_api_version == vk::API_VERSION_1_0
            && !phd_capabilities.supports_extension(khr::storage_buffer_storage_class::NAME)
        {
            log::debug!(
                "SPIR-V storage buffer class is not supported, hiding adapter: {}",
                info.name
            );
            return None;
        }
        if !phd_capabilities.supports_extension(khr::maintenance1::NAME)
            && phd_capabilities.device_api_version < vk::API_VERSION_1_1
        {
            log::debug!(
                "VK_KHR_maintenance1 is not supported, hiding adapter: {}",
                info.name
            );
            return None;
        }

        let queue_families = unsafe {
            self.shared
                .raw
                .get_physical_device_queue_family_properties(phd)
        };
        let queue_family_properties = queue_families.first()?;
        let queue_flags = queue_family_properties.queue_flags;
        if !queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            log::debug!("The first queue only exposes {queue_flags:?}");
            return None;
        }

        let format_support =
            FormatSupport::query(&self.shared.raw, phd, &phd_capabilities, &phd_features);
        let (available_features, mut downlevel_flags) = phd_features.to_wgpu(
            &phd_capabilities,
            |extension| phd_capabilities.supports_extension(extension),
            &format_support,
            queue_family_properties,
        );

        if phd_capabilities.is_driver(vk::DriverId::MESA_LLVMPIPE) {
            // The `F16_IN_F32` instructions do not normally require native `F16` support, but on
            // llvmpipe, they do.
            downlevel_flags.set(
                wgt::DownlevelFlags::SHADER_F16_IN_F32,
                available_features.contains(wgt::Features::SHADER_F16),
            );
        }

        downlevel_flags.set(
            wgt::DownlevelFlags::TEXTURE_COMPRESSION,
            available_features.contains(wgt::Features::TEXTURE_COMPRESSION_BC)
                || available_features.contains(
                    wgt::Features::TEXTURE_COMPRESSION_ETC2
                        | wgt::Features::TEXTURE_COMPRESSION_ASTC,
                ),
        );

        let has_robust_buffer_access2 = phd_features
            .robustness2
            .as_ref()
            .map(|r| r.robust_buffer_access2 == 1)
            .unwrap_or_default();

        let alignments = phd_capabilities.to_hal_alignments(has_robust_buffer_access2);

        let mut private_caps = super::PrivateCapabilities {
            // These are set below, from the extensions the adapter supports and the features it
            // offers, as a device's are from those it was created with.
            image_view_usage: false,
            timeline_semaphores: false,
            multi_draw_indirect: false,
            robust_buffer_access: false,
            robust_image_access: false,
            robust_buffer_access2: false,
            robust_image_access2: false,
            zero_initialize_workgroup_memory: false,
            image_format_list: false,
            shader_integer_dot_product: false,
            shader_int8: false,
            store_op_none: false,
            texture_d24: supports_format(
                &self.shared.raw,
                phd,
                vk::Format::X8_D24_UNORM_PACK32,
                vk::ImageTiling::OPTIMAL,
                depth_stencil_required_flags(),
            ),
            texture_d24_s8: supports_format(
                &self.shared.raw,
                phd,
                vk::Format::D24_UNORM_S8_UINT,
                vk::ImageTiling::OPTIMAL,
                depth_stencil_required_flags(),
            ),
            texture_s8: supports_format(
                &self.shared.raw,
                phd,
                vk::Format::S8_UINT,
                vk::ImageTiling::OPTIMAL,
                depth_stencil_required_flags(),
            ),
            max_draw_indirect_count: phd_capabilities.properties.limits.max_draw_indirect_count,
            non_coherent_map_mask: phd_capabilities.properties.limits.non_coherent_atom_size - 1,
            can_present: true,
            maximum_samplers: phd_capabilities
                .properties
                .limits
                .max_sampler_allocation_count,
            multiview_instance_index_limit: phd_capabilities
                .multiview
                .map(|a| a.max_multiview_instance_index)
                .unwrap_or(0),
            scratch_buffer_alignment: alignments.ray_tracing_scratch_buffer_alignment,
            depth_stencil_swizzle_one_support: phd_capabilities
                .maintenance_5
                .map(|maintenance_5| maintenance_5.depth_stencil_swizzle_one_support == vk::TRUE)
                .unwrap_or(false),
            ray_tracing_pipeline_group_data_size: alignments.ray_tracing_pipeline_group_data_size,
            shader_binding_table_alignment: alignments.ray_tracing_pipeline_data_offset_alignment,
        };
        private_caps.set_extension_and_feature_capabilities(
            &phd_capabilities,
            |extension| phd_capabilities.supports_extension(extension),
            &phd_features,
        );
        let downlevel = wgt::DownlevelCapabilities {
            flags: downlevel_flags,
            limits: wgt::DownlevelLimits {},
            shader_model: wgt::ShaderModel::Sm5, //TODO?
        };
        let capabilities = crate::Capabilities {
            limits: phd_capabilities.to_wgpu_limits(),
            alignments,
            downlevel: downlevel.clone(),
            cooperative_matrix_properties: phd_capabilities.cooperative_matrix_properties.clone(),
        };

        let adapter = super::Adapter {
            raw: phd,
            instance: Arc::clone(&self.shared),
            //queue_families,
            known_memory_flags: vk::MemoryPropertyFlags::DEVICE_LOCAL
                | vk::MemoryPropertyFlags::HOST_VISIBLE
                | vk::MemoryPropertyFlags::HOST_COHERENT
                | vk::MemoryPropertyFlags::HOST_CACHED
                | vk::MemoryPropertyFlags::LAZILY_ALLOCATED,
            phd_capabilities,
            phd_features,
            format_support,
            downlevel,
            private_caps,
            workarounds,
        };

        let available_features = if crate::auxil::supports_core_features_and_limits(
            &capabilities.limits,
            &capabilities.downlevel,
        ) {
            available_features | wgt::Features::CORE_FEATURES_AND_LIMITS
        } else {
            available_features
        };

        Some(crate::ExposedAdapter {
            adapter,
            info,
            features: available_features,
            capabilities,
        })
    }
}

impl super::PrivateCapabilities {
    /// Sets the capabilities that depend on which extensions and features a device has: for an
    /// adapter, those it supports and offers; for a device, those it was created with.
    /// `has_extension` tells whether the device has a device extension.
    fn set_extension_and_feature_capabilities(
        &mut self,
        phd_capabilities: &PhysicalDeviceProperties,
        has_extension: impl Fn(&'static CStr) -> bool,
        features: &PhysicalDeviceFeatures,
    ) {
        use crate::auxil::db;

        let api_version = phd_capabilities.device_api_version;
        self.image_view_usage =
            api_version >= vk::API_VERSION_1_1 || has_extension(khr::maintenance2::NAME);
        // VeriSilicon's proprietary driver, as on NXP's i.MX 8 SoCs, advertises timeline
        // semaphores but crashes in `vkQueueSubmit` when wgpu uses them. It reports
        // `VK_DRIVER_ID_MESA_RADV` as its driver ID, so only the vendor ID identifies it.
        // A timeline semaphore needs its functions, from Vulkan 1.2 or its extension; a structure
        // given without either (against the structure pointer chain rule) leaves binary fences.
        self.timeline_semaphores = phd_capabilities.properties.vendor_id != db::verisilicon::VENDOR
            && (api_version >= vk::API_VERSION_1_2 || has_extension(khr::timeline_semaphore::NAME))
            && features
                .timeline_semaphore
                .is_some_and(|features| features.timeline_semaphore == vk::TRUE);
        self.multi_draw_indirect = features.core.multi_draw_indirect != 0;
        //TODO: make configurable
        self.robust_buffer_access = features.core.robust_buffer_access != 0;
        // `VK_EXT_robustness2` is relied on only where the physical device reports its properties:
        // the uniform bounds-check alignment that wgpu-core pads lazy initialization to comes from
        // them (`to_hal_alignments`), and the workaround for outdated Intel drivers
        // (`is_intel_igpu_outdated_for_robustness2`) clears them as it hides the extension. A
        // device created with robustness2 there keeps Naga's bounds checks.
        let robustness2 = features
            .robustness2
            .filter(|_| phd_capabilities.robustness2.is_some());
        self.robust_image_access = robustness2
            .is_some_and(|features| features.robust_image_access2 != 0)
            || features
                .image_robustness
                .is_some_and(|features| features.robust_image_access != 0);
        self.robust_buffer_access2 =
            robustness2.is_some_and(|features| features.robust_buffer_access2 == vk::TRUE);
        self.robust_image_access2 =
            robustness2.is_some_and(|features| features.robust_image_access2 == vk::TRUE);
        self.zero_initialize_workgroup_memory = features
            .zero_initialize_workgroup_memory
            .is_some_and(|features| features.shader_zero_initialize_workgroup_memory == vk::TRUE);
        self.image_format_list =
            api_version >= vk::API_VERSION_1_2 || has_extension(khr::image_format_list::NAME);
        self.shader_integer_dot_product = features
            .shader_integer_dot_product
            .is_some_and(|features| features.shader_integer_dot_product != 0);
        self.shader_int8 = features
            .shader_float16_int8
            .is_some_and(|features| features.shader_int8 != 0);
        self.store_op_none = api_version >= vk::API_VERSION_1_3
            || (api_version >= vk::API_VERSION_1_2 && has_extension(khr::dynamic_rendering::NAME))
            || has_extension(khr::load_store_op_none::NAME)
            || has_extension(ash::qcom::render_pass_store_ops::NAME)
            || has_extension(ext::load_store_op_none::NAME);
    }
}

impl super::Adapter {
    pub fn raw_physical_device(&self) -> vk::PhysicalDevice {
        self.raw
    }

    /// The downlevel capabilities of a device created from this adapter with the extensions
    /// `enabled_extensions` and the features `enabled_features`, which [`Self::device_from_raw`]
    /// gives the device: the adapter's, but for the flags that depend on what a device enables.
    ///
    /// A device without `independentBlend`, `sampleRateShading`, `imageCubeArray`,
    /// `samplerAnisotropy`, `fragmentStoresAndAtomics`, `depthBiasClamp` or
    /// `fullDrawIndexUint32` lacks `INDEPENDENT_BLEND`, `MULTISAMPLED_SHADING`,
    /// `CUBE_ARRAY_TEXTURES`, `ANISOTROPIC_FILTERING`, `FRAGMENT_WRITABLE_STORAGE`,
    /// `DEPTH_BIAS_CLAMP` or `FULL_DRAW_INDEX_UINT32`, and wgpu-core validates its use against the
    /// first six. `READ_ONLY_DEPTH_STENCIL` follows `VK_KHR_maintenance2` before Vulkan 1.1, and
    /// `SURFACE_VIEW_FORMATS` `VK_KHR_swapchain_mutable_format`. A device has no flag that its
    /// adapter lacks. The devices that wgpu-hal opens have their adapter's, unless a
    /// [`Self::open_with_callback`] callback removes an extension that gives a flag.
    pub fn device_downlevel_capabilities(
        &self,
        enabled_extensions: &[&'static CStr],
        enabled_features: &PhysicalDeviceFeatures,
    ) -> wgt::DownlevelCapabilities {
        device_downlevel_capabilities(
            &self.downlevel,
            &self.phd_capabilities,
            |extension| enabled_extensions.contains(&extension),
            enabled_features,
        )
    }

    /// The wgpu features of a device created from this adapter with the extensions
    /// `enabled_extensions` and the features `enabled_features`, whose queue is of the family
    /// `family_index`: those that its extensions and features give, and with which
    /// [`Self::device_from_raw`] takes it. A device that wgpu-hal opens with `features`, with the
    /// extensions of [`Self::required_device_extensions`] and the features of
    /// [`Self::physical_device_features`], has them.
    ///
    /// `CORE_FEATURES_AND_LIMITS` needs the downlevel capabilities of a WebGPU core device, which
    /// a device without a core feature behind a downlevel flag lacks
    /// ([`Self::device_downlevel_capabilities`]).
    ///
    /// A `family_index` that the physical device does not have gives no timestamp features, and
    /// [`Self::device_from_raw`] refuses such a queue with an error.
    pub fn device_features(
        &self,
        enabled_extensions: &[&'static CStr],
        enabled_features: &PhysicalDeviceFeatures,
        family_index: u32,
    ) -> wgt::Features {
        let queue_props = self
            .queue_family_properties(family_index)
            .unwrap_or_default();
        self.features_of_device(enabled_extensions, enabled_features, &queue_props)
    }

    /// The properties of the physical device's queue family `family_index`; an error if it has
    /// no such family.
    fn queue_family_properties(
        &self,
        family_index: u32,
    ) -> Result<vk::QueueFamilyProperties, crate::DeviceError> {
        // SAFETY: A physical device of the instance.
        unsafe {
            self.instance
                .raw
                .get_physical_device_queue_family_properties(self.raw)
        }
        .get(family_index as usize)
        .copied()
        .ok_or(crate::DeviceError::Unexpected)
    }

    /// [`Self::device_features`], for a queue of the family that `queue_props` describes.
    fn features_of_device(
        &self,
        enabled_extensions: &[&'static CStr],
        enabled_features: &PhysicalDeviceFeatures,
        queue_props: &vk::QueueFamilyProperties,
    ) -> wgt::Features {
        let (features, _) = enabled_features.to_wgpu(
            &self.phd_capabilities,
            |extension| enabled_extensions.contains(&extension),
            &self.format_support,
            queue_props,
        );
        let downlevel = self.device_downlevel_capabilities(enabled_extensions, enabled_features);
        if crate::auxil::supports_core_features_and_limits(
            &self.phd_capabilities.to_wgpu_limits(),
            &downlevel,
        ) {
            features | wgt::Features::CORE_FEATURES_AND_LIMITS
        } else {
            features
        }
    }

    pub fn get_physical_device_features(&self) -> &PhysicalDeviceFeatures {
        &self.phd_features
    }

    pub fn physical_device_capabilities(&self) -> &PhysicalDeviceProperties {
        &self.phd_capabilities
    }

    pub fn shared_instance(&self) -> &super::InstanceShared {
        &self.instance
    }

    pub fn required_device_extensions(&self, features: wgt::Features) -> Vec<&'static CStr> {
        let (supported_extensions, unsupported_extensions) = self
            .phd_capabilities
            .get_required_extensions(features)
            .iter()
            .partition::<Vec<&CStr>, _>(|&&extension| {
                self.phd_capabilities.supports_extension(extension)
            });

        if !unsupported_extensions.is_empty() {
            log::debug!("Missing extensions: {unsupported_extensions:?}");
        }

        log::debug!("Supported extensions: {supported_extensions:?}");
        supported_extensions
    }

    /// Create a `PhysicalDeviceFeatures` for opening a logical device with
    /// `features` from this adapter.
    ///
    /// The given `enabled_extensions` set must include all the extensions
    /// selected by [`required_device_extensions`] when passed `features`.
    /// Otherwise, the `PhysicalDeviceFeatures` value may not be able to select
    /// all the Vulkan features needed to represent `features` and this
    /// adapter's characteristics.
    ///
    /// Typically, you'd simply call `required_device_extensions`, and then pass
    /// its return value and the feature set you gave it directly to this
    /// function. But it's fine to add more extensions to the list.
    ///
    /// [`required_device_extensions`]: Self::required_device_extensions
    pub fn physical_device_features(
        &self,
        enabled_extensions: &[&'static CStr],
        features: wgt::Features,
    ) -> PhysicalDeviceFeatures {
        PhysicalDeviceFeatures::from_extensions_and_requested_features(
            &self.phd_capabilities,
            &self.phd_features,
            enabled_extensions,
            features,
            self.downlevel.flags,
            &self.private_caps,
        )
    }

    /// Wraps `raw_device`, a device created from this adapter, as a wgpu-hal device.
    ///
    /// wgpu-hal takes its paths from the extensions and features that the device was created
    /// with, `enabled_extensions` and `enabled_features`, never from what the adapter offers.
    /// Where the device lacks a feature that wgpu-hal would use, it takes the path that does
    /// without it: binary fences without `timelineSemaphore`, bounds checks that Naga injects
    /// without `robustBufferAccess2` or `robustImageAccess`, zero-initialization that Naga
    /// polyfills without `shaderZeroInitializeWorkgroupMemory`, and a swapchain teardown that waits
    /// for the queue without `swapchainMaintenance1`, among others. Without a core feature behind a
    /// downlevel flag, the device lacks the flag ([`Self::device_downlevel_capabilities`]), and
    /// wgpu-core refuses what would use the feature.
    ///
    /// A device that wgpu-hal did not create gives `enabled_features` from the
    /// `VkDeviceCreateInfo` that its creator gave `vkCreateDevice`, with
    /// [`PhysicalDeviceFeatures::from_device_create_info`], or from its creator's
    /// `VkPhysicalDeviceFeatures2` chain, with [`PhysicalDeviceFeatures::from_enabled`]. One
    /// created as wgpu-hal creates its own gives the [`Self::physical_device_features`] it was
    /// created with.
    ///
    /// Each of `features` must be among those that the device's extensions and features give,
    /// [`Self::device_features`]: the Vulkan features and extensions that it needs, and on which
    /// wgpu-hal relies wherever it is requested.
    ///
    /// # Safety
    ///
    /// - `raw_device` must be created from this adapter, with `queue`.
    /// - `enabled_extensions` must be the device extensions `raw_device` was created with; those
    ///   wgpu-hal does not know may be left out. On a Vulkan 1.0 device they must include
    ///   `VK_KHR_maintenance1` and `VK_KHR_storage_buffer_storage_class`, and `VK_KHR_swapchain`
    ///   for surfaces.
    /// - `enabled_features` must be the features `raw_device` was created with.
    /// - If `drop_callback` is [`None`], wgpu-hal will take ownership of `raw_device`. If
    ///   `drop_callback` is [`Some`], `raw_device` must be valid until the callback is called.
    /// - Destroying the device requires that no other code uses the device or its queues, and
    ///   that code outside wgpu-hal has destroyed every object it created on it.
    ///
    /// # Errors
    ///
    /// On an error, wgpu-hal has not taken `raw_device`: the caller keeps it, and `drop_callback`
    /// is dropped without being called.
    ///
    /// # Panics
    ///
    /// Before taking `raw_device`: if `features` has a feature that is not among
    /// [`Self::device_features`] for `queue`'s family, naming it; if `queue`'s flags include
    /// `VK_DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_BIT_KHR` and `enabled_extensions` lacks
    /// `VK_KHR_internally_synchronized_queues`, or if `queue` has flags and the device lacks
    /// `vkGetDeviceQueue2` (Vulkan 1.1).
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn device_from_raw(
        &self,
        raw_device: ash::Device,
        drop_callback: Option<crate::DropCallback>,
        enabled_extensions: &[&'static CStr],
        enabled_features: &PhysicalDeviceFeatures,
        features: wgt::Features,
        limits: &wgt::Limits,
        memory_hints: &wgt::MemoryHints,
        queue: super::DeviceQueue,
    ) -> Result<crate::OpenDevice<super::Api>, crate::DeviceError> {
        let queue_family_properties = self.queue_family_properties(queue.family_index)?;
        let lacking = features.difference(self.features_of_device(
            enabled_extensions,
            enabled_features,
            &queue_family_properties,
        ));
        if !lacking.is_empty() {
            let names = lacking
                .iter_names()
                .map(|(name, _)| name)
                .collect::<Vec<_>>()
                .join(", ");
            crate::hal_usage_error(format_args!(
                "requested {names}, which the extensions, features and queue family {} that the \
                 device was created with do not give (Adapter::device_features)",
                queue.family_index
            ));
        }
        // SAFETY: As our caller guarantees, and the device gives each of `features`.
        unsafe {
            self.device_from_raw_unchecked(
                raw_device,
                drop_callback,
                enabled_extensions,
                enabled_features,
                features,
                limits,
                memory_hints,
                queue,
                queue_family_properties,
            )
        }
    }

    /// [`Self::device_from_raw`], for a device that gives each of `features`, with a queue of the
    /// family that `queue_family_properties` describes.
    ///
    /// # Safety
    ///
    /// As [`Self::device_from_raw`]'s, and each of `features` is among
    /// [`Self::device_features`].
    #[allow(clippy::too_many_arguments)]
    unsafe fn device_from_raw_unchecked(
        &self,
        raw_device: ash::Device,
        drop_callback: Option<crate::DropCallback>,
        enabled_extensions: &[&'static CStr],
        enabled_features: &PhysicalDeviceFeatures,
        features: wgt::Features,
        limits: &wgt::Limits,
        memory_hints: &wgt::MemoryHints,
        queue: super::DeviceQueue,
        queue_family_properties: vk::QueueFamilyProperties,
    ) -> Result<crate::OpenDevice<super::Api>, crate::DeviceError> {
        let super::DeviceQueue {
            family_index,
            queue_index,
            create_flags,
            ref lock,
        } = queue;
        let internally_synchronized =
            create_flags.contains(DEVICE_QUEUE_CREATE_INTERNALLY_SYNCHRONIZED_KHR);
        if internally_synchronized && !enabled_extensions.contains(&INTERNALLY_SYNCHRONIZED_QUEUES)
        {
            crate::hal_usage_error(
                "an internally synchronized queue needs VK_KHR_internally_synchronized_queues",
            );
        }
        // The device's own capabilities: those that depend on extensions and features from what
        // it was created with, the others from its adapter.
        let mut private_caps = self.private_caps.clone();
        private_caps.set_extension_and_feature_capabilities(
            &self.phd_capabilities,
            |extension| enabled_extensions.contains(&extension),
            enabled_features,
        );
        let downlevel = self.device_downlevel_capabilities(enabled_extensions, enabled_features);
        let mem_properties = {
            profiling::scope!("vkGetPhysicalDeviceMemoryProperties");
            unsafe {
                self.instance
                    .raw
                    .get_physical_device_memory_properties(self.raw)
            }
        };
        let queue_flags = queue_family_properties.queue_flags;
        let memory_types = &mem_properties.memory_types_as_slice();
        let valid_ash_memory_types = memory_types.iter().enumerate().fold(0, |u, (i, mem)| {
            if self.known_memory_flags.contains(mem.property_flags) {
                u | (1 << i)
            } else {
                u
            }
        });

        // Note that VK_EXT_debug_utils is an instance extension (enabled at the instance
        // level) but contains a few functions that can be loaded directly on the Device for a
        // dispatch-table-less pointer.
        let debug_utils_fn = if self.instance.extensions.contains(&ext::debug_utils::NAME) {
            Some(ext::debug_utils::Device::new(
                &self.instance.raw,
                &raw_device,
            ))
        } else {
            None
        };
        let indirect_count_fn = if enabled_extensions.contains(&khr::draw_indirect_count::NAME) {
            Some(khr::draw_indirect_count::Device::new(
                &self.instance.raw,
                &raw_device,
            ))
        } else {
            None
        };
        let timeline_semaphore_fn = if enabled_extensions.contains(&khr::timeline_semaphore::NAME) {
            Some(super::ExtensionFn::Extension(
                khr::timeline_semaphore::Device::new(&self.instance.raw, &raw_device),
            ))
        } else if self.phd_capabilities.device_api_version >= vk::API_VERSION_1_2 {
            Some(super::ExtensionFn::Promoted)
        } else {
            None
        };
        let ray_tracing_fns = if enabled_extensions.contains(&khr::acceleration_structure::NAME)
            && enabled_extensions.contains(&khr::buffer_device_address::NAME)
        {
            Some(super::RayTracingDeviceExtensionFunctions {
                acceleration_structure: khr::acceleration_structure::Device::new(
                    &self.instance.raw,
                    &raw_device,
                ),
                buffer_device_address: khr::buffer_device_address::Device::new(
                    &self.instance.raw,
                    &raw_device,
                ),
            })
        } else {
            None
        };
        let ray_tracing_pipeline_fns =
            if enabled_extensions.contains(&khr::ray_tracing_pipeline::NAME) {
                Some(khr::ray_tracing_pipeline::Device::new(
                    &self.instance.raw,
                    &raw_device,
                ))
            } else {
                None
            };
        let mesh_shading_fns = if enabled_extensions.contains(&ext::mesh_shader::NAME) {
            Some(ext::mesh_shader::Device::new(
                &self.instance.raw,
                &raw_device,
            ))
        } else {
            None
        };
        let external_memory_fd_fn = if enabled_extensions.contains(&khr::external_memory_fd::NAME) {
            Some(khr::external_memory_fd::Device::new(
                &self.instance.raw,
                &raw_device,
            ))
        } else {
            None
        };

        let naga_options = {
            use naga::back::spv;

            // The following capabilities are always available
            // see https://registry.khronos.org/vulkan/specs/1.3-extensions/html/chap52.html#spirvenv-capabilities
            let mut capabilities = vec![
                spv::Capability::Shader,
                spv::Capability::Matrix,
                spv::Capability::Sampled1D,
                spv::Capability::Image1D,
                spv::Capability::ImageQuery,
                spv::Capability::DerivativeControl,
                spv::Capability::StorageImageExtendedFormats,
            ];

            if enabled_features.core.image_cube_array != 0 {
                capabilities.push(spv::Capability::SampledCubeArray);
            }

            if enabled_features.core.sample_rate_shading != 0 {
                capabilities.push(spv::Capability::SampleRateShading);
            }

            if features.contains(wgt::Features::MULTIVIEW) {
                capabilities.push(spv::Capability::MultiView);
            }

            if features.contains(wgt::Features::PRIMITIVE_INDEX) {
                capabilities.push(spv::Capability::Geometry);
            }

            if features.intersects(wgt::Features::SUBGROUP | wgt::Features::SUBGROUP_VERTEX) {
                capabilities.push(spv::Capability::GroupNonUniform);
                capabilities.push(spv::Capability::GroupNonUniformVote);
                capabilities.push(spv::Capability::GroupNonUniformArithmetic);
                capabilities.push(spv::Capability::GroupNonUniformBallot);
                capabilities.push(spv::Capability::GroupNonUniformShuffle);
                capabilities.push(spv::Capability::GroupNonUniformShuffleRelative);
                capabilities.push(spv::Capability::GroupNonUniformQuad);
            }

            if features.intersects(
                wgt::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
                    | wgt::Features::STORAGE_TEXTURE_ARRAY_NON_UNIFORM_INDEXING
                    | wgt::Features::UNIFORM_BUFFER_BINDING_ARRAYS,
            ) {
                capabilities.push(spv::Capability::ShaderNonUniform);
            }
            if features.contains(wgt::Features::BGRA8UNORM_STORAGE) {
                capabilities.push(spv::Capability::StorageImageWriteWithoutFormat);
            }

            if features.contains(wgt::Features::EXPERIMENTAL_RAY_QUERY) {
                capabilities.push(spv::Capability::RayQueryKHR);
            }

            if features.contains(wgt::Features::SHADER_INT64) {
                capabilities.push(spv::Capability::Int64);
            }

            if features.contains(wgt::Features::SHADER_F16) {
                capabilities.push(spv::Capability::Float16);
            }

            if features.contains(wgt::Features::SHADER_I16) {
                capabilities.push(spv::Capability::Int16);
            }

            if features.intersects(
                wgt::Features::SHADER_INT64_ATOMIC_ALL_OPS
                    | wgt::Features::SHADER_INT64_ATOMIC_MIN_MAX
                    | wgt::Features::TEXTURE_INT64_ATOMIC,
            ) {
                capabilities.push(spv::Capability::Int64Atomics);
            }

            if features.intersects(wgt::Features::TEXTURE_INT64_ATOMIC) {
                capabilities.push(spv::Capability::Int64ImageEXT);
            }

            if features.contains(wgt::Features::SHADER_FLOAT32_ATOMIC) {
                capabilities.push(spv::Capability::AtomicFloat32AddEXT);
            }

            if features.contains(wgt::Features::CLIP_DISTANCES) {
                capabilities.push(spv::Capability::ClipDistance);
            }

            // Vulkan bundles both barycentrics and per-vertex attributes under the same feature.
            if features
                .intersects(wgt::Features::SHADER_BARYCENTRICS | wgt::Features::SHADER_PER_VERTEX)
            {
                capabilities.push(spv::Capability::FragmentBarycentricKHR);
            }

            if features.contains(wgt::Features::SHADER_DRAW_INDEX) {
                capabilities.push(spv::Capability::DrawParameters);
            }

            let mut flags = spv::WriterFlags::empty();
            flags.set(
                spv::WriterFlags::DEBUG,
                self.instance.flags.contains(wgt::InstanceFlags::DEBUG),
            );
            flags.set(
                spv::WriterFlags::LABEL_VARYINGS,
                self.phd_capabilities.properties.vendor_id != crate::auxil::db::qualcomm::VENDOR,
            );
            flags.set(
                spv::WriterFlags::FORCE_POINT_SIZE,
                //Note: we could technically disable this when we are compiling separate entry points,
                // and we know exactly that the primitive topology is not `PointList`.
                // But this requires cloning the `spv::Options` struct, which has heap allocations.
                true, // could check `super::Workarounds::SEPARATE_ENTRY_POINTS`
            );
            flags.set(
                spv::WriterFlags::PRINT_ON_RAY_QUERY_INITIALIZATION_FAIL
                    | spv::WriterFlags::PRINT_ON_TRACE_RAYS_FAIL,
                self.instance.flags.contains(wgt::InstanceFlags::DEBUG)
                    && (self.instance.instance_api_version >= vk::API_VERSION_1_3
                        || enabled_extensions.contains(&khr::shader_non_semantic_info::NAME)),
            );
            if features.contains(wgt::Features::EXPERIMENTAL_RAY_QUERY) {
                capabilities.push(spv::Capability::RayQueryKHR);
            }
            if features.contains(wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES) {
                capabilities.push(spv::Capability::RayTracingKHR);
            }
            if features.contains(wgt::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN) {
                capabilities.push(spv::Capability::RayQueryPositionFetchKHR)
            }
            if features.contains(wgt::Features::EXPERIMENTAL_MESH_SHADER) {
                capabilities.push(spv::Capability::MeshShadingEXT);
            }
            if features.contains(wgt::Features::EXPERIMENTAL_COOPERATIVE_MATRIX) {
                capabilities.push(spv::Capability::CooperativeMatrixKHR);
                // TODO: expose this more generally
                capabilities.push(spv::Capability::VulkanMemoryModel);
            }
            if private_caps.shader_integer_dot_product {
                // See <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/VK_KHR_shader_integer_dot_product.html#_new_spir_v_capabilities>.
                capabilities.extend(&[
                    spv::Capability::DotProductInputAllKHR,
                    spv::Capability::DotProductInput4x8BitKHR,
                    spv::Capability::DotProductInput4x8BitPackedKHR,
                    spv::Capability::DotProductKHR,
                ]);
            }
            if private_caps.shader_int8 {
                // See <https://registry.khronos.org/vulkan/specs/latest/man/html/VkPhysicalDeviceShaderFloat16Int8Features.html#extension-features-shaderInt8>.
                capabilities.extend(&[spv::Capability::Int8]);
            }
            spv::Options {
                lang_version: {
                    // Use maximum supported SPIR-V version according to
                    // <https://github.com/KhronosGroup/Vulkan-Docs/blob/19b7651/appendices/spirvenv.adoc?plain=1#L21-L40>.
                    let version = match self.phd_capabilities.device_api_version {
                        vk::API_VERSION_1_0..vk::API_VERSION_1_1 => (1, 0),
                        vk::API_VERSION_1_1..vk::API_VERSION_1_2 => (1, 3),
                        vk::API_VERSION_1_2..vk::API_VERSION_1_3 => (1, 5),
                        vk::API_VERSION_1_3.. => (1, 6),
                        _ => unreachable!(),
                    };
                    // `VK_KHR_spirv_1_4` raises that ceiling on pre-1.2 devices, which
                    // both mesh shaders and ray tracing pipelines need it to do.
                    if enabled_extensions.contains(&khr::spirv_1_4::NAME) {
                        version.max((1, 4))
                    } else {
                        version
                    }
                },
                flags,
                capabilities: Some(capabilities.iter().cloned().collect()),
                bounds_check_policies: naga::proc::BoundsCheckPolicies {
                    index: naga::proc::BoundsCheckPolicy::Restrict,
                    buffer: if private_caps.robust_buffer_access2 {
                        naga::proc::BoundsCheckPolicy::Unchecked
                    } else {
                        naga::proc::BoundsCheckPolicy::Restrict
                    },
                    image_load: if private_caps.robust_image_access {
                        naga::proc::BoundsCheckPolicy::Unchecked
                    } else {
                        naga::proc::BoundsCheckPolicy::Restrict
                    },
                    // TODO: support bounds checks on binding arrays
                    binding_array: naga::proc::BoundsCheckPolicy::Unchecked,
                },
                zero_initialize_workgroup_memory: if private_caps.zero_initialize_workgroup_memory {
                    spv::ZeroInitializeWorkgroupMemoryMode::Native
                } else {
                    spv::ZeroInitializeWorkgroupMemoryMode::Polyfill
                },
                force_loop_bounding: true,
                ray_query_initialization_tracking: true,
                use_storage_input_output_16: features.contains(wgt::Features::SHADER_F16)
                    && enabled_features.supports_storage_input_output_16(),
                fake_missing_bindings: false,
                // We need to build this separately for each invocation, so just default it out here
                binding_map: BTreeMap::default(),
                debug_info: None,
                task_dispatch_limits: Some(naga::back::TaskDispatchLimits {
                    max_mesh_workgroups_per_dim: limits.max_mesh_workgroups_per_dimension,
                    max_mesh_workgroups_total: limits.max_mesh_workgroup_total_count,
                }),
                mesh_shader_primitive_indices_clamp: true,
                trace_ray_argument_validation: true,
                emit_int_div_checks: true,
            }
        };

        let raw_queue = {
            profiling::scope!("vkGetDeviceQueue");
            if create_flags.is_empty() {
                unsafe { raw_device.get_device_queue(family_index, queue_index) }
            } else {
                // A queue created with flags is retrieved with them (VUID-vkGetDeviceQueue-flags-01841).
                let get_device_queue2 = unsafe {
                    self.instance
                        .raw
                        .get_device_proc_addr(raw_device.handle(), c"vkGetDeviceQueue2".as_ptr())
                };
                if get_device_queue2.is_none() {
                    crate::hal_usage_error("a queue created with flags needs vkGetDeviceQueue2");
                }
                let info = vk::DeviceQueueInfo2::default()
                    .flags(create_flags)
                    .queue_family_index(family_index)
                    .queue_index(queue_index);
                unsafe { raw_device.get_device_queue2(&info) }
            }
        };
        // Retrieval flags that differ from the creation flags give no queue, on Mesa.
        if raw_queue == vk::Queue::null() {
            log::error!("vkGetDeviceQueue2 returned no queue for {queue:?}");
            return Err(crate::DeviceError::Unexpected);
        }

        let driver_version = self
            .phd_capabilities
            .properties
            .driver_version
            .to_be_bytes();
        #[rustfmt::skip]
        let pipeline_cache_validation_key = [
            driver_version[0], driver_version[1], driver_version[2], driver_version[3],
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
        ];

        // Every step that can fail comes before wgpu-hal takes the device: on an error, the device
        // and its drop callback stay with the caller, and what was created on it is destroyed.
        let empty_descriptor_set_layout = unsafe {
            raw_device
                .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default(), None)
                .map_err(super::map_host_device_oom_err)?
        };
        let relay_signal =
            match unsafe { raw_device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
            {
                Ok(semaphore) => semaphore,
                Err(err) => {
                    unsafe {
                        raw_device.destroy_descriptor_set_layout(empty_descriptor_set_layout, None)
                    };
                    return Err(super::map_host_device_oom_err(err));
                }
            };
        let allocation_sizes = AllocationSizes::from_memory_hints(memory_hints);
        let buffer_device_address = uses_device_addresses(enabled_extensions, enabled_features);
        let mem_allocator = match super::MemoryAllocators::new(
            self.instance.raw.clone(),
            raw_device.clone(),
            self.raw,
            buffer_device_address,
            allocation_sizes,
        ) {
            Ok(allocator) => allocator,
            Err(err) => {
                unsafe {
                    raw_device.destroy_semaphore(relay_signal, None);
                    raw_device.destroy_descriptor_set_layout(empty_descriptor_set_layout, None);
                }
                return Err(err.into());
            }
        };

        let drop_guard = crate::DropGuard::from_option(drop_callback);

        let shared = Arc::new(super::DeviceShared {
            raw: raw_device,
            family_index,
            queue_flags,
            queue_index,
            raw_queue,
            // A lock given is always taken; without one, wgpu-hal's own, unless the queue is
            // internally synchronized.
            queue_lock: match lock {
                Some(lock) => Some(Arc::clone(lock)),
                None if internally_synchronized => None,
                None => {
                    let own: Arc<dyn super::QueueLock> =
                        Arc::new(super::queue_lock::DeviceQueueLock::new());
                    Some(own)
                }
            }
            .map(|lock| Arc::new(super::queue_lock::TrackedQueueLock::new(lock))),
            queue_scope: super::QueueLockScope::Queue {
                family_index,
                queue_index,
            },
            drop_guard,
            instance: Arc::clone(&self.instance),
            physical_device: self.raw,
            enabled_extensions: enabled_extensions.into(),
            present_fences: enables_swapchain_maintenance1(enabled_features, enabled_extensions),
            extension_fns: super::DeviceExtensionFunctions {
                debug_utils: debug_utils_fn,
                draw_indirect_count: indirect_count_fn,
                timeline_semaphore: timeline_semaphore_fn,
                ray_tracing: ray_tracing_fns,
                ray_tracing_pipelines: ray_tracing_pipeline_fns,
                mesh_shading: mesh_shading_fns,
                external_memory_fd: external_memory_fd_fn,
            },
            pipeline_cache_validation_key,
            vendor_id: self.phd_capabilities.properties.vendor_id,
            timestamp_period: self.phd_capabilities.properties.limits.timestamp_period,
            sampler_cache: Mutex::new(super::sampler::SamplerCache::new(
                private_caps.maximum_samplers,
            )),
            private_caps,
            features,
            downlevel,
            workarounds: self.workarounds,
            render_passes: Mutex::new(Default::default()),
            memory_allocations_counter: Default::default(),

            texture_identity_factory: super::ResourceIdentityFactory::new(),
            texture_view_identity_factory: super::ResourceIdentityFactory::new(),
            empty_descriptor_set_layout,
        });

        if !self
            .instance
            .flags
            .contains(wgt::InstanceFlags::DISCARD_HAL_LABELS)
        {
            unsafe { shared.set_object_name(relay_signal, "RelaySemaphores: 1") };
        }

        let queue = super::Queue {
            raw: raw_queue,
            device: Arc::clone(&shared),
            family_index,
            relay_semaphores: Mutex::new(super::RelaySemaphores::new(relay_signal)),
            signal_semaphores: Mutex::new(SemaphoreList::new(SemaphoreListMode::Signal)),
            wait_semaphores: Mutex::new(SemaphoreList::new(SemaphoreListMode::Wait)),
            next_submit_chain: Mutex::new(None),
        };

        let desc_allocator = super::descriptor::DescriptorAllocator::new(
            if let Some(di) = self.phd_capabilities.descriptor_indexing {
                di.max_update_after_bind_descriptors_in_all_pools
            } else {
                0
            },
        );

        let device = super::Device {
            shared,
            mem_allocator,
            desc_allocator: Mutex::new(desc_allocator),
            valid_ash_memory_types,
            naga_options,
            #[cfg(feature = "renderdoc")]
            render_doc: Default::default(),
            counters: Default::default(),
        };

        Ok(crate::OpenDevice { device, queue })
    }

    pub fn texture_format_as_raw(&self, texture_format: wgt::TextureFormat) -> vk::Format {
        self.private_caps.map_texture_format(texture_format)
    }

    /// Removing an extension in the callback removes the downlevel flags it gives
    /// ([`Self::device_downlevel_capabilities`]).
    ///
    /// # Safety:
    /// - Same as `open` plus
    /// - The callback may not change anything that the device does not support.
    /// - The callback may not remove features.
    pub unsafe fn open_with_callback<'a>(
        &self,
        features: wgt::Features,
        limits: &wgt::Limits,
        memory_hints: &wgt::MemoryHints,
        callback: Option<Box<super::CreateDeviceCallback<'a>>>,
    ) -> Result<crate::OpenDevice<super::Api>, crate::DeviceError> {
        let mut enabled_extensions = self.required_device_extensions(features);
        let mut enabled_phd_features = self.physical_device_features(&enabled_extensions, features);

        let default_family_index = 0;
        let mut family_infos = vec![vk::DeviceQueueCreateInfo::default()
            .queue_family_index(default_family_index)
            .queue_priorities(&[1.0])];

        let mut pre_info = vk::DeviceCreateInfo::default();

        let called_back = callback.is_some();
        if let Some(callback) = callback {
            callback(super::CreateDeviceCallbackArgs {
                extensions: &mut enabled_extensions,
                device_features: &mut enabled_phd_features,
                queue_create_infos: &mut family_infos,
                create_info: &mut pre_info,
                _phantom: PhantomData,
            })
        }

        // The device would give each of `features`; checked before it is created, so that a
        // refusal leaves nothing behind. Without a callback, a feature it would not give is a
        // mismatch between what the adapter offers and what wgpu-hal enables for it.
        let family_index = family_infos[0].queue_family_index;
        let queue_family_properties = self.queue_family_properties(family_index)?;
        let lacking = features.difference(self.features_of_device(
            &enabled_extensions,
            &enabled_phd_features,
            &queue_family_properties,
        ));
        if !lacking.is_empty() {
            log::error!(
                "Not opening a device that would lack {lacking:?}: the extensions \
                 {enabled_extensions:?}, the features and the queue family {family_index} \
                 that {} do not give it",
                if called_back {
                    "the device creation callback left"
                } else {
                    "wgpu-hal would enable"
                }
            );
            debug_assert!(
                called_back,
                "wgpu-hal's own opening does not give {lacking:?}"
            );
            return Err(crate::DeviceError::Unexpected);
        }

        let str_pointers = enabled_extensions
            .iter()
            .map(|&s| {
                // Safe because `enabled_extensions` entries have static lifetime.
                s.as_ptr()
            })
            .collect::<Vec<_>>();

        let pre_info = pre_info
            .queue_create_infos(&family_infos)
            .enabled_extension_names(&str_pointers);
        let info = enabled_phd_features.add_to_device_create(pre_info);
        let raw_device = {
            profiling::scope!("vkCreateDevice");
            unsafe {
                self.instance
                    .raw
                    .create_device(self.raw, &info, None)
                    .map_err(map_err)?
            }
        };
        fn map_err(err: vk::Result) -> crate::DeviceError {
            match err {
                vk::Result::ERROR_TOO_MANY_OBJECTS => crate::DeviceError::OutOfMemory,
                vk::Result::ERROR_INITIALIZATION_FAILED => crate::DeviceError::Lost,
                vk::Result::ERROR_EXTENSION_NOT_PRESENT | vk::Result::ERROR_FEATURE_NOT_PRESENT => {
                    crate::hal_usage_error(err)
                }
                other => super::map_host_device_oom_and_lost_err(other),
            }
        }

        // SAFETY: Created just now from this adapter, with these extensions and features, which
        // give each of `features` (above).
        let opened = unsafe {
            self.device_from_raw_unchecked(
                raw_device.clone(),
                None,
                &enabled_extensions,
                &enabled_phd_features,
                features,
                limits,
                memory_hints,
                super::DeviceQueue::new(family_index, 0).create_flags(family_infos[0].flags),
                queue_family_properties,
            )
        };
        if opened.is_err() {
            // Not taken: wgpu-hal created it, and destroys it.
            unsafe { raw_device.destroy_device(None) };
        }
        opened
    }
}

impl crate::Adapter for super::Adapter {
    type A = super::Api;

    unsafe fn open(
        &self,
        features: wgt::Features,
        limits: &wgt::Limits,
        memory_hints: &wgt::MemoryHints,
    ) -> Result<crate::OpenDevice<super::Api>, crate::DeviceError> {
        unsafe { self.open_with_callback(features, limits, memory_hints, None) }
    }

    unsafe fn texture_format_capabilities(
        &self,
        format: wgt::TextureFormat,
    ) -> crate::TextureFormatCapabilities {
        use crate::TextureFormatCapabilities as Tfc;

        let vk_format = self.private_caps.map_texture_format(format);
        let properties = unsafe {
            self.instance
                .raw
                .get_physical_device_format_properties(self.raw, vk_format)
        };
        let features = properties.optimal_tiling_features;

        let mut flags = Tfc::empty();
        flags.set(
            Tfc::SAMPLED,
            features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE),
        );
        flags.set(
            Tfc::SAMPLED_LINEAR,
            features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR),
        );
        // flags.set(
        //     Tfc::SAMPLED_MINMAX,
        //     features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_MINMAX),
        // );
        flags.set(
            Tfc::STORAGE_READ_WRITE
                | Tfc::STORAGE_WRITE_ONLY
                | Tfc::STORAGE_READ_ONLY
                | Tfc::STORAGE_ATOMIC,
            features.contains(vk::FormatFeatureFlags::STORAGE_IMAGE),
        );
        flags.set(
            Tfc::STORAGE_ATOMIC,
            features.contains(vk::FormatFeatureFlags::STORAGE_IMAGE_ATOMIC),
        );
        flags.set(
            Tfc::COLOR_ATTACHMENT,
            features.contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT),
        );
        flags.set(
            Tfc::COLOR_ATTACHMENT_BLEND,
            features.contains(vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND),
        );
        flags.set(
            Tfc::DEPTH_STENCIL_ATTACHMENT,
            features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT),
        );
        flags.set(
            Tfc::COPY_SRC,
            features.intersects(vk::FormatFeatureFlags::TRANSFER_SRC),
        );
        flags.set(
            Tfc::COPY_DST,
            features.intersects(vk::FormatFeatureFlags::TRANSFER_DST),
        );
        flags.set(
            Tfc::STORAGE_ATOMIC,
            features.intersects(vk::FormatFeatureFlags::STORAGE_IMAGE_ATOMIC),
        );
        // Vulkan is very permissive about MSAA
        flags.set(Tfc::MULTISAMPLE_RESOLVE, !format.is_compressed());

        // get the supported sample counts
        let format_aspect = crate::FormatAspects::from(format);
        let limits = self.phd_capabilities.properties.limits;

        let sample_flags = if format_aspect.contains(crate::FormatAspects::DEPTH) {
            limits
                .framebuffer_depth_sample_counts
                .min(limits.sampled_image_depth_sample_counts)
        } else if format_aspect.contains(crate::FormatAspects::STENCIL) {
            limits
                .framebuffer_stencil_sample_counts
                .min(limits.sampled_image_stencil_sample_counts)
        } else {
            let first_aspect = format_aspect
                .iter()
                .next()
                .expect("All texture should at least one aspect")
                .map();

            // We should never get depth or stencil out of this, due to the above.
            assert_ne!(first_aspect, wgt::TextureAspect::DepthOnly);
            assert_ne!(first_aspect, wgt::TextureAspect::StencilOnly);

            match format.sample_type(Some(first_aspect), None).unwrap() {
                wgt::TextureSampleType::Float { .. } => limits
                    .framebuffer_color_sample_counts
                    .min(limits.sampled_image_color_sample_counts),
                wgt::TextureSampleType::Sint | wgt::TextureSampleType::Uint => {
                    limits.sampled_image_integer_sample_counts
                }
                _ => unreachable!(),
            }
        };

        flags.set(
            Tfc::MULTISAMPLE_X2,
            sample_flags.contains(vk::SampleCountFlags::TYPE_2),
        );
        flags.set(
            Tfc::MULTISAMPLE_X4,
            sample_flags.contains(vk::SampleCountFlags::TYPE_4),
        );
        flags.set(
            Tfc::MULTISAMPLE_X8,
            sample_flags.contains(vk::SampleCountFlags::TYPE_8),
        );
        flags.set(
            Tfc::MULTISAMPLE_X16,
            sample_flags.contains(vk::SampleCountFlags::TYPE_16),
        );

        flags
    }

    unsafe fn surface_capabilities(
        &self,
        surface: &super::Surface,
    ) -> Option<crate::SurfaceCapabilities> {
        surface.inner.surface_capabilities(self)
    }

    unsafe fn surface_display_hdr_info(
        &self,
        surface: &super::Surface,
    ) -> Option<wgt::DisplayHdrInfo> {
        // Vulkan has no portable luminance query; the Win32 surface reads it
        // through DXGI (see `dxgi::hdr`). Every other surface reports `None`.
        surface.inner.display_hdr_info()
    }

    unsafe fn get_presentation_timestamp(&self) -> wgt::PresentationTimestamp {
        // VK_GOOGLE_display_timing is the only way to get presentation
        // timestamps on vulkan right now and it is only ever available
        // on android and linux. This includes mac, but there's no alternative
        // on mac, so this is fine.
        #[cfg(unix)]
        {
            let mut timespec = libc::timespec::default();
            unsafe {
                libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut timespec);
            }

            wgt::PresentationTimestamp(
                timespec.tv_sec as u128 * 1_000_000_000 + timespec.tv_nsec as u128,
            )
        }
        #[cfg(not(unix))]
        {
            wgt::PresentationTimestamp::INVALID_TIMESTAMP
        }
    }

    fn get_ordered_buffer_usages(&self) -> wgt::BufferUses {
        wgt::BufferUses::INCLUSIVE | wgt::BufferUses::MAP_WRITE
    }

    // Vulkan makes very few execution ordering guarantees
    // see https://registry.khronos.org/vulkan/specs/latest/html/vkspec.html#synchronization-implicit
    // We just don't want to insert barriers between inclusive uses
    // See https://github.com/gfx-rs/wgpu/issues/8853
    fn get_ordered_texture_usages(&self) -> wgt::TextureUses {
        wgt::TextureUses::INCLUSIVE
    }
}

fn is_format_16bit_norm_supported(instance: &ash::Instance, phd: vk::PhysicalDevice) -> bool {
    [
        vk::Format::R16_UNORM,
        vk::Format::R16_SNORM,
        vk::Format::R16G16_UNORM,
        vk::Format::R16G16_SNORM,
        vk::Format::R16G16B16A16_UNORM,
        vk::Format::R16G16B16A16_SNORM,
    ]
    .into_iter()
    .all(|format| {
        supports_format(
            instance,
            phd,
            format,
            vk::ImageTiling::OPTIMAL,
            vk::FormatFeatureFlags::SAMPLED_IMAGE
                | vk::FormatFeatureFlags::STORAGE_IMAGE
                | vk::FormatFeatureFlags::TRANSFER_SRC
                | vk::FormatFeatureFlags::TRANSFER_DST,
        )
    })
}

fn is_float32_filterable_supported(instance: &ash::Instance, phd: vk::PhysicalDevice) -> bool {
    [
        vk::Format::R32_SFLOAT,
        vk::Format::R32G32_SFLOAT,
        vk::Format::R32G32B32A32_SFLOAT,
    ]
    .into_iter()
    .all(|format| {
        supports_format(
            instance,
            phd,
            format,
            vk::ImageTiling::OPTIMAL,
            vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR,
        )
    })
}

fn is_float32_blendable_supported(instance: &ash::Instance, phd: vk::PhysicalDevice) -> bool {
    [
        vk::Format::R32_SFLOAT,
        vk::Format::R32G32_SFLOAT,
        vk::Format::R32G32B32A32_SFLOAT,
    ]
    .into_iter()
    .all(|format| {
        supports_format(
            instance,
            phd,
            format,
            vk::ImageTiling::OPTIMAL,
            vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND,
        )
    })
}

pub(super) fn supports_format(
    instance: &ash::Instance,
    phd: vk::PhysicalDevice,
    format: vk::Format,
    tiling: vk::ImageTiling,
    features: vk::FormatFeatureFlags,
) -> bool {
    let properties = unsafe { instance.get_physical_device_format_properties(phd, format) };
    match tiling {
        vk::ImageTiling::LINEAR => properties.linear_tiling_features.contains(features),
        vk::ImageTiling::OPTIMAL => properties.optimal_tiling_features.contains(features),
        _ => false,
    }
}

/// The BC formats, as wgpu maps them.
const BC_FORMATS: [vk::Format; 14] = [
    vk::Format::BC1_RGBA_UNORM_BLOCK,
    vk::Format::BC1_RGBA_SRGB_BLOCK,
    vk::Format::BC2_UNORM_BLOCK,
    vk::Format::BC2_SRGB_BLOCK,
    vk::Format::BC3_UNORM_BLOCK,
    vk::Format::BC3_SRGB_BLOCK,
    vk::Format::BC4_UNORM_BLOCK,
    vk::Format::BC4_SNORM_BLOCK,
    vk::Format::BC5_UNORM_BLOCK,
    vk::Format::BC5_SNORM_BLOCK,
    vk::Format::BC6H_UFLOAT_BLOCK,
    vk::Format::BC6H_SFLOAT_BLOCK,
    vk::Format::BC7_UNORM_BLOCK,
    vk::Format::BC7_SRGB_BLOCK,
];

/// The ASTC LDR formats.
const ASTC_LDR_FORMATS: [vk::Format; 28] = [
    vk::Format::ASTC_4X4_UNORM_BLOCK,
    vk::Format::ASTC_4X4_SRGB_BLOCK,
    vk::Format::ASTC_5X4_UNORM_BLOCK,
    vk::Format::ASTC_5X4_SRGB_BLOCK,
    vk::Format::ASTC_5X5_UNORM_BLOCK,
    vk::Format::ASTC_5X5_SRGB_BLOCK,
    vk::Format::ASTC_6X5_UNORM_BLOCK,
    vk::Format::ASTC_6X5_SRGB_BLOCK,
    vk::Format::ASTC_6X6_UNORM_BLOCK,
    vk::Format::ASTC_6X6_SRGB_BLOCK,
    vk::Format::ASTC_8X5_UNORM_BLOCK,
    vk::Format::ASTC_8X5_SRGB_BLOCK,
    vk::Format::ASTC_8X6_UNORM_BLOCK,
    vk::Format::ASTC_8X6_SRGB_BLOCK,
    vk::Format::ASTC_8X8_UNORM_BLOCK,
    vk::Format::ASTC_8X8_SRGB_BLOCK,
    vk::Format::ASTC_10X5_UNORM_BLOCK,
    vk::Format::ASTC_10X5_SRGB_BLOCK,
    vk::Format::ASTC_10X6_UNORM_BLOCK,
    vk::Format::ASTC_10X6_SRGB_BLOCK,
    vk::Format::ASTC_10X8_UNORM_BLOCK,
    vk::Format::ASTC_10X8_SRGB_BLOCK,
    vk::Format::ASTC_10X10_UNORM_BLOCK,
    vk::Format::ASTC_10X10_SRGB_BLOCK,
    vk::Format::ASTC_12X10_UNORM_BLOCK,
    vk::Format::ASTC_12X10_SRGB_BLOCK,
    vk::Format::ASTC_12X12_UNORM_BLOCK,
    vk::Format::ASTC_12X12_SRGB_BLOCK,
];

/// Whether every one of `formats` has 3D images with optimal tiling on `phd` as wgpu-hal creates
/// them for its textures (VUID-VkImageCreateInfo-imageCreateMaxMipLevels-02251): sampled and
/// copied, and with `MUTABLE_FORMAT` where they have view formats. A successful query also
/// guarantees extents up to `maxImageDimension3D` ("Allowed Extent Values").
fn supports_3d_images(
    instance: &ash::Instance,
    phd: vk::PhysicalDevice,
    formats: &[vk::Format],
) -> bool {
    let usage = vk::ImageUsageFlags::SAMPLED
        | vk::ImageUsageFlags::TRANSFER_SRC
        | vk::ImageUsageFlags::TRANSFER_DST;
    formats.iter().all(|&format| {
        [
            vk::ImageCreateFlags::empty(),
            vk::ImageCreateFlags::MUTABLE_FORMAT,
        ]
        .into_iter()
        .all(|flags| {
            unsafe {
                instance.get_physical_device_image_format_properties(
                    phd,
                    format,
                    vk::ImageType::TYPE_3D,
                    vk::ImageTiling::OPTIMAL,
                    usage,
                    flags,
                )
            }
            .is_ok()
        })
    })
}

fn supports_bgra8unorm_storage(
    instance: &ash::Instance,
    phd: vk::PhysicalDevice,
    device_api_version: u32,
) -> bool {
    // See https://github.com/KhronosGroup/Vulkan-Docs/issues/2027#issuecomment-1380608011

    // This check gates the function call and structures used below.
    // TODO: check for (`VK_KHR_get_physical_device_properties2` or VK1.1) and (`VK_KHR_format_feature_flags2` or VK1.3).
    // Right now we only check for VK1.3.
    if device_api_version < vk::API_VERSION_1_3 {
        return false;
    }

    unsafe {
        let mut properties3 = vk::FormatProperties3::default();
        let mut properties2 = vk::FormatProperties2::default().push_next(&mut properties3);

        instance.get_physical_device_format_properties2(
            phd,
            vk::Format::B8G8R8A8_UNORM,
            &mut properties2,
        );

        let features2 = properties2.format_properties.optimal_tiling_features;
        let features3 = properties3.optimal_tiling_features;

        features2.contains(vk::FormatFeatureFlags::STORAGE_IMAGE)
            && features3.contains(vk::FormatFeatureFlags2::STORAGE_WRITE_WITHOUT_FORMAT)
    }
}

// For https://github.com/gfx-rs/wgpu/issues/4599
// Intel iGPUs with outdated drivers can break rendering if `VK_EXT_robustness2` is used.
// Driver version 31.0.101.2115 works, but there's probably an earlier functional version.
fn is_intel_igpu_outdated_for_robustness2(capabilities: &PhysicalDeviceProperties) -> bool {
    const DRIVER_VERSION_WORKING: u32 = (101 << 14) | 2115; // X.X.101.2115

    let props = &capabilities.properties;

    let is_outdated = props.vendor_id == crate::auxil::db::intel::VENDOR
        && props.device_type == vk::PhysicalDeviceType::INTEGRATED_GPU
        && props.driver_version < DRIVER_VERSION_WORKING
        && capabilities.is_driver(vk::DriverId::INTEL_PROPRIETARY_WINDOWS);

    if is_outdated {
        log::debug!(
            "Disabling robustBufferAccess2 and robustImageAccess2: IntegratedGpu Intel Driver is outdated. Found with version 0x{:X}, less than the known good version 0x{:X} (31.0.101.2115)",
            props.driver_version,
            DRIVER_VERSION_WORKING
        );
    }
    is_outdated
}

/// Convert Vulkan component type to wgt::CooperativeScalarType.
fn map_vk_component_type(ty: vk::ComponentTypeKHR) -> Option<wgt::CooperativeScalarType> {
    match ty {
        vk::ComponentTypeKHR::FLOAT16 => Some(wgt::CooperativeScalarType::F16),
        vk::ComponentTypeKHR::FLOAT32 => Some(wgt::CooperativeScalarType::F32),
        vk::ComponentTypeKHR::SINT32 => Some(wgt::CooperativeScalarType::I32),
        vk::ComponentTypeKHR::UINT32 => Some(wgt::CooperativeScalarType::U32),
        _ => None,
    }
}

/// Convert Vulkan matrix size.
fn map_vk_cooperative_size(size: u32) -> Option<u32> {
    match size {
        8 | 16 => Some(size),
        _ => None,
    }
}

/// Query all supported cooperative matrix configurations from Vulkan.
fn query_cooperative_matrix_properties(
    coop_matrix: &khr::cooperative_matrix::Instance,
    phd: vk::PhysicalDevice,
) -> Vec<wgt::CooperativeMatrixProperties> {
    let vk_properties =
        match unsafe { coop_matrix.get_physical_device_cooperative_matrix_properties(phd) } {
            Ok(props) => props,
            Err(e) => {
                log::warn!("Failed to query cooperative matrix properties: {e:?}");
                return Vec::new();
            }
        };

    log::debug!(
        "Vulkan reports {} cooperative matrix configurations",
        vk_properties.len()
    );

    let mut result = Vec::new();
    for prop in &vk_properties {
        log::debug!(
            "  Vulkan coop matrix: M={} N={} K={} A={:?} B={:?} C={:?} Result={:?} scope={:?} saturating={}",
            prop.m_size,
            prop.n_size,
            prop.k_size,
            prop.a_type,
            prop.b_type,
            prop.c_type,
            prop.result_type,
            prop.scope,
            prop.saturating_accumulation
        );

        // Only include subgroup-scoped operations (the only scope we support)
        if prop.scope != vk::ScopeKHR::SUBGROUP {
            log::debug!("    Skipped: scope is not SUBGROUP");
            continue;
        }

        // Map sizes - skip configurations with sizes we don't support
        let m_size = match map_vk_cooperative_size(prop.m_size) {
            Some(s) => s,
            None => {
                log::debug!("    Skipped: M size {} not supported", prop.m_size);
                continue;
            }
        };
        let n_size = match map_vk_cooperative_size(prop.n_size) {
            Some(s) => s,
            None => {
                log::debug!("    Skipped: N size {} not supported", prop.n_size);
                continue;
            }
        };
        let k_size = match map_vk_cooperative_size(prop.k_size) {
            Some(s) => s,
            None => {
                log::debug!("    Skipped: K size {} not supported", prop.k_size);
                continue;
            }
        };

        // Map the component types - A and B must match, C and Result must match
        let ab_type = match map_vk_component_type(prop.a_type) {
            Some(t) if Some(t) == map_vk_component_type(prop.b_type) => t,
            _ => {
                log::debug!(
                    "    Skipped: A/B types {:?}/{:?} not supported or don't match",
                    prop.a_type,
                    prop.b_type
                );
                continue;
            }
        };
        let cr_type = match map_vk_component_type(prop.c_type) {
            Some(t) if Some(t) == map_vk_component_type(prop.result_type) => t,
            _ => {
                log::debug!(
                    "    Skipped: C/Result types {:?}/{:?} not supported or don't match",
                    prop.c_type,
                    prop.result_type
                );
                continue;
            }
        };

        log::debug!("    Accepted!");
        result.push(wgt::CooperativeMatrixProperties {
            m_size,
            n_size,
            k_size,
            ab_type,
            cr_type,
            saturating_accumulation: prop.saturating_accumulation != 0,
        });
    }

    log::debug!(
        "Found {} cooperative matrix configurations supported by wgpu",
        result.len()
    );
    result
}

mod feature_structures;

#[cfg(test)]
mod tests;
