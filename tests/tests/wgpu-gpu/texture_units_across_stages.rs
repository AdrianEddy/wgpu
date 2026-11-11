//! Texture units across the stages and pass kinds of a pipeline layout.
//!
//! wgpu-core checks the sampled textures of a pipeline layout per shader stage, against
//! `max_sampled_textures_per_shader_stage` (16 on GL). A layout within that limit can still hold
//! more textures in all: up to 16 read by the vertex stage, 16 by the fragment stage, more by the
//! compute stage, and any number visible to no stage at all. The GL backend binds each texture to
//! a texture unit of its own, so these layouts take more units than one stage may use.
//!
//! The draws write one pixel per texture that the render stages read, in binding order, so a
//! texture bound to the wrong unit is caught too, and the dispatch writes the values of the
//! textures that the compute stage reads.

use std::{fmt::Write as _, num::NonZeroU64};

use wgpu::util::DeviceExt as _;
use wgpu_test::{
    apply, gpu_test, GpuTestConfiguration, GpuTestInitializer, TestParameters, TestingContext,
};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.extend([
        TEXTURE_UNITS_ACROSS_STAGES,
        TEXTURE_UNITS_OF_A_FULL_RENDER_LAYOUT,
        TEXTURE_UNITS_PER_PASS_KIND,
    ]);
}

/// 3 vertex-stage and 14 fragment-stage textures: each stage is within its limit, and together
/// they take 17 texture units in a draw.
#[apply(gpu_test!)]
static TEXTURE_UNITS_ACROSS_STAGES: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default())
    .run_sync(|ctx| {
        run(
            ctx,
            &Layout {
                vertex_group: vec![Reader::Vertex; 3],
                fragment_group: vec![Reader::Fragment; 14],
                uniforms: false,
                dispatch: false,
            },
        )
    });

/// 16 vertex-stage and 16 fragment-stage textures, the most a render pipeline can read, in the
/// same layout as a texture that only the compute stage reads and one that no stage reads. The
/// layout also has a dynamic uniform buffer that only the compute stage reads, before one that the
/// fragment stage reads.
#[apply(gpu_test!)]
static TEXTURE_UNITS_OF_A_FULL_RENDER_LAYOUT: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default())
    .run_sync(|ctx| run(ctx, &full_layout(false)));

/// The layout of `TEXTURE_UNITS_OF_A_FULL_RENDER_LAYOUT`, with the same bind groups used by a draw
/// and then by a dispatch. The dispatch reads the compute-only texture and a texture that the
/// fragment stage reads too.
#[apply(gpu_test!)]
static TEXTURE_UNITS_PER_PASS_KIND: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default().test_features_limits())
    .run_sync(|ctx| run(ctx, &full_layout(true)));

fn full_layout(dispatch: bool) -> Layout {
    // The bindings that the render stages do not read sit between the ones they do.
    let vertex_group = [Reader::Vertex; 8]
        .into_iter()
        .chain([Reader::Compute])
        .chain([Reader::Vertex; 8])
        .collect();
    let fragment_group = [Reader::Fragment; 8]
        .into_iter()
        .chain([Reader::Nobody])
        .chain([Reader::Fragment; 7])
        .chain([Reader::FragmentAndCompute])
        .collect();
    Layout {
        vertex_group,
        fragment_group,
        uniforms: true,
        dispatch,
    }
}

/// The stages that read a texture binding.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Reader {
    Vertex,
    Fragment,
    FragmentAndCompute,
    Compute,
    /// The binding is visible to no stage.
    Nobody,
}

impl Reader {
    fn visibility(self) -> wgpu::ShaderStages {
        match self {
            Self::Vertex => wgpu::ShaderStages::VERTEX,
            Self::Fragment => wgpu::ShaderStages::FRAGMENT,
            Self::FragmentAndCompute => wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
            Self::Compute => wgpu::ShaderStages::COMPUTE,
            Self::Nobody => wgpu::ShaderStages::NONE,
        }
    }

    fn render(self) -> bool {
        matches!(
            self,
            Self::Vertex | Self::Fragment | Self::FragmentAndCompute
        )
    }

    fn compute(self) -> bool {
        matches!(self, Self::Compute | Self::FragmentAndCompute)
    }
}

