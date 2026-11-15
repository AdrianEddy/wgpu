//! The members of the feature structures that wgpu-hal reads from a device's creation chain, as
//! `ash` 0.38 defines them, or wgpu-hal where `ash` has none: every member of each is a
//! `vk::Bool32` feature.

use ash::vk;

/// A feature structure whose members are all `vk::Bool32` features.
pub(super) trait FeatureStructure: vk::TaggedStructure + Copy + Default {
    /// Enables in `self` each feature that `other` enables.
    fn enable(&mut self, other: &Self);
}

macro_rules! feature_structures {
    ($($structure:ident { $($member:ident),* $(,)? })*) => {$(
        impl FeatureStructure for vk::$structure<'static> {
            fn enable(&mut self, other: &Self) {
                $(self.$member |= other.$member;)*
            }
        }
    )*};
}

/// Enables in `features` each core feature that `other` enables.
pub(super) fn enable_core_features(
    features: &mut vk::PhysicalDeviceFeatures,
    other: &vk::PhysicalDeviceFeatures,
) {
    macro_rules! enable {
        ($($member:ident),* $(,)?) => {
            $(features.$member |= other.$member;)*
        };
    }
    enable!(
        robust_buffer_access,
        full_draw_index_uint32,
        image_cube_array,
        independent_blend,
        geometry_shader,
        tessellation_shader,
        sample_rate_shading,
        dual_src_blend,
        logic_op,
        multi_draw_indirect,
        draw_indirect_first_instance,
        depth_clamp,
        depth_bias_clamp,
        fill_mode_non_solid,
        depth_bounds,
        wide_lines,
        large_points,
        alpha_to_one,
        multi_viewport,
        sampler_anisotropy,
        texture_compression_etc2,
        texture_compression_astc_ldr,
        texture_compression_bc,
        occlusion_query_precise,
        pipeline_statistics_query,
        vertex_pipeline_stores_and_atomics,
        fragment_stores_and_atomics,
        shader_tessellation_and_geometry_point_size,
        shader_image_gather_extended,
        shader_storage_image_extended_formats,
        shader_storage_image_multisample,
        shader_storage_image_read_without_format,
        shader_storage_image_write_without_format,
        shader_uniform_buffer_array_dynamic_indexing,
        shader_sampled_image_array_dynamic_indexing,
        shader_storage_buffer_array_dynamic_indexing,
        shader_storage_image_array_dynamic_indexing,
        shader_clip_distance,
        shader_cull_distance,
        shader_float64,
        shader_int64,
        shader_int16,
        shader_resource_residency,
        shader_resource_min_lod,
        sparse_binding,
        sparse_residency_buffer,
        sparse_residency_image2_d,
        sparse_residency_image3_d,
        sparse_residency2_samples,
        sparse_residency4_samples,
        sparse_residency8_samples,
        sparse_residency16_samples,
        sparse_residency_aliased,
        variable_multisample_rate,
        inherited_queries,
    );
}

