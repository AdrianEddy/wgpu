//! A device created from a raw Vulkan device without a core feature behind a downlevel flag opens
//! without the flag, and wgpu-core validates its use against the device's flags: it refuses what
//! would use the feature, naming the flag, and takes the path that the device has where there is
//! one.

use ash::vk;
use wgpu_hal::{
    api::Vulkan,
    vulkan::{DeviceQueue, PhysicalDeviceFeatures},
};

/// The core features of `adapter` behind its downlevel flags, those it offers, and those that
/// FFmpeg copies into its devices (`device_features_copy_needed` in
/// `libavutil/hwcontext_vulkan.c`), those it offers.
fn offered(adapter: &wgpu::Adapter) -> (vk::PhysicalDeviceFeatures, vk::PhysicalDeviceFeatures) {
    // SAFETY: Used to read the adapter's features.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
    let core = hal.get_physical_device_features().get_core();
    let downlevel = vk::PhysicalDeviceFeatures::default()
        .independent_blend(core.independent_blend != 0)
        .sample_rate_shading(core.sample_rate_shading != 0)
        .image_cube_array(core.image_cube_array != 0)
        .sampler_anisotropy(core.sampler_anisotropy != 0)
        .fragment_stores_and_atomics(core.fragment_stores_and_atomics != 0)
        .depth_bias_clamp(core.depth_bias_clamp != 0)
        .full_draw_index_uint32(core.full_draw_index_uint32 != 0);
    let ffmpeg = vk::PhysicalDeviceFeatures::default()
        .shader_image_gather_extended(core.shader_image_gather_extended != 0)
        .shader_storage_image_read_without_format(
            core.shader_storage_image_read_without_format != 0,
        )
        .shader_storage_image_write_without_format(
            core.shader_storage_image_write_without_format != 0,
        )
        .fragment_stores_and_atomics(core.fragment_stores_and_atomics != 0)
        .vertex_pipeline_stores_and_atomics(core.vertex_pipeline_stores_and_atomics != 0)
        .shader_int64(core.shader_int64 != 0)
        .shader_int16(core.shader_int16 != 0)
        .shader_float64(core.shader_float64 != 0);
    (downlevel, ffmpeg)
}

/// Opens a device on `adapter` from a raw device created, as a creator other than wgpu-hal
/// creates one, with wgpu-hal's extensions for no feature and the core features `core`, on queue 0
/// of family 0.
fn open(adapter: &wgpu::Adapter, core: vk::PhysicalDeviceFeatures) -> (wgpu::Device, wgpu::Queue) {
    let open = {
        // SAFETY: Used to create and open a device.
        let hal = unsafe { adapter.as_hal::<Vulkan>() }.unwrap();
        let extensions = hal.required_device_extensions(wgpu::Features::empty());
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(0)
            .queue_priorities(&priorities)];
        let names = extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect::<Vec<_>>();
        let enabled = vk::PhysicalDeviceFeatures2::default().features(core);
        let mut info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&names);
        let chain: *const vk::PhysicalDeviceFeatures2<'_> = &enabled;
        info.p_next = chain.cast();
        // SAFETY: A valid create info, whose extensions and features the adapter offers.
        let raw = unsafe {
            hal.shared_instance().raw_instance().create_device(
                hal.raw_physical_device(),
                &info,
                None,
            )
        }
        .expect("vkCreateDevice failed");
        // SAFETY: No chain.
        let enabled_features = unsafe { PhysicalDeviceFeatures::from_enabled(&enabled) };
        // SAFETY: Created above with these extensions and features; wgpu-hal destroys it.
        unsafe {
            hal.device_from_raw(
                raw,
                None,
                &extensions,
                &enabled_features,
                wgpu::Features::empty(),
                &adapter.limits(),
                &wgpu::MemoryHints::default(),
                DeviceQueue::new(0, 0),
            )
        }
        .unwrap()
    };
    // SAFETY: Opened from this adapter, with no feature.
    unsafe { adapter.create_device_from_hal(open, &wgpu::DeviceDescriptor::default()) }.unwrap()
}

/// The validation error, if any, of what `create` does on `device`.
fn validation_error(device: &wgpu::Device, create: impl FnOnce()) -> Option<String> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    create();
    pollster::block_on(scope.pop()).map(|error| error.to_string())
}

const SHADER: &str = r"
@vertex
fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return vec4f(f32(index), 0.0, 0.0, 1.0);
}

struct Two {
    @location(0) first: vec4f,
    @location(1) second: vec4f,
}

@fragment
fn two() -> Two {
    return Two(vec4f(1.0), vec4f(1.0));
}

@fragment
fn one() -> @location(0) vec4f {
    return vec4f(1.0);
}
";

