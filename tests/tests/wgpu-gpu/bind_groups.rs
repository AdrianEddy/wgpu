use std::num::NonZeroU64;

use wgpu::{util::DeviceExt, BufferUsages, PollType};
use wgpu_test::{
    apply, gpu_test, GpuTestConfiguration, GpuTestInitializer, TestParameters, TestingContext,
};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.extend([
        MULTIPLE_BINDINGS_WITH_DIFFERENT_SIZES,
        BIND_GROUP_NONFILTERING_LAYOUT_NONFILTERING_SAMPLER,
        BIND_GROUP_NONFILTERING_LAYOUT_MIN_SAMPLER,
        BIND_GROUP_NONFILTERING_LAYOUT_MAG_SAMPLER,
        BIND_GROUP_NONFILTERING_LAYOUT_MIPMAP_SAMPLER,
        BIND_GROUP_WITH_MAX_BINDING_INDEX,
        BIND_GROUP_STAGE_ORDER,
        BIND_GROUP_AFTER_A_GROUP_THAT_BECOMES_NONE,
        BIND_GROUP_AFTER_A_GROUP_THAT_BECOMES_EMPTY,
        EMPTY_BIND_GROUPS_BESIDE_A_USED_ONE,
    ]);
}

/// Create two bind groups against the same bind group layout, in the same
/// compute pass, but against two different shaders that have different binding
/// sizes. The first has binding size 8, the second has binding size 4.
///
/// Regression test for https://github.com/gfx-rs/wgpu/issues/7359.
fn multiple_bindings_with_differing_sizes(ctx: TestingContext) {
    const SHADER_SRC: &[&str] = &[
        "
        @group(0) @binding(0)
        var<uniform> buffer : vec2<f32>;

        @compute @workgroup_size(1, 1, 1) fn main() {
            // Just need a static use.
            let _value = buffer.x;
        }
        ",
        "
        @group(0) @binding(0)
        var<uniform> buffer : f32;

        @compute @workgroup_size(1, 1, 1) fn main() {
            // Just need a static use.
            let _value = buffer;
        }
        ",
    ];

    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("buffer"),
        size: 8,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group_layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

    let pipelines = SHADER_SRC
        .iter()
        .enumerate()
        .map(|(i, &shader_src)| {
            let module = ctx
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&format!("shader{i}")),
                    source: wgpu::ShaderSource::Wgsl(shader_src.into()),
                });

            ctx.device
                .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(&format!("pipeline{i}")),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })
        })
        .collect::<Vec<_>>();

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());

    for (i, pipeline) in pipelines.iter().enumerate() {
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("bg{i}")),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: Some(NonZeroU64::new(u64::try_from(8 - 4 * i).unwrap()).unwrap()),
                }),
            }],
        });

        cpass.set_pipeline(pipeline);
        cpass.set_bind_group(0, &bind_group, &[0]);
        cpass.dispatch_workgroups(1, 1, 1);
    }
    drop(cpass);

    let data = [0u8; 8];
    ctx.queue.write_buffer(&buffer, 0, &data);
    ctx.queue.submit(Some(encoder.finish()));

    ctx.device.poll(PollType::wait_indefinitely()).unwrap();
}

/// Test `descriptor` against a bind group layout that requires non-filtering sampler.
fn try_sampler_nonfiltering_layout(
    ctx: TestingContext,
    descriptor: &wgpu::SamplerDescriptor,
    good: bool,
) {
    let label = descriptor.label;
    let bind_group_layout = ctx
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            }],
        });

    let sampler = ctx.device.create_sampler(descriptor);

    let create_bind_group = || {
        let _ = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label,
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&sampler),
            }],
        });
    };

    if good {
        wgpu_test::valid(&ctx.device, create_bind_group);
    } else {
        wgpu_test::fail(
            &ctx.device,
            create_bind_group,
            Some("but given a sampler with filtering"),
        );
    }
}

#[apply(gpu_test!)]
static MULTIPLE_BINDINGS_WITH_DIFFERENT_SIZES: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .limits(wgpu::Limits::downlevel_defaults())
            .enable_noop(),
    )
    .run_sync(multiple_bindings_with_differing_sizes);

#[apply(gpu_test!)]
static BIND_GROUP_NONFILTERING_LAYOUT_NONFILTERING_SAMPLER: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(TestParameters::default().enable_noop())
        .run_sync(|ctx| {
            try_sampler_nonfiltering_layout(
                ctx,
                &wgpu::SamplerDescriptor {
                    label: Some("bind_group_non_filtering_layout_nonfiltering_sampler"),
                    min_filter: wgpu::FilterMode::Nearest,
                    mag_filter: wgpu::FilterMode::Nearest,
                    mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                    ..wgpu::SamplerDescriptor::default()
                },
                true,
            );
        });

