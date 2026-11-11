#[cfg(feature = "noop_test_hooks")]
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;
use core::ops::Range;

use super::{surface::ImageUse, Api, Buffer, DeviceResult, Resource, Texture, TextureView};

/// Command buffer type, which performs double duty as the command encoder type too.
#[derive(Debug)]
pub struct CommandBuffer {
    commands: Vec<Command>,
    /// The surface images the commands use, which the queue checks on submission.
    surface_images: Vec<ImageUse>,
    /// The test hooks of the encoder's device. `None` for a finished command buffer.
    #[cfg(feature = "noop_test_hooks")]
    hooks: Option<Arc<super::DeviceHooks>>,
}

#[derive(Debug)]
enum Command {
    ClearBuffer {
        buffer: Buffer,
        range: crate::MemoryRange,
    },
    CopyBufferToBuffer {
        src: Buffer,
        dst: Buffer,
        regions: Vec<crate::BufferCopy>,
    },
    /// A command that writes to a surface image.
    #[cfg(feature = "noop_test_hooks")]
    WriteImage {
        image: ImageUse,
        write: super::ImageWrite,
    },
}

impl CommandBuffer {
    /// # Safety
    ///
    /// Must be called with appropriate synchronization for the resources affected by the command,
    /// such as ensuring that buffers are not accessed by a command while aliasing references exist.
    pub(crate) unsafe fn execute(&self) {
        for command in &self.commands {
            unsafe { command.execute() };
        }
    }

    pub(super) fn new(
        #[cfg(feature = "noop_test_hooks")] hooks: Option<Arc<super::DeviceHooks>>,
    ) -> Self {
        Self {
            commands: Vec::new(),
            surface_images: Vec::new(),
            #[cfg(feature = "noop_test_hooks")]
            hooks,
        }
    }

    /// Checks the commands' use of surface images (see `super::surface`).
    pub(super) fn check_surface_images(&self, surface_textures: &[&super::SurfaceTexture]) {
        for image in &self.surface_images {
            image.check_submitted(surface_textures);
        }
    }

    fn use_texture(&mut self, texture: &Texture) {
        self.use_image(texture.image.as_deref());
    }

    fn use_view(&mut self, view: &TextureView) {
        self.use_image(view.image.as_deref());
    }

    /// Records that `image`, if any, is written with `write`.
    #[cfg(feature = "noop_test_hooks")]
    fn write_image(&mut self, image: Option<&super::surface::Image>, write: super::ImageWrite) {
        if let Some(image) = image {
            self.commands.push(Command::WriteImage {
                image: ImageUse::new(image),
                write,
            });
        }
    }

    fn use_image(&mut self, image: Option<&super::surface::Image>) {
        let Some(image) = image else {
            return;
        };
        let image = ImageUse::new(image);
        if !self.surface_images.iter().any(|used| used.same(&image)) {
            self.surface_images.push(image);
        }
    }
}

impl crate::CommandEncoder for CommandBuffer {
    type A = Api;

    unsafe fn begin_encoding(&mut self, label: crate::Label) -> DeviceResult<()> {
        assert!(self.commands.is_empty());
        #[cfg(feature = "noop_test_hooks")]
        if let Some(hooks) = &self.hooks {
            hooks.run_during_begin_encoding();
        }
        Ok(())
    }
    unsafe fn discard_encoding(&mut self) {
        self.commands.clear();
        self.surface_images.clear();
    }
    unsafe fn end_encoding(&mut self) -> DeviceResult<CommandBuffer> {
        #[cfg(feature = "noop_test_hooks")]
        if let Some(error) = self
            .hooks
            .as_deref()
            .and_then(|hooks| hooks.take_failure(super::DeviceOperation::EndEncoding))
        {
            return Err(error);
        }
        Ok(CommandBuffer {
            commands: mem::take(&mut self.commands),
            surface_images: mem::take(&mut self.surface_images),
            #[cfg(feature = "noop_test_hooks")]
            hooks: None,
        })
    }
    unsafe fn reset_all<I>(&mut self, command_buffers: I) {}

    unsafe fn transition_buffers<'a, T>(&mut self, barriers: T)
    where
        T: Iterator<Item = crate::BufferBarrier<'a, Buffer>>,
    {
    }

    unsafe fn transition_textures<'a, T>(&mut self, barriers: T)
    where
        T: Iterator<Item = crate::TextureBarrier<'a, Texture>>,
    {
        for barrier in barriers {
            self.use_texture(barrier.texture);
        }
    }

    unsafe fn clear_buffer(&mut self, buffer: &Buffer, range: crate::MemoryRange) {
        self.commands.push(Command::ClearBuffer {
            buffer: buffer.clone(),
            range,
        })
    }