/// A test layout.
///
/// Bind group 0 holds `vertex_group`'s textures and bind group 1 `fragment_group`'s, each at the
/// binding of its index. Bind group 0 may not hold fragment-stage textures and bind group 1 may
/// not hold vertex-stage textures, so the binding order is the order of the render stages.
///
/// With `uniforms`, bind group 2 holds a compute-stage uniform buffer at binding 0 and a
/// fragment-stage uniform buffer at binding 1, both with dynamic offsets. With `dispatch`, it also
/// holds the compute stage's output at binding 2.
struct Layout {
    vertex_group: Vec<Reader>,
    fragment_group: Vec<Reader>,
    uniforms: bool,
    dispatch: bool,
}

impl Layout {
    fn texture_groups(&self) -> [&[Reader]; 2] {
        [&self.vertex_group[..], &self.fragment_group[..]]
    }

    /// The `(group, binding, reader)` of each texture, in binding order.
    fn textures(&self) -> impl Iterator<Item = (u32, u32, Reader)> + '_ {
        self.texture_groups()
            .into_iter()
            .zip(0..)
            .flat_map(|(readers, group)| {
                readers
                    .iter()
                    .zip(0..)
                    .map(move |(&reader, binding)| (group, binding, reader))
            })
    }

    /// The value that each texture holds, in binding order: the textures that the render stages
    /// read hold 1, 2, 3, and so on in binding order, the others 101 and up.
    fn values(&self) -> Vec<u32> {
        let mut render = 0;
        let mut other = 100;
        self.textures()
            .map(|(_, _, reader)| {
                let counter = if reader.render() {
                    &mut render
                } else {
                    &mut other
                };
                *counter += 1;
                *counter
            })
            .collect()
    }
}

/// The value at the fragment-stage uniform buffer's dynamic offset, and at the compute-stage one's.
const FRAGMENT_UNIFORM: u32 = 2000;
const COMPUTE_UNIFORM: u32 = 1000;
/// The fragment-stage uniform buffer's dynamic offset: a multiple of every adapter's
/// `min_uniform_buffer_offset_alignment`. The compute-stage one's is 0.
const FRAGMENT_UNIFORM_OFFSET: u32 = 256;

