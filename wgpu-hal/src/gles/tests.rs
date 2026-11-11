//! The slots of a pipeline layout's bindings in each pass kind, and how a pass binds them.
//!
//! wgpu-core checks each stage of a pipeline layout on its own, so a layout within the per-stage
//! limits can hold more bindings of a type in all than one stage may use: up to the limit for
//! each of the vertex, fragment and compute stages, plus any number visible to no stage. A
//! pipeline layout therefore numbers its bindings once per pass kind, and gives no slot to a
//! binding that no stage of the kind can see. The GPU tests `texture_units_across_stages` draw and
//! dispatch with such layouts.

use alloc::{sync::Arc, vec, vec::Vec};
use core::num::NonZeroU32;

use super::{
    device::{number_bindings, NumberedBindings},
    PassKind,
};

const V: wgt::ShaderStages = wgt::ShaderStages::VERTEX;
const F: wgt::ShaderStages = wgt::ShaderStages::FRAGMENT;
const C: wgt::ShaderStages = wgt::ShaderStages::COMPUTE;
const NONE: wgt::ShaderStages = wgt::ShaderStages::NONE;

fn texture(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::Texture {
            sample_type: wgt::TextureSampleType::Float { filterable: true },
            view_dimension: wgt::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::Sampler(wgt::SamplerBindingType::Filtering),
        count: None,
    }
}