    unsafe fn copy_buffer_to_buffer<T>(&mut self, src: &Buffer, dst: &Buffer, regions: T)
    where
        T: Iterator<Item = crate::BufferCopy>,
    {
        self.commands.push(Command::CopyBufferToBuffer {
            src: src.clone(),
            dst: dst.clone(),
            regions: regions.collect(),
        });
    }

    #[cfg(webgl)]
    unsafe fn copy_external_image_to_texture<T>(
        &mut self,
        src: &wgt::CopyExternalImageSourceInfo,
        dst: &Texture,
        dst_premultiplication: bool,
        regions: T,
    ) where
        T: Iterator<Item = crate::TextureCopy>,
    {
        self.use_texture(dst);
    }

    unsafe fn copy_texture_to_texture<T>(
        &mut self,
        src: &Texture,
        src_usage: wgt::TextureUses,
        dst: &Texture,
        regions: T,
    ) {
        // TODO: consider implementing this and other texture manipulation
        self.use_texture(src);
        self.use_texture(dst);
        #[cfg(feature = "noop_test_hooks")]
        self.write_image(dst.image.as_deref(), super::ImageWrite::Copy);
    }

    unsafe fn copy_buffer_to_texture<T>(&mut self, src: &Buffer, dst: &Texture, regions: T) {
        // TODO: consider implementing this and other texture manipulation
        self.use_texture(dst);
        #[cfg(feature = "noop_test_hooks")]
        self.write_image(dst.image.as_deref(), super::ImageWrite::Copy);
    }

    unsafe fn copy_texture_to_buffer<T>(
        &mut self,
        src: &Texture,
        src_usage: wgt::TextureUses,
        dst: &Buffer,
        regions: T,
    ) {
        // TODO: consider implementing this and other texture manipulation
        self.use_texture(src);
    }

    unsafe fn begin_query(&mut self, set: &Resource, index: u32) {}
    unsafe fn end_query(&mut self, set: &Resource, index: u32) {}
    unsafe fn write_timestamp(&mut self, set: &Resource, index: u32) {}
    unsafe fn read_acceleration_structure_compact_size(
        &mut self,
        acceleration_structure: &Resource,
        buf: &Buffer,
    ) {
    }
    unsafe fn reset_queries(&mut self, set: &Resource, range: Range<u32>) {}
    unsafe fn copy_query_results(
        &mut self,
        set: &Resource,
        range: Range<u32>,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        stride: wgt::BufferSize,
    ) {
    }

    // render

    unsafe fn begin_render_pass(
        &mut self,
        desc: &crate::RenderPassDescriptor<Resource, TextureView>,
    ) -> DeviceResult<()> {
        for attachment in desc.color_attachments.iter().flatten() {
            self.use_view(attachment.target.view);
            #[cfg(feature = "noop_test_hooks")]
            self.write_image(
                attachment.target.view.image.as_deref(),
                super::ImageWrite::RenderPass {
                    cleared: attachment.ops.contains(crate::AttachmentOps::LOAD_CLEAR),
                },
            );
            if let Some(resolve_target) = &attachment.resolve_target {
                self.use_view(resolve_target.view);
            }
        }
        if let Some(attachment) = &desc.depth_stencil_attachment {
            self.use_view(attachment.target.view);
        }
        Ok(())
    }
    unsafe fn end_render_pass(&mut self) {}

    unsafe fn set_bind_group(
        &mut self,
        layout: &Resource,
        index: u32,
        group: &Resource,
        dynamic_offsets: &[wgt::DynamicOffset],
    ) {
    }
    unsafe fn set_immediates(&mut self, layout: &Resource, offset_bytes: u32, data: &[u32]) {}

    unsafe fn insert_debug_marker(&mut self, label: &str) {}
    unsafe fn begin_debug_marker(&mut self, group_label: &str) {}
    unsafe fn end_debug_marker(&mut self) {}

    unsafe fn set_render_pipeline(&mut self, pipeline: &Resource) {}