/// A render pipeline drawing `targets` with the fragment entry point `entry_point`, and with
/// `depth_stencil`.
fn pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    entry_point: &str,
    targets: &[Option<wgpu::ColorTargetState>],
    depth_stencil: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            targets,
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// On each adapter with `INDEPENDENT_BLEND` and `DEPTH_BIAS_CLAMP`, a device created without
/// `independentBlend` and `depthBiasClamp` lacks their flags, and wgpu-core refuses a pipeline
/// with different blending for its two targets, and one with a depth bias clamp, naming the
/// flags; it takes the same pipelines without them. Before, wgpu-core validated the device against
/// its adapter's flags, took both pipelines, and wgpu-hal created them with features the device
/// lacks (VUID-VkPipelineColorBlendStateCreateInfo-pAttachments-00605,
/// VUID-VkGraphicsPipelineCreateInfo-pDynamicStates-00754).
#[test]
fn a_pipeline_needing_a_downlevel_feature_the_device_lacks_is_refused() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let Some((instance, _messenger)) = super::instance(wgpu::InstanceFlags::empty()) else {
        return;
    };
    let needed = wgpu::DownlevelFlags::INDEPENDENT_BLEND | wgpu::DownlevelFlags::DEPTH_BIAS_CLAMP;
    let mut wrong = Vec::new();
    for adapter in super::adapters(&instance) {
        let name = adapter.get_info().name;
        if !adapter.get_downlevel_capabilities().flags.contains(needed) {
            println!("skipped {name:?}: no independent blending or depth bias clamp");
            continue;
        }
        let (core, _) = offered(&adapter);
        let (device, _queue) = open(
            &adapter,
            core.independent_blend(false).depth_bias_clamp(false),
        );
        let flags = device.downlevel_capabilities().flags;
        println!("{name:?}: {flags:?}");
        if flags.intersects(needed) {
            wrong.push(format!("{name:?}: the device has {flags:?}"));
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let target = |blend| {
            Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let depth = |clamp| {
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp,
                },
            })
        };
        let blends = [
            target(Some(wgpu::BlendState::REPLACE)),
            target(Some(wgpu::BlendState::ALPHA_BLENDING)),
        ];
        let same = [
            target(Some(wgpu::BlendState::REPLACE)),
            target(Some(wgpu::BlendState::REPLACE)),
        ];
        for (case, flag, error) in [
            (
                "different blending",
                "INDEPENDENT_BLEND",
                validation_error(&device, || {
                    pipeline(&device, &module, "two", &blends, None);
                }),
            ),
            (
                "a depth bias clamp",
                "DEPTH_BIAS_CLAMP",
                validation_error(&device, || {
                    pipeline(&device, &module, "one", &same[..1], depth(0.5));
                }),
            ),
        ] {
            println!("{name:?}: {case}: {error:?}");
            if !error.as_deref().is_some_and(|error| error.contains(flag)) {
                wrong.push(format!("{name:?}: {case} without {flag}: {error:?}"));
            }
        }
        for (case, error) in [
            (
                "the same blending",
                validation_error(&device, || {
                    pipeline(&device, &module, "two", &same, None);
                }),
            ),
            (
                "no depth bias clamp",
                validation_error(&device, || {
                    pipeline(&device, &module, "one", &same[..1], depth(0.0));
                }),
            ),
        ] {
            if let Some(error) = error {
                wrong.push(format!("{name:?}: {case} refused: {error}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// On each adapter, a device created with FFmpeg's core features opens through wgpu without the
/// six flags of the downlevel features that FFmpeg leaves out, and works under the validation
/// layer: a sampler that asks for anisotropic filtering takes the path without it, as wgpu-core
/// clamps the anisotropy of a device without `ANISOTROPIC_FILTERING`. Before, wgpu-core gave the
/// device its adapter's flags, and wgpu-hal enabled anisotropy without `samplerAnisotropy`
/// (VUID-VkSamplerCreateInfo-anisotropyEnable-01070).
#[test]
fn a_device_with_ffmpegs_features_works_through_wgpu() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let flags = wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG;
    let Some((instance, messenger)) = super::instance(flags) else {
        return;
    };
    let left_out = wgpu::DownlevelFlags::INDEPENDENT_BLEND
        | wgpu::DownlevelFlags::MULTISAMPLED_SHADING
        | wgpu::DownlevelFlags::CUBE_ARRAY_TEXTURES
        | wgpu::DownlevelFlags::ANISOTROPIC_FILTERING
        | wgpu::DownlevelFlags::DEPTH_BIAS_CLAMP
        | wgpu::DownlevelFlags::FULL_DRAW_INDEX_UINT32;
    let mut wrong = Vec::new();
    for adapter in super::adapters(&instance) {
        let name = adapter.get_info().name;
        let (_, ffmpeg) = offered(&adapter);
        let (device, queue) = open(&adapter, ffmpeg);
        let adapter_flags = adapter.get_downlevel_capabilities().flags;
        let flags = device.downlevel_capabilities().flags;
        println!("{name:?}: {flags:?}");
        if flags != adapter_flags - left_out {
            wrong.push(format!(
                "{name:?}: the device has {flags:?}, expected {:?}",
                adapter_flags - left_out
            ));
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, &[1; 256]);
        buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert_eq!(buffer.slice(..).get_mapped_range().unwrap()[0], 1);
        drop((sampler, buffer, queue, device));
        match &messenger {
            Some(messenger) => {
                let messages = messenger.take();
                if !messages.is_empty() {
                    wrong.push(format!("{name:?}: {messages:#?}"));
                }
            }
            None => println!("{name:?}: no validation layer"),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// On each adapter, a device created with FFmpeg's features, without `independentBlend`, takes
/// pipelines with a hole among their colour targets, before or after the target, and the
/// validation layer reports nothing: the hole has the target's blend state. Before, wgpu-hal gave
/// it the default state, and the attachments differed
/// (VUID-VkPipelineColorBlendStateCreateInfo-pAttachments-00605).
#[test]
fn a_colour_target_hole_takes_the_first_targets_blend_state() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let flags = wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG;
    let Some((instance, messenger)) = super::instance(flags) else {
        return;
    };
    let mut wrong = Vec::new();
    for adapter in super::adapters(&instance) {
        let name = adapter.get_info().name;
        let (_, ffmpeg) = offered(&adapter);
        let (device, _queue) = open(&adapter, ffmpeg);
        assert!(!device
            .downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::INDEPENDENT_BLEND));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let target = Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        });
        for (entry_point, targets) in [
            ("one", vec![target.clone(), None]),
            ("two", vec![None, target.clone()]),
        ] {
            if let Some(error) = validation_error(&device, || {
                pipeline(&device, &module, entry_point, &targets, None);
            }) {
                wrong.push(format!("{name:?}: {targets:?} refused: {error}"));
            }
        }
        drop(device);
        match &messenger {
            Some(messenger) => {
                let messages = messenger.take();
                if !messages.is_empty() {
                    wrong.push(format!("{name:?}: {messages:#?}"));
                }
            }
            None => println!("{name:?}: no validation layer"),
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// On each adapter with `CUBE_ARRAY_TEXTURES`, a device created without `imageCubeArray` lacks the
/// flag, and wgpu-core refuses a cube array view of a cube-compatible texture, naming the flag,
/// while it takes a cube view; the validation layer reports nothing. Before, wgpu-core validated
/// no view against the flag, and wgpu-hal created the view without the feature
/// (VUID-VkImageViewCreateInfo-viewType-01004).
#[test]
fn a_cube_array_view_needs_cube_array_textures() {
    let _one = super::ONE_AT_A_TIME.lock().unwrap();
    let flags = wgpu::InstanceFlags::VALIDATION | wgpu::InstanceFlags::DEBUG;
    let Some((instance, messenger)) = super::instance(flags) else {
        return;
    };
    let mut wrong = Vec::new();
    for adapter in super::adapters(&instance) {
        let name = adapter.get_info().name;
        let flag = wgpu::DownlevelFlags::CUBE_ARRAY_TEXTURES;
        if !adapter.get_downlevel_capabilities().flags.contains(flag) {
            println!("skipped {name:?}: no cube array textures");
            continue;
        }
        let (core, _) = offered(&adapter);
        let (device, _queue) = open(&adapter, core.image_cube_array(false));
        if device.downlevel_capabilities().flags.contains(flag) {
            wrong.push(format!("{name:?}: the device has CUBE_ARRAY_TEXTURES"));
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 12,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
            texture_binding_view_dimension: None,
        });
        let view = |dimension, layers| {
            validation_error(&device, || {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(dimension),
                    array_layer_count: Some(layers),
                    ..Default::default()
                });
            })
        };
        let cube_array = view(wgpu::TextureViewDimension::CubeArray, 12);
        println!("{name:?}: cube array view: {cube_array:?}");
        if !cube_array
            .as_deref()
            .is_some_and(|error| error.contains("CUBE_ARRAY_TEXTURES"))
        {
            wrong.push(format!("{name:?}: a cube array view: {cube_array:?}"));
        }
        if let Some(error) = view(wgpu::TextureViewDimension::Cube, 6) {
            wrong.push(format!("{name:?}: a cube view refused: {error}"));
        }
        drop((texture, device));
        if let Some(messenger) = &messenger {
            let messages = messenger.take();
            if !messages.is_empty() {
                wrong.push(format!("{name:?}: {messages:#?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