#[apply(gpu_test!)]
static BIND_GROUP_NONFILTERING_LAYOUT_MIN_SAMPLER: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(TestParameters::default().enable_noop())
        .run_sync(|ctx| {
            try_sampler_nonfiltering_layout(
                ctx,
                &wgpu::SamplerDescriptor {
                    label: Some("bind_group_non_filtering_layout_min_sampler"),
                    min_filter: wgpu::FilterMode::Linear,
                    mag_filter: wgpu::FilterMode::Nearest,
                    mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                    ..wgpu::SamplerDescriptor::default()
                },
                false,
            );
        });

#[apply(gpu_test!)]
static BIND_GROUP_NONFILTERING_LAYOUT_MAG_SAMPLER: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(TestParameters::default().enable_noop())
        .run_sync(|ctx| {
            try_sampler_nonfiltering_layout(
                ctx,
                &wgpu::SamplerDescriptor {
                    label: Some("bind_group_non_filtering_layout_mag_sampler"),
                    min_filter: wgpu::FilterMode::Nearest,
                    mag_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                    ..wgpu::SamplerDescriptor::default()
                },
                false,
            );
        });

#[apply(gpu_test!)]
static BIND_GROUP_NONFILTERING_LAYOUT_MIPMAP_SAMPLER: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(TestParameters::default().enable_noop())
        .run_sync(|ctx| {
            try_sampler_nonfiltering_layout(
                ctx,
                &wgpu::SamplerDescriptor {
                    label: Some("bind_group_non_filtering_layout_mipmap_sampler"),
                    min_filter: wgpu::FilterMode::Nearest,
                    mag_filter: wgpu::FilterMode::Nearest,
                    mipmap_filter: wgpu::MipmapFilterMode::Linear,
                    ..wgpu::SamplerDescriptor::default()
                },
                false,
            );
        });

#[apply(gpu_test!)]
static BIND_GROUP_WITH_MAX_BINDING_INDEX: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default().limits(wgpu::Limits::downlevel_defaults()))
    .run_async(|ctx| async move {
        let (device, queue) = ctx
            .adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: wgpu::Limits {
                    max_bindings_per_bind_group: ctx.adapter.limits().max_bindings_per_bind_group,
                    ..Default::default()
                },
                ..Default::default()
            })
            .await
            .unwrap();

        let max_binding_index = device.limits().max_bindings_per_bind_group - 1;
        let src_binding_index = max_binding_index - 1;
        let dst_binding_index = max_binding_index;

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: src_binding_index,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: dst_binding_index,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let shader = format!(
            "
            @group(0) @binding({src_binding_index}) var<uniform> src: u32;
            @group(0) @binding({dst_binding_index}) var<storage, read_write> dst: u32;
            @compute @workgroup_size(1)
            fn main() {{
                dst = src;
            }}"
        );

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&pl),
            entry_point: Some("main"),
            compilation_options: Default::default(),
            module: &module,
            cache: None,
        });

        let test_value = 123u32;

        let src = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            usage: wgpu::BufferUsages::UNIFORM,
            contents: &test_value.to_le_bytes(),
        });
        let dst = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: src_binding_index,
                    resource: wgpu::BindingResource::Buffer(src.as_entire_buffer_binding()),
                },
                wgpu::BindGroupEntry {
                    binding: dst_binding_index,
                    resource: wgpu::BindingResource::Buffer(dst.as_entire_buffer_binding()),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_bind_group(0, &bg, &[]);
            pass.set_pipeline(&pipeline);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&dst, 0, &readback, 0, 4);
        queue.submit(Some(encoder.finish()));

        readback.slice(..).map_async(wgpu::MapMode::Read, |_| ());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

        assert_eq!(
            &*readback.slice(..).get_mapped_range().unwrap(),
            &test_value.to_le_bytes()
        );
    });