fn run(ctx: TestingContext, layout: &Layout) {
    let limit = ctx.device.limits().max_sampled_textures_per_shader_stage;
    for (stage, readers) in [
        (wgpu::ShaderStages::VERTEX, &layout.vertex_group),
        (wgpu::ShaderStages::FRAGMENT, &layout.fragment_group),
    ] {
        let count = readers
            .iter()
            .filter(|reader| reader.visibility().contains(stage))
            .count();
        assert!(count as u32 <= limit, "{stage:?} reads {count} textures");
        assert!(
            !readers
                .iter()
                .any(|reader| reader.render() && !reader.visibility().contains(stage)),
            "bind group of the {stage:?} stage has another render stage's texture"
        );
    }

    let shader = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(shader_source(layout).into()),
        });

    let texture_layouts = layout.texture_groups().map(|readers| {
        ctx.device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &readers
                    .iter()
                    .zip(0..)
                    .map(|(reader, binding)| wgpu::BindGroupLayoutEntry {
                        binding,
                        visibility: reader.visibility(),
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Uint,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    })
                    .collect::<Vec<_>>(),
            })
    });
    let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(16),
        },
        count: None,
    };
    let buffer_layout = layout.uniforms.then(|| {
        let mut entries = vec![
            uniform_entry(0, wgpu::ShaderStages::COMPUTE),
            uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
        ];
        if layout.dispatch {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        ctx.device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &entries,
            })
    });
    let bind_group_layouts = texture_layouts
        .iter()
        .chain(&buffer_layout)
        .map(Some)
        .collect::<Vec<_>>();
    let pipeline_layout = ctx
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &bind_group_layouts,
            immediate_size: 0,
        });

    let format = wgpu::TextureFormat::R32Uint;
    let render_pipeline = ctx
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
    let compute_pipeline = layout.dispatch.then(|| {
        ctx.device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            })
    });

    let values = layout.values();
    let mut values_iter = values.iter();
    let texture_views = layout.texture_groups().map(|readers| {
        readers
            .iter()
            .map(|_| {
                ctx.device
                    .create_texture_with_data(
                        &ctx.queue,
                        &wgpu::TextureDescriptor {
                            label: None,
                            size: wgpu::Extent3d::default(),
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::R8Uint,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING,
                            view_formats: &[],
                        },
                        wgpu::util::TextureDataOrder::LayerMajor,
                        &[*values_iter.next().unwrap() as u8],
                    )
                    .create_view(&wgpu::TextureViewDescriptor::default())
            })
            .collect::<Vec<_>>()
    });
    let texture_bind_groups = texture_layouts
        .iter()
        .zip(&texture_views)
        .map(|(bind_group_layout, views)| {
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: bind_group_layout,
                entries: &views
                    .iter()
                    .zip(0..)
                    .map(|(view, binding)| wgpu::BindGroupEntry {
                        binding,
                        resource: wgpu::BindingResource::TextureView(view),
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    let compute_textures = layout
        .textures()
        .zip(&values)
        .filter(|((_, _, reader), _)| reader.compute())
        .map(|(_, &value)| value)
        .collect::<Vec<_>>();
    let output_size = (4 * (compute_textures.len() + 1)) as u64;
    let output_buffer = |usage| {
        ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: output_size,
            usage,
            mapped_at_creation: false,
        })
    };
    let output = layout.dispatch.then(|| {
        (
            output_buffer(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC),
            output_buffer(wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ),
        )
    });
    let uniforms = layout.uniforms.then(|| {
        let mut contents = vec![0u32; (FRAGMENT_UNIFORM_OFFSET as usize + 16) / 4];
        contents[0] = COMPUTE_UNIFORM;
        contents[FRAGMENT_UNIFORM_OFFSET as usize / 4] = FRAGMENT_UNIFORM;
        ctx.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&contents),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    });
    let buffer_bind_group = buffer_layout.as_ref().map(|bind_group_layout| {
        let uniforms = uniforms.as_ref().unwrap();
        let uniform = |binding| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: uniforms,
                offset: 0,
                size: NonZeroU64::new(16),
            }),
        };
        let mut entries = vec![uniform(0), uniform(1)];
        if let Some((output, _)) = &output {
            entries.push(wgpu::BindGroupEntry {
                binding: 2,
                resource: output.as_entire_binding(),
            });
        }
        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: bind_group_layout,
            entries: &entries,
        })
    });
    // Ordered by binding number: binding 0 is the compute stage's, binding 1 the fragment stage's.
    let dynamic_offsets = [0, FRAGMENT_UNIFORM_OFFSET];

    // One pixel per texture that the render stages read, then one for the fragment-stage uniform.
    let mut expected_pixels = values
        .iter()
        .zip(layout.textures())
        .filter(|(_, (_, _, reader))| reader.render())
        .map(|(&value, _)| value)
        .collect::<Vec<_>>();
    if layout.uniforms {
        expected_pixels.push(FRAGMENT_UNIFORM);
    }
    let width = expected_pixels.len() as u32;
    let target_size = wgpu::Extent3d {
        width,
        height: 1,
        depth_or_array_layers: 1,
    };
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: target_size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    assert!(4 * width <= wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let pixel_readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&render_pipeline);
        for (index, bind_group) in (0..).zip(&texture_bind_groups) {
            pass.set_bind_group(index, bind_group, &[]);
        }
        if let Some(bind_group) = &buffer_bind_group {
            pass.set_bind_group(2, bind_group, &dynamic_offsets);
        }
        pass.draw(0..3, 0..1);
    }
    if let Some(compute_pipeline) = &compute_pipeline {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(compute_pipeline);
        for (index, bind_group) in (0..).zip(&texture_bind_groups) {
            pass.set_bind_group(index, bind_group, &[]);
        }
        pass.set_bind_group(2, buffer_bind_group.as_ref().unwrap(), &dynamic_offsets);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &pixel_readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                rows_per_image: None,
            },
        },
        target_size,
    );
    if let Some((output, output_readback)) = &output {
        encoder.copy_buffer_to_buffer(output, 0, output_readback, 0, output_size);
    }
    ctx.queue.submit([encoder.finish()]);

    pixel_readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |_| ());
    if let Some((_, output_readback)) = &output {
        output_readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| ());
    }
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();

    let pixels = bytemuck::cast_slice::<u8, u32>(
        &pixel_readback.slice(..).get_mapped_range().unwrap()[..4 * width as usize],
    )
    .to_vec();
    assert_eq!(
        pixels, expected_pixels,
        "the value of each texture that the render stages read, then the fragment-stage uniform"
    );
    if let Some((_, output_readback)) = &output {
        let mut expected_output = compute_textures;
        expected_output.push(COMPUTE_UNIFORM);
        let output =
            bytemuck::cast_slice::<u8, u32>(&output_readback.slice(..).get_mapped_range().unwrap())
                .to_vec();
        assert_eq!(
            output, expected_output,
            "the value of each texture that the compute stage reads, then the compute-stage uniform"
        );
    }
}