fn storage_texture(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::StorageTexture {
            access: wgt::StorageTextureAccess::ReadWrite,
            format: wgt::TextureFormat::R32Float,
            view_dimension: wgt::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn buffer(
    binding: u32,
    visibility: wgt::ShaderStages,
    ty: wgt::BufferBindingType,
    has_dynamic_offset: bool,
) -> wgt::BindGroupLayoutEntry {
    wgt::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgt::BindingType::Buffer {
            ty,
            has_dynamic_offset,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_buffer(binding: u32, visibility: wgt::ShaderStages) -> wgt::BindGroupLayoutEntry {
    buffer(binding, visibility, wgt::BufferBindingType::Uniform, false)
}

fn group(entries: Vec<wgt::BindGroupLayoutEntry>) -> super::BindGroupLayout {
    super::BindGroupLayout {
        entries: Arc::from(entries),
    }
}

/// `make(binding, visibility)` for each of `visibilities`, at the bindings that follow `first`.
fn entries(
    first: u32,
    visibilities: &[wgt::ShaderStages],
    make: fn(u32, wgt::ShaderStages) -> wgt::BindGroupLayoutEntry,
) -> Vec<wgt::BindGroupLayoutEntry> {
    visibilities
        .iter()
        .zip(first..)
        .map(|(&visibility, binding)| make(binding, visibility))
        .collect()
}

/// `count` times `visibility`.
fn times(count: usize, visibility: wgt::ShaderStages) -> Vec<wgt::ShaderStages> {
    vec![visibility; count]
}

/// The visibilities of the texture bindings of the bind groups of `full_render_layout`: 8 vertex
/// textures, a compute one, and 8 more vertex textures; then 8 fragment textures, one visible to no
/// stage, 7 more fragment textures, and one visible to the fragment and compute stages.
fn full_render_visibilities() -> [Vec<wgt::ShaderStages>; 2] {
    [
        [times(8, V), vec![C], times(8, V)].concat(),
        [times(8, F), vec![NONE], times(7, F), vec![F | C]].concat(),
    ]
}

/// A layout of 16 vertex textures and 16 fragment textures, the most a render pipeline can read,
/// with a compute texture, a texture visible to no stage, and a texture visible to the fragment
/// and compute stages among them.
fn full_render_layout(
    make: fn(u32, wgt::ShaderStages) -> wgt::BindGroupLayoutEntry,
) -> [super::BindGroupLayout; 2] {
    full_render_visibilities().map(|visibilities| group(entries(0, &visibilities, make)))
}

fn number(groups: &[&super::BindGroupLayout]) -> NumberedBindings {
    let numbered = number_bindings(&groups.iter().map(|&group| Some(group)).collect::<Vec<_>>());
    check_binding_maps(&numbered);
    numbered
}

/// The slots of the bindings of bind group `group` in `kind`, by binding number.
fn slots(numbered: &NumberedBindings, kind: PassKind, group: usize) -> Vec<Option<u8>> {
    let info = numbered.group_infos[group].as_ref().unwrap();
    info.binding_to_slot[kind].to_vec()
}

/// `Some` of each of `slots`.
fn some(slots: impl IntoIterator<Item = u8>) -> Vec<Option<u8>> {
    slots.into_iter().map(Some).collect()
}

/// Checks that naga's binding map of each pass kind holds exactly the slots of the binding tables.
fn check_binding_maps(numbered: &NumberedBindings) {
    for kind in PassKind::ALL {
        let mut from_tables = Vec::new();
        for (group, info) in (0..).zip(numbered.group_infos.iter()) {
            let Some(info) = info else { continue };
            for (binding, &slot) in (0..).zip(info.binding_to_slot[kind].iter()) {
                if let Some(slot) = slot {
                    from_tables.push((naga::ResourceBinding { group, binding }, slot));
                }
            }
        }
        from_tables.sort();
        let from_map = numbered.binding_maps[kind]
            .iter()
            .map(|(&br, &slot)| (br, slot))
            .collect::<Vec<_>>();
        assert_eq!(from_map, from_tables, "{kind:?}");
    }
}

/// The binding tables of a pass hold the slots of every stage of its kind.
#[test]
fn the_tables_hold_the_slots_of_a_pass() {
    const {
        // A render pass has two stages.
        assert!(super::MAX_TEXTURE_SLOTS >= 2 * super::MAX_TEXTURE_SLOTS_PER_STAGE);
        assert!(super::MAX_SAMPLERS >= 2 * super::MAX_SAMPLERS_PER_STAGE);
        // GLES 3.0 and WebGL2 guarantee 32 combined texture image units.
        assert!(super::MAX_TEXTURE_SLOTS <= 32);
        // The dirty masks of `command.rs` have a bit per unit.
        assert!(super::MAX_TEXTURE_SLOTS <= u32::BITS as usize);
        assert!(super::MAX_SAMPLERS <= u32::BITS as usize);
        // The other types' slots are `u8`s.
        assert!(2 * super::MAX_BINDING_SLOTS_PER_STAGE <= u8::MAX as u32 + 1);
    }
    assert_eq!(PassKind::Render.stages(), V | F);
    assert_eq!(PassKind::Compute.stages(), C);
}

/// 3 vertex textures and 14 fragment textures, each stage within its limit, take 17 units in a
/// render pass and none in a compute pass.
#[test]
fn a_render_pass_numbers_the_textures_of_both_stages() {
    let mut vertex = entries(0, &times(3, V), texture);
    vertex.push(sampler(3, V));
    let mut fragment = entries(0, &times(14, F), texture);
    fragment.push(sampler(20, F));
    let numbered = number(&[&group(vertex), &group(fragment)]);

    let mut vertex_slots = some(0..3);
    vertex_slots.push(Some(0));
    assert_eq!(slots(&numbered, PassKind::Render, 0), vertex_slots);
    let mut fragment_slots = some(3..17);
    fragment_slots.extend([None; 6]);
    fragment_slots.push(Some(1));
    assert_eq!(slots(&numbered, PassKind::Render, 1), fragment_slots);

    assert_eq!(slots(&numbered, PassKind::Compute, 0), vec![None; 4]);
    assert_eq!(slots(&numbered, PassKind::Compute, 1), vec![None; 21]);
}

/// The slots of `full_render_layout`'s two bind groups in a render pass and in a compute pass.
fn full_render_slots() -> [[Vec<Option<u8>>; 2]; 2] {
    let render = [
        [some(0..8), vec![None], some(8..16)].concat(),
        [some(16..24), vec![None], some(24..31), some([31])].concat(),
    ];
    let compute = [
        [vec![None; 8], some([0]), vec![None; 8]].concat(),
        [vec![None; 16], some([1])].concat(),
    ];
    [render, compute]
}

/// A full render layout takes every texture unit of a render pass, and the compute texture and
/// the texture visible to no stage take none in it.
#[test]
fn a_full_render_layout_beside_compute_and_invisible_textures() {
    let [vertex, fragment] = full_render_layout(texture);
    let numbered = number(&[&vertex, &fragment]);
    let [render, compute] = full_render_slots();
    for (group, (render, compute)) in render.iter().zip(&compute).enumerate() {
        assert_eq!(&slots(&numbered, PassKind::Render, group), render);
        assert_eq!(&slots(&numbered, PassKind::Compute, group), compute);
    }
    let last_unit = super::MAX_TEXTURE_SLOTS as u8 - 1;
    assert_eq!(render[1].last(), Some(&Some(last_unit)));
}

#[test]
fn samplers_are_numbered_like_textures() {
    let [vertex, fragment] = full_render_layout(sampler);
    let numbered = number(&[&vertex, &fragment]);
    let [render, compute] = full_render_slots();
    for (group, (render, compute)) in render.iter().zip(&compute).enumerate() {
        assert_eq!(&slots(&numbered, PassKind::Render, group), render);
        assert_eq!(&slots(&numbered, PassKind::Compute, group), compute);
    }
}

/// Bindings visible to no stage take no slot, however many there are: 33 textures, 33 samplers
/// and 256 uniform buffers, more than a `u8` counter can count.
#[test]
fn a_binding_visible_to_no_stage_takes_no_slot() {
    let invisible = group(
        [
            entries(0, &times(33, NONE), texture),
            entries(100, &times(33, NONE), sampler),
            entries(200, &times(256, NONE), uniform_buffer),
        ]
        .concat(),
    );
    let numbered = number(&[&invisible]);
    for kind in PassKind::ALL {
        assert!(slots(&numbered, kind, 0).iter().all(Option::is_none));
        assert!(numbered.binding_maps[kind].is_empty());
    }
}

/// With the per-stage limit at `MAX_BINDING_SLOTS_PER_STAGE`, the two stages of a render pass
/// take every `u8` slot.
#[test]
fn a_render_pass_takes_every_u8_slot_of_a_type() {
    let per_stage = super::MAX_BINDING_SLOTS_PER_STAGE as usize;
    let uniforms = group(
        [
            entries(0, &times(per_stage, V), uniform_buffer),
            entries(per_stage as u32, &times(per_stage, F), uniform_buffer),
        ]
        .concat(),
    );
    let numbered = number(&[&uniforms]);
    assert_eq!(slots(&numbered, PassKind::Render, 0), some(0..=u8::MAX));
    assert_eq!(
        slots(&numbered, PassKind::Compute, 0),
        vec![None; 2 * per_stage]
    );
}

/// A binding visible to stages of both pass kinds takes a slot in each, and a binding visible to
/// several stages of a pass kind takes one slot in it.
#[test]
fn a_binding_visible_to_both_pass_kinds_takes_a_slot_in_each() {
    let mixed = group(entries(0, &[V, C, V | F | C, F | C, V | F], texture));
    let shared = group(entries(0, &times(16, V | F | C), texture));
    let numbered = number(&[&mixed, &shared]);
    assert_eq!(
        slots(&numbered, PassKind::Render, 0),
        [Some(0), None, Some(1), Some(2), Some(3)]
    );
    assert_eq!(
        slots(&numbered, PassKind::Compute, 0),
        [None, Some(0), Some(1), Some(2), None]
    );
    assert_eq!(slots(&numbered, PassKind::Render, 1), some(4..20));
    assert_eq!(slots(&numbered, PassKind::Compute, 1), some(3..19));
}

/// Each type of resource has a numbering of its own in each pass kind.
#[test]
fn each_type_is_numbered_per_pass_kind() {
    let storage = wgt::BufferBindingType::Storage { read_only: false };
    let mixed = group(vec![
        uniform_buffer(0, V),
        buffer(1, C, storage, false),
        storage_texture(2, F),
        storage_texture(3, C),
        uniform_buffer(4, C),
        buffer(5, F | C, storage, false),
        texture(6, C),
        sampler(7, F),
    ]);
    let numbered = number(&[&mixed]);
    let render = [Some(0), None, Some(0), None, None, Some(0), None, Some(0)];
    let compute = [
        None,
        Some(0),
        None,
        Some(0),
        Some(0),
        Some(1),
        Some(0),
        None,
    ];
    assert_eq!(slots(&numbered, PassKind::Render, 0), render);
    assert_eq!(slots(&numbered, PassKind::Compute, 0), compute);
}

/// The slots of a bind group depend only on its own layout and those of the bind groups before
/// it: wgpu-core sets the bind groups again only from the first one whose layout changed.
#[test]
fn slots_depend_only_on_earlier_bind_groups() {
    let first = group(entries(0, &[V, C, F, V | C, NONE], texture));
    let second = group(entries(0, &[C, F, V], texture));
    let thirds = [
        group(entries(0, &[F, F, C], texture)),
        group(entries(0, &[C, V, NONE, F], texture)),
    ];
    let numbered = thirds
        .iter()
        .map(|third| number(&[&first, &second, third]))
        .chain([number(&[&first, &second])])
        .collect::<Vec<_>>();
    for kind in PassKind::ALL {
        for group in 0..2 {
            let slots = numbered
                .iter()
                .map(|numbered| slots(numbered, kind, group))
                .collect::<Vec<_>>();
            assert!(slots.iter().all(|s| *s == slots[0]), "{kind:?} {group}");
        }
    }
}

#[test]
fn a_binding_array_takes_a_slot_per_element() {
    let mut array = texture(0, F);
    array.count = NonZeroU32::new(8);
    let layout = group(vec![array, texture(1, F)]);
    let numbered = number(&[&layout]);
    assert_eq!(slots(&numbered, PassKind::Render, 0), some([0, 8]));
}

/// How a pass binds a pipeline layout's bind groups: only the bindings that its kind numbers.
#[cfg(native)]
mod set_bind_group {
    use alloc::{boxed::Box, sync::Arc, vec::Vec};
    use core::num::NonZeroU32;

    use crate::CommandEncoder as _;

    use super::{
        buffer, full_render_layout, full_render_slots, group, number, sampler, texture,
        NumberedBindings, PassKind,
    };

    fn pipeline_layout(groups: &[&super::super::BindGroupLayout]) -> super::super::PipelineLayout {
        let NumberedBindings {
            group_infos,
            binding_maps,
        } = number(groups);
        super::super::PipelineLayout {
            group_infos,
            naga_options: binding_maps.map(|binding_map| naga::back::glsl::Options {
                binding_map,
                ..Default::default()
            }),
        }
    }

    fn encoder_in(kind: PassKind) -> super::super::CommandEncoder {
        let mut encoder = super::super::CommandEncoder {
            cmd_buffer: Default::default(),
            state: Default::default(),
            private_caps: super::super::PrivateCapabilities::empty(),
            counters: Arc::new(wgt::HalCounters::default()),
        };
        match kind {
            PassKind::Render => unsafe {
                encoder
                    .begin_render_pass(&crate::RenderPassDescriptor {
                        label: None,
                        extent: wgt::Extent3d::default(),
                        sample_count: 1,
                        color_attachments: &[],
                        depth_stencil_attachment: None,
                        multiview_mask: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .unwrap();
            },
            PassKind::Compute => unsafe {
                encoder.begin_compute_pass(&crate::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: None,
                })
            },
        }
        encoder.cmd_buffer.commands.clear();
        encoder
    }

    fn id(id: u32) -> NonZeroU32 {
        NonZeroU32::new(id).unwrap()
    }

    /// A bind group of a texture `id(base + binding)` for each binding of `layout`.
    fn textures(layout: &super::super::BindGroupLayout, base: u32) -> super::super::BindGroup {
        super::super::BindGroup {
            contents: layout
                .entries
                .iter()
                .map(|entry| super::super::RawBinding::Texture {
                    raw: glow::NativeTexture(id(base + entry.binding)),
                    target: glow::TEXTURE_2D,
                    aspects: crate::FormatAspects::COLOR,
                    mip_levels: 0..1,
                })
                .collect::<Box<[_]>>(),
        }
    }

    /// What an encoder bound.
    struct Bound {
        /// The `(slot, texture)` of each texture.
        textures: Vec<(u32, u32)>,
        /// The `(slot, buffer, offset)` of each buffer.
        buffers: Vec<(u32, u32, i32)>,
    }

    fn bound(encoder: &super::super::CommandEncoder) -> Bound {
        let mut bound = Bound {
            textures: Vec::new(),
            buffers: Vec::new(),
        };
        for command in &encoder.cmd_buffer.commands {
            match *command {
                super::super::Command::BindTexture { slot, texture, .. } => {
                    bound.textures.push((slot, texture.0.get()));
                }
                super::super::Command::BindBuffer {
                    slot,
                    buffer,
                    offset,
                    ..
                } => bound.buffers.push((slot, buffer.0.get(), offset)),
                _ => {}
            }
        }
        bound
    }

    /// A render pass and a compute pass each bind, from the same bind groups, only the textures
    /// and buffers that their kind numbers, at the slots of their kind. The compute uniform buffer
    /// comes first, so a render pass must still take its dynamic offset.
    #[test]
    fn each_pass_kind_binds_its_own_bindings() {
        let [vertex, fragment] = full_render_layout(texture);
        let uniform = wgt::BufferBindingType::Uniform;
        let buffers = group(vec![
            buffer(0, wgt::ShaderStages::COMPUTE, uniform, true),
            buffer(1, wgt::ShaderStages::FRAGMENT, uniform, true),
        ]);
        let layout = pipeline_layout(&[&vertex, &fragment, &buffers]);
        let texture_groups = [textures(&vertex, 100), textures(&fragment, 200)];
        let buffer_group = super::super::BindGroup {
            contents: Box::new([
                super::super::RawBinding::Buffer {
                    raw: glow::NativeBuffer(id(300)),
                    offset: 64,
                    size: 16,
                },
                super::super::RawBinding::Buffer {
                    raw: glow::NativeBuffer(id(301)),
                    offset: 64,
                    size: 16,
                },
            ]),
        };
        let dynamic_offsets = [1024, 2048];

        let slots = full_render_slots();
        for kind in PassKind::ALL {
            let mut encoder = encoder_in(kind);
            for (index, bind_group) in (0..).zip(&texture_groups) {
                unsafe { encoder.set_bind_group(&layout, index, bind_group, &[]) };
            }
            unsafe { encoder.set_bind_group(&layout, 2, &buffer_group, &dynamic_offsets) };

            let expected_textures = slots[kind as usize]
                .iter()
                .zip([100, 200])
                .flat_map(|(group_slots, base)| {
                    (0..).zip(group_slots).filter_map(move |(binding, slot)| {
                        slot.map(|slot| (u32::from(slot), base + binding))
                    })
                })
                .collect::<Vec<_>>();
            let expected_buffers = match kind {
                PassKind::Render => [(0, 301, 64 + 2048)],
                PassKind::Compute => [(0, 300, 64 + 1024)],
            };
            let bound = bound(&encoder);
            assert_eq!(bound.textures, expected_textures, "{kind:?}");
            assert_eq!(bound.buffers, expected_buffers, "{kind:?}");
        }
    }

    /// A render pass binds 32 samplers, 16 from each stage, from a layout that also has a compute
    /// sampler and a sampler visible to no stage. A sampler slot past the sampler table, or past
    /// the `u32` dirty mask, panics: numbered with the other two samplers, the last fragment
    /// sampler would take slot 33.
    #[test]
    fn a_render_pass_binds_the_samplers_of_both_stages() {
        let [vertex, fragment] = full_render_layout(sampler);
        let layout = pipeline_layout(&[&vertex, &fragment]);
        let mut encoder = encoder_in(PassKind::Render);
        for (index, bind_group_layout) in (0..).zip([&vertex, &fragment]) {
            let bind_group = super::super::BindGroup {
                contents: bind_group_layout
                    .entries
                    .iter()
                    .map(|entry| {
                        super::super::RawBinding::Sampler(glow::NativeSampler(
                            id(1 + entry.binding),
                        ))
                    })
                    .collect::<Box<[_]>>(),
            };
            unsafe { encoder.set_bind_group(&layout, index, &bind_group, &[]) };
        }
    }
}