/// Regression test for the wgpu-hal Metal backend binding a shader stage's
/// resources from the wrong offset into its internal per-bind-group
/// resource arrays.
///
/// wgpu-hal's Metal backend builds one flat array per resource kind
/// (buffers/textures/samplers) for each bind group, containing every
/// resource visible to any shader stage, laid out stage-by-stage in a fixed
/// order. When encoding `set_bind_group`, it computes where each stage's own
/// resources start in that array as the sum of the resource counts of every
/// stage that comes before it. If the stage order used to lay out the array
/// and the stage order used to sum up the preceding counts ever disagree,
/// a stage can end up bound to a completely different resource than the one
/// it was assigned in the bind group.
///
/// This creates a bind group with buffers visible to `TASK`, `MESH`, and
/// `FRAGMENT` (none of which the compute shader below actually uses) ahead
/// of a `COMPUTE`-visible buffer, then dispatches a compute shader that
/// doubles the compute buffer's value. If the compute stage's resources are
/// read from the wrong offset because of a stage-ordering mismatch, the
/// shader will double one of the filler buffers' values instead, and the
/// compute buffer will be left unmodified.
async fn bind_group_stage_order(ctx: TestingContext) {
    let device = &ctx.device;

    const FILLER_VALUE: u32 = 0xDEAD_0000;
    const COMPUTE_VALUE: u32 = 111;

    let make_filler_buffer = |label: &str| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            usage: wgpu::BufferUsages::STORAGE,
            contents: bytemuck::bytes_of(&FILLER_VALUE),
        })
    };

    let task_filler = make_filler_buffer("task filler");
    let mesh_filler = make_filler_buffer("mesh filler");
    let fragment_filler = make_filler_buffer("fragment filler");
    let compute_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("compute buffer"),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        contents: bytemuck::bytes_of(&COMPUTE_VALUE),
    });

    let filler_entry = |binding: u32, visibility: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl"),
        entries: &[
            filler_entry(0, wgpu::ShaderStages::TASK),
            filler_entry(1, wgpu::ShaderStages::MESH),
            filler_entry(2, wgpu::ShaderStages::FRAGMENT),
            filler_entry(3, wgpu::ShaderStages::COMPUTE),
        ],
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bg"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: task_filler.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: mesh_filler.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: fragment_filler.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: compute_buffer.as_entire_binding(),
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shader"),
        source: wgpu::ShaderSource::Wgsl(
            "
            @group(0) @binding(3) var<storage, read_write> value: u32;

            @compute @workgroup_size(1)
            fn main() {
                value = value * 2u;
            }
            "
            .into(),
        ),
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("pipeline"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: 4,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        cpass.set_pipeline(&pipeline);
        cpass.set_bind_group(0, &bind_group, &[]);
        cpass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&compute_buffer, 0, &readback, 0, 4);
    ctx.queue.submit(Some(encoder.finish()));

    readback.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    device.poll(PollType::wait_indefinitely()).unwrap();

    let result: u32 = *bytemuck::from_bytes(&readback.slice(..).get_mapped_range().unwrap());
    assert_eq!(
        result,
        COMPUTE_VALUE * 2,
        "compute stage was bound to the wrong buffer (expected the `COMPUTE`-visible buffer's \
         value to be doubled, got {result:#x})",
    );
}

#[apply(gpu_test!)]
static BIND_GROUP_STAGE_ORDER: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .limits(wgpu::Limits::downlevel_defaults())
            .features(wgpu::Features::EXPERIMENTAL_MESH_SHADER),
    )
    .run_async(bind_group_stage_order);

/// What a pipeline layout has where the first pipeline's layout has a bind group layout.
#[derive(Clone, Copy)]
enum UnusedGroup {
    None,
    Empty,
}

/// In one compute pass, switches from a pipeline with the layout `[y, z]` to one with the layout
/// `[unused, z]`, where `unused` is no bind group layout or an empty one, and checks that the
/// second dispatch uses the bind group set at index 1.
///
/// Most backends number a bind group's bindings after the bindings of the groups before it. That
/// numbering changes with the switch, so the bind group at index 1 must be set again, although
/// its layout is the same.
async fn bind_group_after_a_group_that_becomes_unused(ctx: TestingContext, unused: UnusedGroup) {
    let device = &ctx.device;

    const Y_VALUE: u32 = 0xBAD0_0000;
    const Z_VALUE: u32 = 7;

    let storage_entry = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let y_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("y"),
        entries: &[storage_entry(0, true)],
    });
    let z_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("z"),
        entries: &[storage_entry(0, true), storage_entry(1, false)],
    });
    let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("empty"),
        entries: &[],
    });

    let y_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("y"),
        usage: wgpu::BufferUsages::STORAGE,
        contents: bytemuck::bytes_of(&Y_VALUE),
    });
    let z_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("z"),
        usage: wgpu::BufferUsages::STORAGE,
        contents: bytemuck::bytes_of(&Z_VALUE),
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("output"),
        size: 8,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let y_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("y"),
        layout: &y_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: y_buffer.as_entire_binding(),
        }],
    });
    let z_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("z"),
        layout: &z_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: z_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shader"),
        source: wgpu::ShaderSource::Wgsl(
            "
            @group(0) @binding(0) var<storage, read> y: u32;
            @group(1) @binding(0) var<storage, read> z: u32;
            @group(1) @binding(1) var<storage, read_write> output: array<u32, 2>;

            @compute @workgroup_size(1)
            fn first() {
                output[0] = z;
            }

            @compute @workgroup_size(1)
            fn second() {
                output[1] = z;
            }
            "
            .into(),
        ),
    });
    let pipeline = |label: &str, layouts: &[Option<&wgpu::BindGroupLayout>]| {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: layouts,
            immediate_size: 0,
        });
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            module: &module,
            entry_point: Some(label),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let first = pipeline("first", &[Some(&y_layout), Some(&z_layout)]);
    let unused_layout = match unused {
        UnusedGroup::None => None,
        UnusedGroup::Empty => Some(&empty_layout),
    };
    let second = pipeline("second", &[unused_layout, Some(&z_layout)]);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: 8,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&first);
        pass.set_bind_group(0, &y_group, &[]);
        pass.set_bind_group(1, &z_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&second);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 8);
    ctx.queue.submit(Some(encoder.finish()));

    readback.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    device.poll(PollType::wait_indefinitely()).unwrap();

    let result: [u32; 2] = *bytemuck::from_bytes(&readback.slice(..).get_mapped_range().unwrap());
    assert_eq!(
        result,
        [Z_VALUE, Z_VALUE],
        "the second dispatch did not use the bind group at index 1"
    );
}