    unsafe fn set_index_buffer<'a>(
        &mut self,
        binding: crate::BufferBinding<'a, Buffer, wgt::BufferAddress>,
        format: wgt::IndexFormat,
    ) {
    }
    unsafe fn set_vertex_buffer<'a>(
        &mut self,
        index: u32,
        binding: crate::BufferBinding<'a, Buffer, wgt::BufferAddress>,
    ) {
    }
    unsafe fn set_viewport(&mut self, rect: &crate::Rect<f32>, depth_range: Range<f32>) {}
    unsafe fn set_scissor_rect(&mut self, rect: &crate::Rect<u32>) {}
    unsafe fn set_stencil_reference(&mut self, value: u32) {}
    unsafe fn set_blend_constants(&mut self, color: &[f32; 4]) {}

    unsafe fn draw(
        &mut self,
        first_vertex: u32,
        vertex_count: u32,
        first_instance: u32,
        instance_count: u32,
    ) {
    }
    unsafe fn draw_indexed(
        &mut self,
        first_index: u32,
        index_count: u32,
        base_vertex: i32,
        first_instance: u32,
        instance_count: u32,
    ) {
    }
    unsafe fn draw_mesh_tasks(
        &mut self,
        group_count_x: u32,
        group_count_y: u32,
        group_count_z: u32,
    ) {
    }
    unsafe fn draw_indirect(
        &mut self,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        draw_count: u32,
    ) {
    }
    unsafe fn draw_indexed_indirect(
        &mut self,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        draw_count: u32,
    ) {
    }
    unsafe fn draw_mesh_tasks_indirect(
        &mut self,
        buffer: &<Self::A as crate::Api>::Buffer,
        offset: wgt::BufferAddress,
        draw_count: u32,
    ) {
    }
    unsafe fn draw_indirect_count(
        &mut self,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        count_buffer: &Buffer,
        count_offset: wgt::BufferAddress,
        max_count: u32,
    ) {
    }
    unsafe fn draw_indexed_indirect_count(
        &mut self,
        buffer: &Buffer,
        offset: wgt::BufferAddress,
        count_buffer: &Buffer,
        count_offset: wgt::BufferAddress,
        max_count: u32,
    ) {
    }
    unsafe fn draw_mesh_tasks_indirect_count(
        &mut self,
        buffer: &<Self::A as crate::Api>::Buffer,
        offset: wgt::BufferAddress,
        count_buffer: &<Self::A as crate::Api>::Buffer,
        count_offset: wgt::BufferAddress,
        max_count: u32,
    ) {
    }

    // compute

    unsafe fn begin_compute_pass(&mut self, desc: &crate::ComputePassDescriptor<Resource>) {}
    unsafe fn end_compute_pass(&mut self) {}

    unsafe fn set_compute_pipeline(&mut self, pipeline: &Resource) {}

    unsafe fn dispatch_workgroups(&mut self, count: [u32; 3]) {}
    unsafe fn dispatch_workgroups_indirect(&mut self, buffer: &Buffer, offset: wgt::BufferAddress) {
    }

    unsafe fn build_acceleration_structures<'a, T>(
        &mut self,
        _descriptor_count: u32,
        descriptors: T,
    ) where
        Api: 'a,
        T: IntoIterator<Item = crate::BuildAccelerationStructureDescriptor<'a, Buffer, Resource>>,
    {
    }

    unsafe fn place_acceleration_structure_barrier(
        &mut self,
        _barriers: crate::AccelerationStructureBarrier,
    ) {
    }

    unsafe fn copy_acceleration_structure_to_acceleration_structure(
        &mut self,
        src: &Resource,
        dst: &Resource,
        copy: wgt::AccelerationStructureCopy,
    ) {
    }

    unsafe fn set_acceleration_structure_dependencies(
        command_buffers: &[&CommandBuffer],
        dependencies: &[&Resource],
    ) {
    }

    unsafe fn begin_ray_tracing_pass(&mut self, _desc: &crate::RayTracingPassDescriptor) {}

    unsafe fn end_ray_tracing_pass(&mut self) {}

    unsafe fn set_ray_tracing_pipeline(
        &mut self,
        _pipeline: &<Self::A as crate::Api>::RayTracingPipeline,
    ) {
    }

    unsafe fn trace_rays(
        &mut self,
        _count: [u32; 3],
        _ray_generation_group_data: crate::PipelineGroupData<Buffer>,
        _miss_group_data: crate::PipelineGroupData<Buffer>,
        _intersection_group_data: crate::PipelineGroupData<Buffer>,
    ) {
    }
}

impl Command {
    /// # Safety
    ///
    /// Must be called with appropriate synchronization for the resources affected by the command,
    /// such as ensuring that buffers are not accessed by a command while aliasing references exist.
    unsafe fn execute(&self) {
        match self {
            Command::ClearBuffer { ref buffer, range } => {
                // SAFETY:
                // Caller is responsible for ensuring this does not alias.
                let buffer_slice: &mut [u8] = unsafe { &mut *buffer.get_slice_ptr(range.clone()) };
                buffer_slice.fill(0);
            }

            #[cfg(feature = "noop_test_hooks")]
            Command::WriteImage { image, write } => image.record_write(*write),

            Command::CopyBufferToBuffer { src, dst, regions } => {
                for &crate::BufferCopy {
                    src_offset,
                    dst_offset,
                    size,
                } in regions
                {
                    // SAFETY:
                    // Caller is responsible for ensuring this does not alias.
                    let src_region: &[u8] =
                        unsafe { &*src.get_slice_ptr(src_offset..src_offset + size.get()) };
                    let dst_region: &mut [u8] =
                        unsafe { &mut *dst.get_slice_ptr(dst_offset..dst_offset + size.get()) };
                    dst_region.copy_from_slice(src_region);
                }
            }
        }
    }
}