/// The vertex stage passes the values of its textures to the fragment stage, four per varying.
/// The fragment stage writes, at pixel `i`, the `i`th value of the vertex stage's textures and
/// then of its own, and after them the value of its uniform buffer. The compute stage writes the
/// values of its textures and then of its uniform buffer.
fn shader_source(layout: &Layout) -> String {
    let load = |group: u32, binding: u32| format!("textureLoad(t{group}_{binding}, vec2i(0), 0).r");
    let mut source = String::new();
    for (group, binding, _) in layout.textures() {
        writeln!(
            source,
            "@group({group}) @binding({binding}) var t{group}_{binding}: texture_2d<u32>;"
        )
        .unwrap();
    }
    if layout.uniforms {
        source += "@group(2) @binding(0) var<uniform> compute_uniform: vec4u;\n";
        source += "@group(2) @binding(1) var<uniform> fragment_uniform: vec4u;\n";
    }

    let vertex_loads = layout
        .textures()
        .filter(|&(_, _, reader)| reader == Reader::Vertex)
        .map(|(group, binding, _)| load(group, binding))
        .collect::<Vec<_>>();
    let varyings = vertex_loads.len().div_ceil(4);
    source += "\nstruct VertexOutput {\n    @builtin(position) position: vec4f,\n";
    for varying in 0..varyings {
        writeln!(
            source,
            "    @location({varying}) @interpolate(flat) vertex_values_{varying}: vec4u,"
        )
        .unwrap();
    }
    source += "}\n\n@vertex\nfn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {\n";
    source += "    // A triangle that covers the target.\n";
    source += "    let uv = vec2f(f32((index << 1u) & 2u), f32(index & 2u));\n";
    source += "    var vertex_output: VertexOutput;\n";
    source += "    vertex_output.position = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);\n";
    for (varying, loads) in vertex_loads.chunks(4).enumerate() {
        let mut components = loads.to_vec();
        components.resize(4, "0u".to_owned());
        writeln!(
            source,
            "    vertex_output.vertex_values_{varying} = vec4u({});",
            components.join(", ")
        )
        .unwrap();
    }
    source += "    return vertex_output;\n}\n";

    let mut pixels = (0..vertex_loads.len())
        .map(|index| {
            format!(
                "input.vertex_values_{}.{}",
                index / 4,
                ["x", "y", "z", "w"][index % 4]
            )
        })
        .chain(
            layout
                .textures()
                .filter(|&(_, _, reader)| reader.render() && reader != Reader::Vertex)
                .map(|(group, binding, _)| load(group, binding)),
        )
        .collect::<Vec<_>>();
    if layout.uniforms {
        pixels.push("fragment_uniform.x".to_owned());
    }
    write!(
        source,
        "
@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4u {{
    var pixels = array<u32, {count}>({pixels});
    return vec4u(pixels[u32(input.position.x)], 0u, 0u, 0u);
}}
",
        count = pixels.len(),
        pixels = pixels.join(", "),
    )
    .unwrap();

    if layout.dispatch {
        let mut outputs = layout
            .textures()
            .filter(|&(_, _, reader)| reader.compute())
            .map(|(group, binding, _)| load(group, binding))
            .collect::<Vec<_>>();
        outputs.push("compute_uniform.x".to_owned());
        write!(
            source,
            "
@group(2) @binding(2) var<storage, read_write> compute_output: array<u32, {count}>;

@compute @workgroup_size(1)
fn cs_main() {{
{stores}}}
",
            count = outputs.len(),
            stores = outputs
                .iter()
                .enumerate()
                .map(|(index, value)| format!("    compute_output[{index}] = {value};\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    source
}