fn unused_group_parameters() -> TestParameters {
    TestParameters::default()
        .limits(wgpu::Limits::downlevel_defaults())
        .downlevel_flags(wgpu::DownlevelFlags::COMPUTE_SHADERS)
}

#[apply(gpu_test!)]
static BIND_GROUP_AFTER_A_GROUP_THAT_BECOMES_NONE: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(unused_group_parameters())
        .run_async(|ctx| bind_group_after_a_group_that_becomes_unused(ctx, UnusedGroup::None));

#[apply(gpu_test!)]
static BIND_GROUP_AFTER_A_GROUP_THAT_BECOMES_EMPTY: GpuTestConfiguration =
    GpuTestConfiguration::new()
        .parameters(unused_group_parameters())
        .run_async(|ctx| bind_group_after_a_group_that_becomes_unused(ctx, UnusedGroup::Empty));

/// Sets an empty bind group below and above the group that the shader uses, before and after
/// it, switching between pipelines whose layouts have the empty group at either index, and
/// checks that every dispatch wrote through the used group. Vulkan gives an empty group no
/// descriptor set and binds nothing for it.
async fn empty_bind_groups_beside_a_used_one(ctx: TestingContext) {
    let device = &ctx.device;

    let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("empty"),
        entries: &[],
    });
    let data_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("data"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("output"),
        size: 16,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("empty"),
        layout: &empty_layout,
        entries: &[],
    });
    let data_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("data"),
        layout: &data_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        }],
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shader"),
        source: wgpu::ShaderSource::Wgsl(
            "
            @group(0) @binding(0) var<storage, read_write> at_zero: array<u32, 4>;
            @group(1) @binding(0) var<storage, read_write> at_one: array<u32, 4>;

            @compute @workgroup_size(1)
            fn above_first() {
                at_one[0] = 1u;
            }

            @compute @workgroup_size(1)
            fn below_first() {
                at_zero[1] = 2u;
            }

            @compute @workgroup_size(1)
            fn above_second() {
                at_one[2] = 3u;
            }

            @compute @workgroup_size(1)
            fn below_second() {
                at_zero[3] = 4u;
            }
            "
            .into(),
        ),
    });
    let above = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("above"),
        bind_group_layouts: &[Some(&empty_layout), Some(&data_layout)],
        immediate_size: 0,
    });
    let below = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("below"),
        bind_group_layouts: &[Some(&data_layout), Some(&empty_layout)],
        immediate_size: 0,
    });
    let pipeline = |entry_point: &str, layout: &wgpu::PipelineLayout| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry_point),
            layout: Some(layout),
            module: &module,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            cache: None,
        })
    };

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: 16,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        // The empty group set first.
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline("above_first", &above));
        pass.set_bind_group(0, &empty_group, &[]);
        pass.set_bind_group(1, &data_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&pipeline("below_first", &below));
        pass.set_bind_group(0, &data_group, &[]);
        pass.set_bind_group(1, &empty_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    {
        // The empty group set last.
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(&pipeline("above_second", &above));
        pass.set_bind_group(1, &data_group, &[]);
        pass.set_bind_group(0, &empty_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&pipeline("below_second", &below));
        pass.set_bind_group(1, &empty_group, &[]);
        pass.set_bind_group(0, &data_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 16);
    ctx.queue.submit(Some(encoder.finish()));

    readback.slice(..).map_async(wgpu::MapMode::Read, |_| ());
    device.poll(PollType::wait_indefinitely()).unwrap();

    let result: [u32; 4] = *bytemuck::from_bytes(&readback.slice(..).get_mapped_range().unwrap());
    assert_eq!(result, [1, 2, 3, 4]);
}

#[apply(gpu_test!)]
static EMPTY_BIND_GROUPS_BESIDE_A_USED_ONE: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(unused_group_parameters())
    .run_async(empty_bind_groups_beside_a_used_one);