feature_structures! {
    PhysicalDeviceDescriptorIndexingFeaturesEXT {
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
        descriptor_binding_update_unused_while_pending, descriptor_binding_partially_bound,
        descriptor_binding_variable_descriptor_count, runtime_descriptor_array,
    }
    PhysicalDeviceTimelineSemaphoreFeaturesKHR {
        timeline_semaphore,
    }
    PhysicalDeviceImageRobustnessFeaturesEXT {
        robust_image_access,
    }
    PhysicalDeviceRobustness2FeaturesEXT {
        robust_buffer_access2, robust_image_access2, null_descriptor,
    }
    PhysicalDeviceMultiviewFeaturesKHR {
        multiview, multiview_geometry_shader, multiview_tessellation_shader,
    }
    PhysicalDeviceTextureCompressionASTCHDRFeaturesEXT {
        texture_compression_astc_hdr,
    }
    PhysicalDeviceShaderFloat16Int8Features {
        shader_float16, shader_int8,
    }
    PhysicalDevice16BitStorageFeatures {
        storage_buffer16_bit_access, uniform_and_storage_buffer16_bit_access,
        storage_push_constant16, storage_input_output16,
    }
    PhysicalDeviceAccelerationStructureFeaturesKHR {
        acceleration_structure, acceleration_structure_capture_replay,
        acceleration_structure_indirect_build, acceleration_structure_host_commands,
        descriptor_binding_acceleration_structure_update_after_bind,
    }
    PhysicalDeviceBufferDeviceAddressFeaturesKHR {
        buffer_device_address, buffer_device_address_capture_replay,
        buffer_device_address_multi_device,
    }
    PhysicalDeviceRayQueryFeaturesKHR {
        ray_query,
    }
    PhysicalDeviceRayTracingPipelineFeaturesKHR {
        ray_tracing_pipeline, ray_tracing_pipeline_shader_group_handle_capture_replay,
        ray_tracing_pipeline_shader_group_handle_capture_replay_mixed,
        ray_tracing_pipeline_trace_rays_indirect, ray_traversal_primitive_culling,
    }
    PhysicalDeviceZeroInitializeWorkgroupMemoryFeatures {
        shader_zero_initialize_workgroup_memory,
    }
    PhysicalDeviceRayTracingPositionFetchFeaturesKHR {
        ray_tracing_position_fetch,
    }
    PhysicalDeviceShaderAtomicInt64Features {
        shader_buffer_int64_atomics, shader_shared_int64_atomics,
    }
    PhysicalDeviceShaderImageAtomicInt64FeaturesEXT {
        shader_image_int64_atomics, sparse_image_int64_atomics,
    }
    PhysicalDeviceShaderAtomicFloatFeaturesEXT {
        shader_buffer_float32_atomics, shader_buffer_float32_atomic_add,
        shader_buffer_float64_atomics, shader_buffer_float64_atomic_add,
        shader_shared_float32_atomics, shader_shared_float32_atomic_add,
        shader_shared_float64_atomics, shader_shared_float64_atomic_add,
        shader_image_float32_atomics, shader_image_float32_atomic_add, sparse_image_float32_atomics,
        sparse_image_float32_atomic_add,
    }
    PhysicalDeviceSubgroupSizeControlFeatures {
        subgroup_size_control, compute_full_subgroups,
    }
    PhysicalDeviceMaintenance4FeaturesKHR {
        maintenance4,
    }
    PhysicalDeviceMeshShaderFeaturesEXT {
        task_shader, mesh_shader, multiview_mesh_shader,
        primitive_fragment_shading_rate_mesh_shader, mesh_shader_queries,
    }
    PhysicalDeviceShaderIntegerDotProductFeaturesKHR {
        shader_integer_dot_product,
    }
    PhysicalDeviceFragmentShaderBarycentricFeaturesKHR {
        fragment_shader_barycentric,
    }
    PhysicalDevicePortabilitySubsetFeaturesKHR {
        constant_alpha_color_blend_factors, events, image_view_format_reinterpretation,
        image_view_format_swizzle, image_view2_d_on3_d_image, multisample_array_image,
        mutable_comparison_samplers, point_polygons, sampler_mip_lod_bias,
        separate_stencil_mask_ref, shader_sample_rate_interpolation_functions,
        tessellation_isolines, tessellation_point_mode, triangle_fans,
        vertex_attribute_access_beyond_stride,
    }
    PhysicalDeviceCooperativeMatrixFeaturesKHR {
        cooperative_matrix, cooperative_matrix_robust_buffer_access,
    }
    PhysicalDeviceVulkanMemoryModelFeaturesKHR {
        vulkan_memory_model, vulkan_memory_model_device_scope,
        vulkan_memory_model_availability_visibility_chains,
    }
    PhysicalDeviceShaderDrawParametersFeatures {
        shader_draw_parameters,
    }
    PhysicalDeviceSwapchainMaintenance1FeaturesEXT {
        swapchain_maintenance1,
    }
}

/// `VK_EXT_ray_tracing_invocation_reorder`'s, which `ash` 0.38 does not define.
impl FeatureStructure for super::ray_tracing_invocation_reorder::FeaturesEXT {
    fn enable(&mut self, other: &Self) {
        self.ray_tracing_invocation_reorder |= other.ray_tracing_invocation_reorder;
    }
}
