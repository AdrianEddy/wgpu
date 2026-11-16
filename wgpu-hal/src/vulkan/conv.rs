use alloc::vec::Vec;

use ash::vk;

impl super::PrivateCapabilities {
    pub fn map_texture_format(&self, format: wgt::TextureFormat) -> vk::Format {
        use ash::vk::Format as F;
        use wgt::TextureFormat as Tf;
        use wgt::{AstcBlock, AstcChannel};
        match format {
            Tf::R8Unorm => F::R8_UNORM,
            Tf::R8Snorm => F::R8_SNORM,
            Tf::R8Uint => F::R8_UINT,
            Tf::R8Sint => F::R8_SINT,
            Tf::R16Uint => F::R16_UINT,
            Tf::R16Sint => F::R16_SINT,
            Tf::R16Unorm => F::R16_UNORM,
            Tf::R16Snorm => F::R16_SNORM,
            Tf::R16Float => F::R16_SFLOAT,
            Tf::Rg8Unorm => F::R8G8_UNORM,
            Tf::Rg8Snorm => F::R8G8_SNORM,
            Tf::Rg8Uint => F::R8G8_UINT,
            Tf::Rg8Sint => F::R8G8_SINT,
            Tf::Rg16Unorm => F::R16G16_UNORM,
            Tf::Rg16Snorm => F::R16G16_SNORM,
            Tf::R32Uint => F::R32_UINT,
            Tf::R32Sint => F::R32_SINT,
            Tf::R32Float => F::R32_SFLOAT,
            Tf::Rg16Uint => F::R16G16_UINT,
            Tf::Rg16Sint => F::R16G16_SINT,
            Tf::Rg16Float => F::R16G16_SFLOAT,
            Tf::Rgba8Unorm => F::R8G8B8A8_UNORM,
            Tf::Rgba8UnormSrgb => F::R8G8B8A8_SRGB,
            Tf::Bgra8UnormSrgb => F::B8G8R8A8_SRGB,
            Tf::Rgba8Snorm => F::R8G8B8A8_SNORM,
            Tf::Bgra8Unorm => F::B8G8R8A8_UNORM,
            Tf::Rgba8Uint => F::R8G8B8A8_UINT,
            Tf::Rgba8Sint => F::R8G8B8A8_SINT,
            Tf::Rgb10a2Uint => F::A2B10G10R10_UINT_PACK32,
            Tf::Rgb10a2Unorm => F::A2B10G10R10_UNORM_PACK32,
            Tf::Rg11b10Ufloat => F::B10G11R11_UFLOAT_PACK32,
            Tf::R64Uint => F::R64_UINT,
            Tf::Rg32Uint => F::R32G32_UINT,
            Tf::Rg32Sint => F::R32G32_SINT,
            Tf::Rg32Float => F::R32G32_SFLOAT,
            Tf::Rgba16Uint => F::R16G16B16A16_UINT,
            Tf::Rgba16Sint => F::R16G16B16A16_SINT,
            Tf::Rgba16Unorm => F::R16G16B16A16_UNORM,
            Tf::Rgba16Snorm => F::R16G16B16A16_SNORM,
            Tf::Rgba16Float => F::R16G16B16A16_SFLOAT,
            Tf::Rgba32Uint => F::R32G32B32A32_UINT,
            Tf::Rgba32Sint => F::R32G32B32A32_SINT,
            Tf::Rgba32Float => F::R32G32B32A32_SFLOAT,
            Tf::Depth32Float => F::D32_SFLOAT,
            Tf::Depth32FloatStencil8 => F::D32_SFLOAT_S8_UINT,
            Tf::Depth24Plus => {
                if self.texture_d24 {
                    F::X8_D24_UNORM_PACK32
                } else {
                    F::D32_SFLOAT
                }
            }
            Tf::Depth24PlusStencil8 => {
                if self.texture_d24_s8 {
                    F::D24_UNORM_S8_UINT
                } else {
                    F::D32_SFLOAT_S8_UINT
                }
            }
            Tf::Stencil8 => {
                if self.texture_s8 {
                    F::S8_UINT
                } else if self.texture_d24_s8 {
                    F::D24_UNORM_S8_UINT
                } else {
                    F::D32_SFLOAT_S8_UINT
                }
            }
            Tf::Depth16Unorm => F::D16_UNORM,
            Tf::NV12 => F::G8_B8R8_2PLANE_420_UNORM,
            Tf::P010 => F::G10X6_B10X6R10X6_2PLANE_420_UNORM_3PACK16,
            Tf::Rgb9e5Ufloat => F::E5B9G9R9_UFLOAT_PACK32,
            Tf::Bc1RgbaUnorm => F::BC1_RGBA_UNORM_BLOCK,
            Tf::Bc1RgbaUnormSrgb => F::BC1_RGBA_SRGB_BLOCK,
            Tf::Bc2RgbaUnorm => F::BC2_UNORM_BLOCK,
            Tf::Bc2RgbaUnormSrgb => F::BC2_SRGB_BLOCK,
            Tf::Bc3RgbaUnorm => F::BC3_UNORM_BLOCK,
            Tf::Bc3RgbaUnormSrgb => F::BC3_SRGB_BLOCK,
            Tf::Bc4RUnorm => F::BC4_UNORM_BLOCK,
            Tf::Bc4RSnorm => F::BC4_SNORM_BLOCK,
            Tf::Bc5RgUnorm => F::BC5_UNORM_BLOCK,
            Tf::Bc5RgSnorm => F::BC5_SNORM_BLOCK,
            Tf::Bc6hRgbUfloat => F::BC6H_UFLOAT_BLOCK,
            Tf::Bc6hRgbFloat => F::BC6H_SFLOAT_BLOCK,
            Tf::Bc7RgbaUnorm => F::BC7_UNORM_BLOCK,
            Tf::Bc7RgbaUnormSrgb => F::BC7_SRGB_BLOCK,
            Tf::Etc2Rgb8Unorm => F::ETC2_R8G8B8_UNORM_BLOCK,
            Tf::Etc2Rgb8UnormSrgb => F::ETC2_R8G8B8_SRGB_BLOCK,
            Tf::Etc2Rgb8A1Unorm => F::ETC2_R8G8B8A1_UNORM_BLOCK,
            Tf::Etc2Rgb8A1UnormSrgb => F::ETC2_R8G8B8A1_SRGB_BLOCK,
            Tf::Etc2Rgba8Unorm => F::ETC2_R8G8B8A8_UNORM_BLOCK,
            Tf::Etc2Rgba8UnormSrgb => F::ETC2_R8G8B8A8_SRGB_BLOCK,
            Tf::EacR11Unorm => F::EAC_R11_UNORM_BLOCK,
            Tf::EacR11Snorm => F::EAC_R11_SNORM_BLOCK,
            Tf::EacRg11Unorm => F::EAC_R11G11_UNORM_BLOCK,
            Tf::EacRg11Snorm => F::EAC_R11G11_SNORM_BLOCK,
            Tf::Astc { block, channel } => match channel {
                AstcChannel::Unorm => match block {
                    AstcBlock::B4x4 => F::ASTC_4X4_UNORM_BLOCK,
                    AstcBlock::B5x4 => F::ASTC_5X4_UNORM_BLOCK,
                    AstcBlock::B5x5 => F::ASTC_5X5_UNORM_BLOCK,
                    AstcBlock::B6x5 => F::ASTC_6X5_UNORM_BLOCK,
                    AstcBlock::B6x6 => F::ASTC_6X6_UNORM_BLOCK,
                    AstcBlock::B8x5 => F::ASTC_8X5_UNORM_BLOCK,
                    AstcBlock::B8x6 => F::ASTC_8X6_UNORM_BLOCK,
                    AstcBlock::B8x8 => F::ASTC_8X8_UNORM_BLOCK,
                    AstcBlock::B10x5 => F::ASTC_10X5_UNORM_BLOCK,
                    AstcBlock::B10x6 => F::ASTC_10X6_UNORM_BLOCK,
                    AstcBlock::B10x8 => F::ASTC_10X8_UNORM_BLOCK,
                    AstcBlock::B10x10 => F::ASTC_10X10_UNORM_BLOCK,
                    AstcBlock::B12x10 => F::ASTC_12X10_UNORM_BLOCK,
                    AstcBlock::B12x12 => F::ASTC_12X12_UNORM_BLOCK,
                },
                AstcChannel::UnormSrgb => match block {
                    AstcBlock::B4x4 => F::ASTC_4X4_SRGB_BLOCK,
                    AstcBlock::B5x4 => F::ASTC_5X4_SRGB_BLOCK,
                    AstcBlock::B5x5 => F::ASTC_5X5_SRGB_BLOCK,
                    AstcBlock::B6x5 => F::ASTC_6X5_SRGB_BLOCK,
                    AstcBlock::B6x6 => F::ASTC_6X6_SRGB_BLOCK,
                    AstcBlock::B8x5 => F::ASTC_8X5_SRGB_BLOCK,
                    AstcBlock::B8x6 => F::ASTC_8X6_SRGB_BLOCK,
                    AstcBlock::B8x8 => F::ASTC_8X8_SRGB_BLOCK,
                    AstcBlock::B10x5 => F::ASTC_10X5_SRGB_BLOCK,
                    AstcBlock::B10x6 => F::ASTC_10X6_SRGB_BLOCK,
                    AstcBlock::B10x8 => F::ASTC_10X8_SRGB_BLOCK,
                    AstcBlock::B10x10 => F::ASTC_10X10_SRGB_BLOCK,
                    AstcBlock::B12x10 => F::ASTC_12X10_SRGB_BLOCK,
                    AstcBlock::B12x12 => F::ASTC_12X12_SRGB_BLOCK,
                },
                AstcChannel::Hdr => match block {
                    AstcBlock::B4x4 => F::ASTC_4X4_SFLOAT_BLOCK_EXT,
                    AstcBlock::B5x4 => F::ASTC_5X4_SFLOAT_BLOCK_EXT,
                    AstcBlock::B5x5 => F::ASTC_5X5_SFLOAT_BLOCK_EXT,
                    AstcBlock::B6x5 => F::ASTC_6X5_SFLOAT_BLOCK_EXT,
                    AstcBlock::B6x6 => F::ASTC_6X6_SFLOAT_BLOCK_EXT,
                    AstcBlock::B8x5 => F::ASTC_8X5_SFLOAT_BLOCK_EXT,
                    AstcBlock::B8x6 => F::ASTC_8X6_SFLOAT_BLOCK_EXT,
                    AstcBlock::B8x8 => F::ASTC_8X8_SFLOAT_BLOCK_EXT,
                    AstcBlock::B10x5 => F::ASTC_10X5_SFLOAT_BLOCK_EXT,
                    AstcBlock::B10x6 => F::ASTC_10X6_SFLOAT_BLOCK_EXT,
                    AstcBlock::B10x8 => F::ASTC_10X8_SFLOAT_BLOCK_EXT,
                    AstcBlock::B10x10 => F::ASTC_10X10_SFLOAT_BLOCK_EXT,
                    AstcBlock::B12x10 => F::ASTC_12X10_SFLOAT_BLOCK_EXT,
                    AstcBlock::B12x12 => F::ASTC_12X12_SFLOAT_BLOCK_EXT,
                },
            },
        }
    }
}

pub fn map_vk_surface_formats(
    sf: vk::SurfaceFormatKHR,
) -> Option<(wgt::TextureFormat, wgt::SurfaceColorSpace)> {
    use ash::vk::Format as F;
    use wgt::TextureFormat as Tf;
    // Format list we care about pulled from https://vulkan.gpuinfo.org/listsurfaceformats.php.
    let format = match sf.format {
        F::B8G8R8A8_UNORM => Tf::Bgra8Unorm,
        F::B8G8R8A8_SRGB => Tf::Bgra8UnormSrgb,
        F::R8G8B8A8_SNORM => Tf::Rgba8Snorm,
        F::R8G8B8A8_UNORM => Tf::Rgba8Unorm,
        F::R8G8B8A8_SRGB => Tf::Rgba8UnormSrgb,
        F::R16G16B16A16_SNORM => Tf::Rgba16Snorm,
        F::R16G16B16A16_UNORM => Tf::Rgba16Unorm,
        F::R16G16B16A16_SFLOAT => Tf::Rgba16Float,
        F::A2B10G10R10_UNORM_PACK32 => Tf::Rgb10a2Unorm,
        _ => return None,
    };
    let color_space = map_vk_color_space(sf.color_space)?;
    Some((format, color_space))
}

pub fn map_vk_color_space(color_space: vk::ColorSpaceKHR) -> Option<wgt::SurfaceColorSpace> {
    use wgt::SurfaceColorSpace as Scs;
    Some(match color_space {
        vk::ColorSpaceKHR::SRGB_NONLINEAR => Scs::Srgb,
        vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT => Scs::ExtendedSrgbLinear,
        vk::ColorSpaceKHR::EXTENDED_SRGB_NONLINEAR_EXT => Scs::ExtendedSrgb,
        vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT => Scs::DisplayP3,
        vk::ColorSpaceKHR::HDR10_ST2084_EXT => Scs::Bt2100Pq,
        vk::ColorSpaceKHR::HDR10_HLG_EXT => Scs::Bt2100Hlg,
        _ => return None,
    })
}

pub fn map_surface_color_space(color_space: wgt::SurfaceColorSpace) -> vk::ColorSpaceKHR {
    use wgt::SurfaceColorSpace as Scs;
    match color_space {
        Scs::Auto => unreachable!("wgpu-core resolves `Auto` before configuring the surface"),
        Scs::Srgb => vk::ColorSpaceKHR::SRGB_NONLINEAR,
        Scs::ExtendedSrgbLinear => vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
        Scs::ExtendedSrgb => vk::ColorSpaceKHR::EXTENDED_SRGB_NONLINEAR_EXT,
        Scs::DisplayP3 => vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT,
        Scs::Bt2100Pq => vk::ColorSpaceKHR::HDR10_ST2084_EXT,
        Scs::Bt2100Hlg => vk::ColorSpaceKHR::HDR10_HLG_EXT,
        Scs::ExtendedDisplayP3 => {
            unreachable!("`ExtendedDisplayP3` is never reported in the Vulkan surface capabilities")
        }
    }
}

impl crate::Attachment<'_, super::TextureView> {
    pub(super) fn make_attachment_key(&self, ops: crate::AttachmentOps) -> super::AttachmentKey {
        super::AttachmentKey {
            format: self.view.raw_format,
            layout: derive_image_layout(self.usage, self.view.format),
            ops,
        }
    }
}

impl crate::ColorAttachment<'_, super::TextureView> {
    pub(super) unsafe fn make_vk_clear_color(&self) -> vk::ClearColorValue {
        let cv = &self.clear_value;
        match self.target.view.format.sample_type(None, None).unwrap() {
            wgt::TextureSampleType::Float { .. } => vk::ClearColorValue {
                float32: [cv.r as f32, cv.g as f32, cv.b as f32, cv.a as f32],
            },
            wgt::TextureSampleType::Sint => vk::ClearColorValue {
                int32: [cv.r as i32, cv.g as i32, cv.b as i32, cv.a as i32],
            },
            wgt::TextureSampleType::Uint => vk::ClearColorValue {
                uint32: [cv.r as u32, cv.g as u32, cv.b as u32, cv.a as u32],
            },
            wgt::TextureSampleType::Depth => unreachable!(),
        }
    }
}

pub fn derive_image_layout(usage: wgt::TextureUses, format: wgt::TextureFormat) -> vk::ImageLayout {
    let is_color = !format.is_depth_stencil_format();
    match usage {
        wgt::TextureUses::UNINITIALIZED => vk::ImageLayout::UNDEFINED,
        wgt::TextureUses::COPY_SRC => vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        wgt::TextureUses::COPY_DST => vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        wgt::TextureUses::RESOURCE if is_color => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        wgt::TextureUses::COLOR_TARGET => vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        _ => {
            if usage == wgt::TextureUses::PRESENT {
                vk::ImageLayout::PRESENT_SRC_KHR
            } else if is_color {
                vk::ImageLayout::GENERAL
            } else {
                match (
                    usage.contains(wgt::TextureUses::DEPTH_WRITE),
                    usage.contains(wgt::TextureUses::STENCIL_WRITE),
                ) {
                    (true, true) => vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                    (false, true) => vk::ImageLayout::DEPTH_READ_ONLY_STENCIL_ATTACHMENT_OPTIMAL,
                    (true, false) => vk::ImageLayout::DEPTH_ATTACHMENT_STENCIL_READ_ONLY_OPTIMAL,
                    (false, false) => vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL,
                }
            }
        }
    }
}

pub fn map_queue_family(family: crate::QueueFamily) -> u32 {
    match family {
        crate::QueueFamily::Explicit(index) => index,
        crate::QueueFamily::External => vk::QUEUE_FAMILY_EXTERNAL,
        crate::QueueFamily::Foreign => vk::QUEUE_FAMILY_FOREIGN_EXT,
    }
}

/// Image usages of which an image must have at least one to be viewable, per
/// `VUID-VkImageViewCreateInfo-image-04441`.
pub const VIEWABLE_IMAGE_USAGES: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::SAMPLED.as_raw()
        | vk::ImageUsageFlags::STORAGE.as_raw()
        | vk::ImageUsageFlags::COLOR_ATTACHMENT.as_raw()
        | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT.as_raw()
        | vk::ImageUsageFlags::INPUT_ATTACHMENT.as_raw()
        | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT.as_raw(),
);

pub fn map_texture_usage(usage: wgt::TextureUses) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
    if usage.contains(wgt::TextureUses::COPY_SRC) {
        flags |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(wgt::TextureUses::COPY_DST) {
        flags |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if usage.contains(wgt::TextureUses::RESOURCE) {
        flags |= vk::ImageUsageFlags::SAMPLED;
    }
    if usage.contains(wgt::TextureUses::COLOR_TARGET) {
        flags |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    if usage.intersects(
        wgt::TextureUses::DEPTH_READ
            | wgt::TextureUses::DEPTH_WRITE
            | wgt::TextureUses::STENCIL_READ
            | wgt::TextureUses::STENCIL_WRITE,
    ) {
        flags |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
    }
    if usage.intersects(
        wgt::TextureUses::STORAGE_READ_ONLY
            | wgt::TextureUses::STORAGE_WRITE_ONLY
            | wgt::TextureUses::STORAGE_READ_WRITE
            | wgt::TextureUses::STORAGE_ATOMIC,
    ) {
        flags |= vk::ImageUsageFlags::STORAGE;
    }
    if usage.contains(wgt::TextureUses::TRANSIENT) {
        flags |= vk::ImageUsageFlags::TRANSIENT_ATTACHMENT;
    }
    flags
}

pub fn map_texture_usage_to_barrier(
    usage: wgt::TextureUses,
    queue_flags: vk::QueueFlags,
    support_store_op_none: bool,
    features: wgt::Features,
) -> (vk::PipelineStageFlags, vk::AccessFlags) {
    let mut stages = vk::PipelineStageFlags::empty();
    let mut access = vk::AccessFlags::empty();
    let shader_stages = shader_stages(queue_flags, features);

    if usage.contains(wgt::TextureUses::COPY_SRC) {
        stages |= vk::PipelineStageFlags::TRANSFER;
        access |= vk::AccessFlags::TRANSFER_READ;
    }
    if usage.contains(wgt::TextureUses::COPY_DST) {
        stages |= vk::PipelineStageFlags::TRANSFER;
        access |= vk::AccessFlags::TRANSFER_WRITE;
    }
    if usage.contains(wgt::TextureUses::RESOURCE) {
        stages |= shader_stages;
        access |= vk::AccessFlags::SHADER_READ;
    }
    if usage.contains(wgt::TextureUses::COLOR_TARGET) {
        stages |= vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT;
        access |= vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE;
    }
    if usage.intersects(wgt::TextureUses::DEPTH_READ | wgt::TextureUses::STENCIL_READ) {
        stages |= vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
        access |= if !support_store_op_none {
            // If `vk::AttachmentStoreOp::NONE` isn't available we use `vk::AttachmentStoreOp::Store`
            // for readonly depth-stencil attachments, which needs write access to avoid validation error.
            vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE
        } else {
            vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
        }
    }
    if usage.intersects(wgt::TextureUses::DEPTH_WRITE | wgt::TextureUses::STENCIL_WRITE) {
        stages |= vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
            | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS;
        access |= vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
            | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE;
    }
    if usage.intersects(
        wgt::TextureUses::STORAGE_READ_ONLY
            | wgt::TextureUses::STORAGE_READ_WRITE
            | wgt::TextureUses::STORAGE_ATOMIC,
    ) {
        stages |= shader_stages;
        access |= vk::AccessFlags::SHADER_READ;
    }
    if usage.intersects(
        wgt::TextureUses::STORAGE_WRITE_ONLY
            | wgt::TextureUses::STORAGE_READ_WRITE
            | wgt::TextureUses::STORAGE_ATOMIC,
    ) {
        stages |= shader_stages;
        access |= vk::AccessFlags::SHADER_WRITE;
    }

    if usage == wgt::TextureUses::UNINITIALIZED || usage == wgt::TextureUses::PRESENT {
        (
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::AccessFlags::empty(),
        )
    } else {
        (stages, access)
    }
}

pub fn map_vk_image_usage(usage: vk::ImageUsageFlags) -> wgt::TextureUses {
    let mut bits = wgt::TextureUses::empty();
    if usage.contains(vk::ImageUsageFlags::TRANSFER_SRC) {
        bits |= wgt::TextureUses::COPY_SRC;
    }
    if usage.contains(vk::ImageUsageFlags::TRANSFER_DST) {
        bits |= wgt::TextureUses::COPY_DST;
    }
    if usage.contains(vk::ImageUsageFlags::SAMPLED) {
        bits |= wgt::TextureUses::RESOURCE;
    }
    if usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
        bits |= wgt::TextureUses::COLOR_TARGET;
    }
    if usage.contains(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT) {
        bits |= wgt::TextureUses::DEPTH_READ
            | wgt::TextureUses::DEPTH_WRITE
            | wgt::TextureUses::STENCIL_READ
            | wgt::TextureUses::STENCIL_WRITE;
    }
    if usage.contains(vk::ImageUsageFlags::STORAGE) {
        bits |= wgt::TextureUses::STORAGE_READ_ONLY
            | wgt::TextureUses::STORAGE_WRITE_ONLY
            | wgt::TextureUses::STORAGE_READ_WRITE
            | wgt::TextureUses::STORAGE_ATOMIC;
    }
    if usage.contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT) {
        bits |= wgt::TextureUses::TRANSIENT;
    }
    bits
}

pub fn map_texture_dimension(dim: wgt::TextureDimension) -> vk::ImageType {
    match dim {
        wgt::TextureDimension::D1 => vk::ImageType::TYPE_1D,
        wgt::TextureDimension::D2 => vk::ImageType::TYPE_2D,
        wgt::TextureDimension::D3 => vk::ImageType::TYPE_3D,
    }
}

pub fn map_index_format(index_format: wgt::IndexFormat) -> vk::IndexType {
    match index_format {
        wgt::IndexFormat::Uint16 => vk::IndexType::UINT16,
        wgt::IndexFormat::Uint32 => vk::IndexType::UINT32,
    }
}

pub fn map_vertex_format(vertex_format: wgt::VertexFormat) -> vk::Format {
    use wgt::VertexFormat as Vf;
    match vertex_format {
        Vf::Uint8 => vk::Format::R8_UINT,
        Vf::Uint8x2 => vk::Format::R8G8_UINT,
        Vf::Uint8x4 => vk::Format::R8G8B8A8_UINT,
        Vf::Sint8 => vk::Format::R8_SINT,
        Vf::Sint8x2 => vk::Format::R8G8_SINT,
        Vf::Sint8x4 => vk::Format::R8G8B8A8_SINT,
        Vf::Unorm8 => vk::Format::R8_UNORM,
        Vf::Unorm8x2 => vk::Format::R8G8_UNORM,
        Vf::Unorm8x4 => vk::Format::R8G8B8A8_UNORM,
        Vf::Snorm8 => vk::Format::R8_SNORM,
        Vf::Snorm8x2 => vk::Format::R8G8_SNORM,
        Vf::Snorm8x4 => vk::Format::R8G8B8A8_SNORM,
        Vf::Uint16 => vk::Format::R16_UINT,
        Vf::Uint16x2 => vk::Format::R16G16_UINT,
        Vf::Uint16x4 => vk::Format::R16G16B16A16_UINT,
        Vf::Sint16 => vk::Format::R16_SINT,
        Vf::Sint16x2 => vk::Format::R16G16_SINT,
        Vf::Sint16x4 => vk::Format::R16G16B16A16_SINT,
        Vf::Unorm16 => vk::Format::R16_UNORM,
        Vf::Unorm16x2 => vk::Format::R16G16_UNORM,
        Vf::Unorm16x4 => vk::Format::R16G16B16A16_UNORM,
        Vf::Snorm16 => vk::Format::R16_SNORM,
        Vf::Snorm16x2 => vk::Format::R16G16_SNORM,
        Vf::Snorm16x4 => vk::Format::R16G16B16A16_SNORM,
        Vf::Float16 => vk::Format::R16_SFLOAT,
        Vf::Float16x2 => vk::Format::R16G16_SFLOAT,
        Vf::Float16x4 => vk::Format::R16G16B16A16_SFLOAT,
        Vf::Float32 => vk::Format::R32_SFLOAT,
        Vf::Float32x2 => vk::Format::R32G32_SFLOAT,
        Vf::Float32x3 => vk::Format::R32G32B32_SFLOAT,
        Vf::Float32x4 => vk::Format::R32G32B32A32_SFLOAT,
        Vf::Uint32 => vk::Format::R32_UINT,
        Vf::Uint32x2 => vk::Format::R32G32_UINT,
        Vf::Uint32x3 => vk::Format::R32G32B32_UINT,
        Vf::Uint32x4 => vk::Format::R32G32B32A32_UINT,
        Vf::Sint32 => vk::Format::R32_SINT,
        Vf::Sint32x2 => vk::Format::R32G32_SINT,
        Vf::Sint32x3 => vk::Format::R32G32B32_SINT,
        Vf::Sint32x4 => vk::Format::R32G32B32A32_SINT,
        Vf::Float64 => vk::Format::R64_SFLOAT,
        Vf::Float64x2 => vk::Format::R64G64_SFLOAT,
        Vf::Float64x3 => vk::Format::R64G64B64_SFLOAT,
        Vf::Float64x4 => vk::Format::R64G64B64A64_SFLOAT,
        Vf::Unorm10_10_10_2 => vk::Format::A2B10G10R10_UNORM_PACK32,
        // Note that, unlike the unorm variant, this format has no mandatory format
        // features in Vulkan, so vertex buffer support is not guaranteed.
        Vf::Snorm10_10_10_2 => vk::Format::A2B10G10R10_SNORM_PACK32,
        Vf::Unorm8x4Bgra => vk::Format::B8G8R8A8_UNORM,
    }
}

pub fn map_aspects(aspects: crate::FormatAspects) -> vk::ImageAspectFlags {
    let mut flags = vk::ImageAspectFlags::empty();
    if aspects.contains(crate::FormatAspects::COLOR) {
        flags |= vk::ImageAspectFlags::COLOR;
    }
    if aspects.contains(crate::FormatAspects::DEPTH) {
        flags |= vk::ImageAspectFlags::DEPTH;
    }
    if aspects.contains(crate::FormatAspects::STENCIL) {
        flags |= vk::ImageAspectFlags::STENCIL;
    }
    if aspects.contains(crate::FormatAspects::PLANE_0) {
        flags |= vk::ImageAspectFlags::PLANE_0;
    }
    if aspects.contains(crate::FormatAspects::PLANE_1) {
        flags |= vk::ImageAspectFlags::PLANE_1;
    }
    if aspects.contains(crate::FormatAspects::PLANE_2) {
        flags |= vk::ImageAspectFlags::PLANE_2;
    }
    flags
}

pub fn map_attachment_ops(
    op: crate::AttachmentOps,
) -> (vk::AttachmentLoadOp, vk::AttachmentStoreOp) {
    let load_op = if op.contains(crate::AttachmentOps::LOAD) {
        vk::AttachmentLoadOp::LOAD
    } else if op.contains(crate::AttachmentOps::LOAD_DONT_CARE) {
        vk::AttachmentLoadOp::DONT_CARE
    } else if op.contains(crate::AttachmentOps::LOAD_CLEAR) {
        vk::AttachmentLoadOp::CLEAR
    } else {
        unreachable!()
    };
    let store_op = if op.contains(crate::AttachmentOps::STORE) {
        vk::AttachmentStoreOp::STORE
    } else if op.contains(crate::AttachmentOps::STORE_DISCARD) {
        vk::AttachmentStoreOp::DONT_CARE
    } else {
        unreachable!()
    };
    (load_op, store_op)
}

pub fn map_present_mode(mode: wgt::PresentMode) -> vk::PresentModeKHR {
    match mode {
        wgt::PresentMode::Immediate => vk::PresentModeKHR::IMMEDIATE,
        wgt::PresentMode::Mailbox => vk::PresentModeKHR::MAILBOX,
        wgt::PresentMode::Fifo => vk::PresentModeKHR::FIFO,
        wgt::PresentMode::FifoRelaxed => vk::PresentModeKHR::FIFO_RELAXED,
        wgt::PresentMode::AutoNoVsync | wgt::PresentMode::AutoVsync => {
            unreachable!("Cannot create swapchain with Auto PresentationMode")
        }
    }
}

pub fn map_vk_present_mode(mode: vk::PresentModeKHR) -> Option<wgt::PresentMode> {
    // Not exposed in Ash yet.
    const FIFO_LATEST_READY: vk::PresentModeKHR = vk::PresentModeKHR::from_raw(1_000_361_000);

    // See https://registry.khronos.org/vulkan/specs/latest/man/html/VkPresentModeKHR.html
    match mode {
        vk::PresentModeKHR::IMMEDIATE => Some(wgt::PresentMode::Immediate),
        vk::PresentModeKHR::MAILBOX => Some(wgt::PresentMode::Mailbox),
        vk::PresentModeKHR::FIFO => Some(wgt::PresentMode::Fifo),
        vk::PresentModeKHR::FIFO_RELAXED => Some(wgt::PresentMode::FifoRelaxed),

        // Modes that aren't exposed yet.
        vk::PresentModeKHR::SHARED_DEMAND_REFRESH => None,
        vk::PresentModeKHR::SHARED_CONTINUOUS_REFRESH => None,
        FIFO_LATEST_READY => None,

        _ => {
            log::debug!("Unrecognized present mode {mode:?}");
            None
        }
    }
}

pub fn map_composite_alpha_mode(mode: wgt::CompositeAlphaMode) -> vk::CompositeAlphaFlagsKHR {
    match mode {
        wgt::CompositeAlphaMode::Opaque => vk::CompositeAlphaFlagsKHR::OPAQUE,
        wgt::CompositeAlphaMode::PreMultiplied => vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        wgt::CompositeAlphaMode::PostMultiplied => vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        wgt::CompositeAlphaMode::Inherit => vk::CompositeAlphaFlagsKHR::INHERIT,
        wgt::CompositeAlphaMode::Auto => unreachable!(),
    }
}

pub fn map_vk_composite_alpha(flags: vk::CompositeAlphaFlagsKHR) -> Vec<wgt::CompositeAlphaMode> {
    let mut modes = Vec::new();
    if flags.contains(vk::CompositeAlphaFlagsKHR::OPAQUE) {
        modes.push(wgt::CompositeAlphaMode::Opaque);
    }
    if flags.contains(vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED) {
        modes.push(wgt::CompositeAlphaMode::PreMultiplied);
    }
    if flags.contains(vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED) {
        modes.push(wgt::CompositeAlphaMode::PostMultiplied);
    }
    if flags.contains(vk::CompositeAlphaFlagsKHR::INHERIT) {
        modes.push(wgt::CompositeAlphaMode::Inherit);
    }
    modes
}

/// The present scaling and gravity of a scaling mode, or `None` for
/// [`wgt::SurfaceScalingMode::Auto`], the platform's own placement. The gravity is empty where
/// the mode does not depend on it: a stretch maps the whole image onto the whole surface. Both
/// axes take the same gravity (VUID-VkSwapchainPresentScalingCreateInfoKHR-presentGravityX-07765
/// and -07766 forbid setting only one).
pub fn map_surface_scaling_mode(
    mode: wgt::SurfaceScalingMode,
) -> Option<(vk::PresentScalingFlagsEXT, vk::PresentGravityFlagsEXT)> {
    match mode {
        wgt::SurfaceScalingMode::Auto => None,
        wgt::SurfaceScalingMode::Stretch => Some((
            vk::PresentScalingFlagsEXT::STRETCH,
            vk::PresentGravityFlagsEXT::empty(),
        )),
        wgt::SurfaceScalingMode::AspectRatio => Some((
            vk::PresentScalingFlagsEXT::ASPECT_RATIO_STRETCH,
            vk::PresentGravityFlagsEXT::CENTERED,
        )),
        wgt::SurfaceScalingMode::OneToOne => Some((
            vk::PresentScalingFlagsEXT::ONE_TO_ONE,
            vk::PresentGravityFlagsEXT::MIN,
        )),
    }
}

/// What a surface supports of present scaling with one present mode: the members of its
/// `VkSurfacePresentScalingCapabilitiesKHR`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PresentScalingSupport {
    pub scaling: vk::PresentScalingFlagsEXT,
    pub gravity_x: vk::PresentGravityFlagsEXT,
    pub gravity_y: vk::PresentGravityFlagsEXT,
}

impl PresentScalingSupport {
    pub fn of(caps: &vk::SurfacePresentScalingCapabilitiesEXT<'_>) -> Self {
        Self {
            scaling: caps.supported_present_scaling,
            gravity_x: caps.supported_present_gravity_x,
            gravity_y: caps.supported_present_gravity_y,
        }
    }
}

/// Whether a swapchain of `extent` is in the surface's range for scaled presentation, from
/// `min_scaled_image_extent` to `max_scaled_image_extent`
/// (VUID-VkSwapchainCreateInfoKHR-pNext-07782). Either end may be the special value
/// `(0xFFFFFFFF, 0xFFFFFFFF)`, "indicating that the surface size will be determined by the extent
/// of a swapchain targeting the surface": that end does not bound it.
pub fn extent_within_scaled_range(
    caps: &vk::SurfacePresentScalingCapabilitiesEXT<'_>,
    extent: wgt::Extent3d,
) -> bool {
    let unbounded = |e: vk::Extent2D| e.width == u32::MAX && e.height == u32::MAX;
    let min = caps.min_scaled_image_extent;
    let max = caps.max_scaled_image_extent;
    (unbounded(min) || (min.width <= extent.width && min.height <= extent.height))
        && (unbounded(max) || (extent.width <= max.width && extent.height <= max.height))
}

/// The scaling modes, other than [`wgt::SurfaceScalingMode::Auto`], that a surface supports with
/// every present mode whose support `per_present_mode` holds: those whose scaling each lists, with
/// the gravity on both axes that the mode needs (a stretch, which fills the surface, needs
/// none). Empty for no present mode.
pub fn surface_scaling_modes(
    per_present_mode: &[PresentScalingSupport],
) -> Vec<wgt::SurfaceScalingMode> {
    if per_present_mode.is_empty() {
        return Vec::new();
    }
    [
        wgt::SurfaceScalingMode::Stretch,
        wgt::SurfaceScalingMode::AspectRatio,
        wgt::SurfaceScalingMode::OneToOne,
    ]
    .into_iter()
    .filter(|&mode| {
        let (scaling, gravity) =
            map_surface_scaling_mode(mode).expect("only `Auto` maps to no scaling");
        per_present_mode.iter().all(|support| {
            support.scaling.contains(scaling)
                && (gravity.is_empty()
                    || (support.gravity_x.contains(gravity) && support.gravity_y.contains(gravity)))
        })
    })
    .collect()
}

pub fn map_buffer_usage(usage: wgt::BufferUses) -> vk::BufferUsageFlags {
    let mut flags = vk::BufferUsageFlags::empty();
    if usage.contains(wgt::BufferUses::COPY_SRC) {
        flags |= vk::BufferUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(wgt::BufferUses::COPY_DST) {
        flags |= vk::BufferUsageFlags::TRANSFER_DST;
    }
    if usage.contains(wgt::BufferUses::UNIFORM) {
        flags |= vk::BufferUsageFlags::UNIFORM_BUFFER;
    }
    if usage.intersects(wgt::BufferUses::STORAGE_READ_ONLY | wgt::BufferUses::STORAGE_READ_WRITE) {
        flags |= vk::BufferUsageFlags::STORAGE_BUFFER;
    }
    if usage.contains(wgt::BufferUses::INDEX) {
        flags |= vk::BufferUsageFlags::INDEX_BUFFER;
    }
    if usage.contains(wgt::BufferUses::VERTEX) {
        flags |= vk::BufferUsageFlags::VERTEX_BUFFER;
    }
    if usage.contains(wgt::BufferUses::INDIRECT) {
        flags |= vk::BufferUsageFlags::INDIRECT_BUFFER;
    }
    if usage.contains(wgt::BufferUses::ACCELERATION_STRUCTURE_SCRATCH) {
        flags |= vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    }
    if usage.intersects(
        wgt::BufferUses::BOTTOM_LEVEL_ACCELERATION_STRUCTURE_INPUT
            | wgt::BufferUses::TOP_LEVEL_ACCELERATION_STRUCTURE_INPUT,
    ) {
        flags |= vk::BufferUsageFlags::ACCELERATION_STRUCTURE_BUILD_INPUT_READ_ONLY_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    }
    if usage.intersects(wgt::BufferUses::ACCELERATION_STRUCTURE_QUERY) {
        flags |= vk::BufferUsageFlags::TRANSFER_DST;
    }
    if usage.intersects(wgt::BufferUses::RAY_TRACING_PIPELINE_SHADER_DATA) {
        flags |= vk::BufferUsageFlags::SHADER_BINDING_TABLE_KHR
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;
    }
    flags
}

pub fn map_buffer_usage_to_barrier(
    usage: wgt::BufferUses,
    queue_flags: vk::QueueFlags,
    features: wgt::Features,
) -> (vk::PipelineStageFlags, vk::AccessFlags) {
    let mut stages = vk::PipelineStageFlags::empty();
    let mut access = vk::AccessFlags::empty();
    let shader_stages = shader_stages(queue_flags, features);

    if usage.contains(wgt::BufferUses::MAP_READ) {
        stages |= vk::PipelineStageFlags::HOST;
        access |= vk::AccessFlags::HOST_READ;
    }
    if usage.contains(wgt::BufferUses::MAP_WRITE) {
        stages |= vk::PipelineStageFlags::HOST;
        access |= vk::AccessFlags::HOST_WRITE;
    }
    if usage.contains(wgt::BufferUses::COPY_SRC) {
        stages |= vk::PipelineStageFlags::TRANSFER;
        access |= vk::AccessFlags::TRANSFER_READ;
    }
    if usage.contains(wgt::BufferUses::COPY_DST) {
        stages |= vk::PipelineStageFlags::TRANSFER;
        access |= vk::AccessFlags::TRANSFER_WRITE;
    }
    if usage.contains(wgt::BufferUses::UNIFORM) {
        stages |= shader_stages;
        access |= vk::AccessFlags::UNIFORM_READ;
    }
    if usage.intersects(wgt::BufferUses::STORAGE_READ_ONLY) {
        stages |= shader_stages;
        access |= vk::AccessFlags::SHADER_READ;
    }
    if usage.intersects(wgt::BufferUses::STORAGE_READ_WRITE) {
        stages |= shader_stages;
        access |= vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE;
    }
    if usage.contains(wgt::BufferUses::INDEX) {
        stages |= vk::PipelineStageFlags::VERTEX_INPUT;
        access |= vk::AccessFlags::INDEX_READ;
    }
    if usage.contains(wgt::BufferUses::VERTEX) {
        stages |= vk::PipelineStageFlags::VERTEX_INPUT;
        access |= vk::AccessFlags::VERTEX_ATTRIBUTE_READ;
    }
    if usage.contains(wgt::BufferUses::INDIRECT) {
        stages |= vk::PipelineStageFlags::DRAW_INDIRECT;
        access |= vk::AccessFlags::INDIRECT_COMMAND_READ;
    }
    if usage.intersects(
        wgt::BufferUses::BOTTOM_LEVEL_ACCELERATION_STRUCTURE_INPUT
            | wgt::BufferUses::TOP_LEVEL_ACCELERATION_STRUCTURE_INPUT
            | wgt::BufferUses::ACCELERATION_STRUCTURE_SCRATCH,
    ) {
        stages |= vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR
            | vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR
            | vk::AccessFlags::SHADER_READ;
    }
    if usage.contains(wgt::BufferUses::ACCELERATION_STRUCTURE_QUERY) {
        stages |= vk::PipelineStageFlags::TRANSFER;
        access |= vk::AccessFlags::TRANSFER_WRITE;
    }
    if usage.contains(wgt::BufferUses::RAY_TRACING_PIPELINE_SHADER_DATA) {
        stages |= vk::PipelineStageFlags::RAY_TRACING_SHADER_KHR;
        access |= vk::AccessFlags::SHADER_READ;
    }

    (stages, access)
}

fn shader_stages(queue_flags: vk::QueueFlags, features: wgt::Features) -> vk::PipelineStageFlags {
    let mut stages = vk::PipelineStageFlags::empty();

    if queue_flags.contains(vk::QueueFlags::GRAPHICS) {
        stages |= vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER;
    }
    if queue_flags.contains(vk::QueueFlags::COMPUTE) {
        stages |= vk::PipelineStageFlags::COMPUTE_SHADER;
        // `cmd_trace_rays` counts as compute according to the vulkan spec.
        // https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdTraceRaysKHR.html#VUID-vkCmdTraceRaysKHR-commandBuffer-cmdpool
        if features.contains(wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES) {
            stages |= vk::PipelineStageFlags::RAY_TRACING_SHADER_KHR;
        }
    }

    stages
}

pub fn map_view_dimension(dim: wgt::TextureViewDimension) -> vk::ImageViewType {
    match dim {
        wgt::TextureViewDimension::D1 => vk::ImageViewType::TYPE_1D,
        wgt::TextureViewDimension::D2 => vk::ImageViewType::TYPE_2D,
        wgt::TextureViewDimension::D2Array => vk::ImageViewType::TYPE_2D_ARRAY,
        wgt::TextureViewDimension::Cube => vk::ImageViewType::CUBE,
        wgt::TextureViewDimension::CubeArray => vk::ImageViewType::CUBE_ARRAY,
        wgt::TextureViewDimension::D3 => vk::ImageViewType::TYPE_3D,
    }
}

pub fn map_copy_extent(extent: &crate::CopyExtent) -> vk::Extent3D {
    vk::Extent3D {
        width: extent.width,
        height: extent.height,
        depth: extent.depth,
    }
}

pub fn map_subresource_range(
    range: &wgt::ImageSubresourceRange,
    format: wgt::TextureFormat,
) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: map_aspects(crate::FormatAspects::new(format, range.aspect)),
        base_mip_level: range.base_mip_level,
        level_count: range.mip_level_count.unwrap_or(vk::REMAINING_MIP_LEVELS),
        base_array_layer: range.base_array_layer,
        layer_count: range
            .array_layer_count
            .unwrap_or(vk::REMAINING_ARRAY_LAYERS),
    }
}

// Special subresource range mapping for dealing with barriers
// so that we account for the "hidden" depth aspect in emulated Stencil8.
pub(super) fn map_subresource_range_combined_aspect(
    range: &wgt::ImageSubresourceRange,
    format: wgt::TextureFormat,
    private_caps: &super::PrivateCapabilities,
) -> vk::ImageSubresourceRange {
    let mut range = map_subresource_range(range, format);
    if !private_caps.texture_s8 && format == wgt::TextureFormat::Stencil8 {
        range.aspect_mask |= vk::ImageAspectFlags::DEPTH;
    }
    range
}

pub fn map_subresource_layers(
    base: &crate::TextureCopyBase,
) -> (vk::ImageSubresourceLayers, vk::Offset3D) {
    let offset = vk::Offset3D {
        x: base.origin.x as i32,
        y: base.origin.y as i32,
        z: base.origin.z as i32,
    };
    let subresource = vk::ImageSubresourceLayers {
        aspect_mask: map_aspects(base.aspect),
        mip_level: base.mip_level,
        base_array_layer: base.array_layer,
        layer_count: 1,
    };
    (subresource, offset)
}

pub fn map_filter_mode(mode: wgt::FilterMode) -> vk::Filter {
    match mode {
        wgt::FilterMode::Nearest => vk::Filter::NEAREST,
        wgt::FilterMode::Linear => vk::Filter::LINEAR,
    }
}

pub fn map_mip_filter_mode(mode: wgt::MipmapFilterMode) -> vk::SamplerMipmapMode {
    match mode {
        wgt::MipmapFilterMode::Nearest => vk::SamplerMipmapMode::NEAREST,
        wgt::MipmapFilterMode::Linear => vk::SamplerMipmapMode::LINEAR,
    }
}

pub fn map_address_mode(mode: wgt::AddressMode) -> vk::SamplerAddressMode {
    match mode {
        wgt::AddressMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        wgt::AddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
        wgt::AddressMode::MirrorRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        wgt::AddressMode::ClampToBorder => vk::SamplerAddressMode::CLAMP_TO_BORDER,
        // wgt::AddressMode::MirrorClamp => vk::SamplerAddressMode::MIRROR_CLAMP_TO_EDGE,
    }
}

pub fn map_border_color(border_color: wgt::SamplerBorderColor) -> vk::BorderColor {
    match border_color {
        wgt::SamplerBorderColor::TransparentBlack | wgt::SamplerBorderColor::Zero => {
            vk::BorderColor::FLOAT_TRANSPARENT_BLACK
        }
        wgt::SamplerBorderColor::OpaqueBlack => vk::BorderColor::FLOAT_OPAQUE_BLACK,
        wgt::SamplerBorderColor::OpaqueWhite => vk::BorderColor::FLOAT_OPAQUE_WHITE,
    }
}

pub fn map_comparison(fun: wgt::CompareFunction) -> vk::CompareOp {
    use wgt::CompareFunction as Cf;
    match fun {
        Cf::Never => vk::CompareOp::NEVER,
        Cf::Less => vk::CompareOp::LESS,
        Cf::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
        Cf::Equal => vk::CompareOp::EQUAL,
        Cf::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
        Cf::Greater => vk::CompareOp::GREATER,
        Cf::NotEqual => vk::CompareOp::NOT_EQUAL,
        Cf::Always => vk::CompareOp::ALWAYS,
    }
}

pub fn map_shader_stage(stage: wgt::ShaderStages) -> vk::ShaderStageFlags {
    let mut flags = vk::ShaderStageFlags::empty();
    if stage.contains(wgt::ShaderStages::VERTEX) {
        flags |= vk::ShaderStageFlags::VERTEX;
    }
    if stage.contains(wgt::ShaderStages::FRAGMENT) {
        flags |= vk::ShaderStageFlags::FRAGMENT;
    }
    if stage.contains(wgt::ShaderStages::COMPUTE) {
        flags |= vk::ShaderStageFlags::COMPUTE;
    }
    if stage.contains(wgt::ShaderStages::TASK) {
        flags |= vk::ShaderStageFlags::TASK_EXT;
    }
    if stage.contains(wgt::ShaderStages::MESH) {
        flags |= vk::ShaderStageFlags::MESH_EXT;
    }
    if stage.contains(wgt::ShaderStages::RAY_GENERATION) {
        flags |= vk::ShaderStageFlags::RAYGEN_KHR;
    }
    if stage.contains(wgt::ShaderStages::MISS) {
        flags |= vk::ShaderStageFlags::MISS_KHR;
    }
    if stage.contains(wgt::ShaderStages::ANY_HIT) {
        flags |= vk::ShaderStageFlags::ANY_HIT_KHR;
    }
    if stage.contains(wgt::ShaderStages::CLOSEST_HIT) {
        flags |= vk::ShaderStageFlags::CLOSEST_HIT_KHR;
    }
    flags
}

pub fn map_binding_type(ty: wgt::BindingType) -> vk::DescriptorType {
    match ty {
        wgt::BindingType::Buffer {
            ty,
            has_dynamic_offset,
            ..
        } => match ty {
            wgt::BufferBindingType::Storage { .. } => match has_dynamic_offset {
                true => vk::DescriptorType::STORAGE_BUFFER_DYNAMIC,
                false => vk::DescriptorType::STORAGE_BUFFER,
            },
            wgt::BufferBindingType::Uniform => match has_dynamic_offset {
                true => vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC,
                false => vk::DescriptorType::UNIFORM_BUFFER,
            },
        },
        wgt::BindingType::Sampler { .. } => vk::DescriptorType::SAMPLER,
        wgt::BindingType::Texture { .. } => vk::DescriptorType::SAMPLED_IMAGE,
        wgt::BindingType::StorageTexture { .. } => vk::DescriptorType::STORAGE_IMAGE,
        wgt::BindingType::AccelerationStructure { .. } => {
            vk::DescriptorType::ACCELERATION_STRUCTURE_KHR
        }
        wgt::BindingType::ExternalTexture => unreachable!("map_binding_type for ExternalTexture"),
    }
}

pub fn map_topology(topology: wgt::PrimitiveTopology) -> vk::PrimitiveTopology {
    use wgt::PrimitiveTopology as Pt;
    match topology {
        Pt::PointList => vk::PrimitiveTopology::POINT_LIST,
        Pt::LineList => vk::PrimitiveTopology::LINE_LIST,
        Pt::LineStrip => vk::PrimitiveTopology::LINE_STRIP,
        Pt::TriangleList => vk::PrimitiveTopology::TRIANGLE_LIST,
        Pt::TriangleStrip => vk::PrimitiveTopology::TRIANGLE_STRIP,
    }
}

pub fn map_polygon_mode(mode: wgt::PolygonMode) -> vk::PolygonMode {
    match mode {
        wgt::PolygonMode::Fill => vk::PolygonMode::FILL,
        wgt::PolygonMode::Line => vk::PolygonMode::LINE,
        wgt::PolygonMode::Point => vk::PolygonMode::POINT,
    }
}

pub fn map_front_face(front_face: wgt::FrontFace) -> vk::FrontFace {
    match front_face {
        wgt::FrontFace::Cw => vk::FrontFace::CLOCKWISE,
        wgt::FrontFace::Ccw => vk::FrontFace::COUNTER_CLOCKWISE,
    }
}

pub fn map_cull_face(face: wgt::Face) -> vk::CullModeFlags {
    match face {
        wgt::Face::Front => vk::CullModeFlags::FRONT,
        wgt::Face::Back => vk::CullModeFlags::BACK,
    }
}

pub fn map_stencil_op(op: wgt::StencilOperation) -> vk::StencilOp {
    use wgt::StencilOperation as So;
    match op {
        So::Keep => vk::StencilOp::KEEP,
        So::Zero => vk::StencilOp::ZERO,
        So::Replace => vk::StencilOp::REPLACE,
        So::Invert => vk::StencilOp::INVERT,
        So::IncrementClamp => vk::StencilOp::INCREMENT_AND_CLAMP,
        So::IncrementWrap => vk::StencilOp::INCREMENT_AND_WRAP,
        So::DecrementClamp => vk::StencilOp::DECREMENT_AND_CLAMP,
        So::DecrementWrap => vk::StencilOp::DECREMENT_AND_WRAP,
    }
}

pub fn map_stencil_face(
    face: &wgt::StencilFaceState,
    compare_mask: u32,
    write_mask: u32,
) -> vk::StencilOpState {
    vk::StencilOpState {
        fail_op: map_stencil_op(face.fail_op),
        pass_op: map_stencil_op(face.pass_op),
        depth_fail_op: map_stencil_op(face.depth_fail_op),
        compare_op: map_comparison(face.compare),
        compare_mask,
        write_mask,
        reference: 0,
    }
}

fn map_blend_factor(factor: wgt::BlendFactor) -> vk::BlendFactor {
    use wgt::BlendFactor as Bf;
    match factor {
        Bf::Zero => vk::BlendFactor::ZERO,
        Bf::One => vk::BlendFactor::ONE,
        Bf::Src => vk::BlendFactor::SRC_COLOR,
        Bf::OneMinusSrc => vk::BlendFactor::ONE_MINUS_SRC_COLOR,
        Bf::SrcAlpha => vk::BlendFactor::SRC_ALPHA,
        Bf::OneMinusSrcAlpha => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        Bf::Dst => vk::BlendFactor::DST_COLOR,
        Bf::OneMinusDst => vk::BlendFactor::ONE_MINUS_DST_COLOR,
        Bf::DstAlpha => vk::BlendFactor::DST_ALPHA,
        Bf::OneMinusDstAlpha => vk::BlendFactor::ONE_MINUS_DST_ALPHA,
        Bf::SrcAlphaSaturated => vk::BlendFactor::SRC_ALPHA_SATURATE,
        Bf::Constant => vk::BlendFactor::CONSTANT_COLOR,
        Bf::OneMinusConstant => vk::BlendFactor::ONE_MINUS_CONSTANT_COLOR,
        Bf::Src1 => vk::BlendFactor::SRC1_COLOR,
        Bf::OneMinusSrc1 => vk::BlendFactor::ONE_MINUS_SRC1_COLOR,
        Bf::Src1Alpha => vk::BlendFactor::SRC1_ALPHA,
        Bf::OneMinusSrc1Alpha => vk::BlendFactor::ONE_MINUS_SRC1_ALPHA,
    }
}

fn map_blend_op(operation: wgt::BlendOperation) -> vk::BlendOp {
    use wgt::BlendOperation as Bo;
    match operation {
        Bo::Add => vk::BlendOp::ADD,
        Bo::Subtract => vk::BlendOp::SUBTRACT,
        Bo::ReverseSubtract => vk::BlendOp::REVERSE_SUBTRACT,
        Bo::Min => vk::BlendOp::MIN,
        Bo::Max => vk::BlendOp::MAX,
    }
}

/// The blend state of the colour target `target`.
fn map_color_target_blend(target: &wgt::ColorTargetState) -> vk::PipelineColorBlendAttachmentState {
    let mut state = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(vk::ColorComponentFlags::from_raw(target.write_mask.bits()));
    if let Some(ref blend) = target.blend {
        let (color_op, color_src, color_dst) = map_blend_component(&blend.color);
        let (alpha_op, alpha_src, alpha_dst) = map_blend_component(&blend.alpha);
        state = state
            .blend_enable(true)
            .color_blend_op(color_op)
            .src_color_blend_factor(color_src)
            .dst_color_blend_factor(color_dst)
            .alpha_blend_op(alpha_op)
            .src_alpha_blend_factor(alpha_src)
            .dst_alpha_blend_factor(alpha_dst);
    }
    state
}

/// The blend states of a pipeline's colour attachments, one per element of `targets`.
///
/// A hole among the targets is an unused attachment, whose writes are discarded, so its state is
/// never read; it takes the first target's. Without `independentBlend`, every element of
/// `pAttachments` must be identical, unused ones included
/// (VUID-VkPipelineColorBlendStateCreateInfo-pAttachments-00605), and wgpu-core compares only the
/// targets.
pub fn map_color_targets_blend(
    targets: &[Option<wgt::ColorTargetState>],
) -> Vec<vk::PipelineColorBlendAttachmentState> {
    let hole = targets
        .iter()
        .flatten()
        .next()
        .map(map_color_target_blend)
        .unwrap_or_default();
    targets
        .iter()
        .map(|target| target.as_ref().map_or(hole, map_color_target_blend))
        .collect()
}

pub fn map_blend_component(
    component: &wgt::BlendComponent,
) -> (vk::BlendOp, vk::BlendFactor, vk::BlendFactor) {
    let op = map_blend_op(component.operation);
    let src = map_blend_factor(component.src_factor);
    let dst = map_blend_factor(component.dst_factor);
    (op, src, dst)
}

pub fn map_pipeline_statistics(
    types: wgt::PipelineStatisticsTypes,
) -> vk::QueryPipelineStatisticFlags {
    use wgt::PipelineStatisticsTypes as Pst;
    let mut flags = vk::QueryPipelineStatisticFlags::empty();
    if types.contains(Pst::VERTEX_SHADER_INVOCATIONS) {
        flags |= vk::QueryPipelineStatisticFlags::VERTEX_SHADER_INVOCATIONS;
    }
    if types.contains(Pst::CLIPPER_INVOCATIONS) {
        flags |= vk::QueryPipelineStatisticFlags::CLIPPING_INVOCATIONS;
    }
    if types.contains(Pst::CLIPPER_PRIMITIVES_OUT) {
        flags |= vk::QueryPipelineStatisticFlags::CLIPPING_PRIMITIVES;
    }
    if types.contains(Pst::FRAGMENT_SHADER_INVOCATIONS) {
        flags |= vk::QueryPipelineStatisticFlags::FRAGMENT_SHADER_INVOCATIONS;
    }
    if types.contains(Pst::COMPUTE_SHADER_INVOCATIONS) {
        flags |= vk::QueryPipelineStatisticFlags::COMPUTE_SHADER_INVOCATIONS;
    }
    flags
}

pub fn map_acceleration_structure_format(
    format: crate::AccelerationStructureFormat,
) -> vk::AccelerationStructureTypeKHR {
    match format {
        crate::AccelerationStructureFormat::TopLevel => vk::AccelerationStructureTypeKHR::TOP_LEVEL,
        crate::AccelerationStructureFormat::BottomLevel => {
            vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL
        }
    }
}

pub fn map_acceleration_structure_build_mode(
    format: crate::AccelerationStructureBuildMode,
) -> vk::BuildAccelerationStructureModeKHR {
    match format {
        crate::AccelerationStructureBuildMode::Build => {
            vk::BuildAccelerationStructureModeKHR::BUILD
        }
        crate::AccelerationStructureBuildMode::Update => {
            vk::BuildAccelerationStructureModeKHR::UPDATE
        }
    }
}

pub fn map_acceleration_structure_flags(
    flags: crate::AccelerationStructureBuildFlags,
) -> vk::BuildAccelerationStructureFlagsKHR {
    let mut vk_flags = vk::BuildAccelerationStructureFlagsKHR::empty();

    if flags.contains(crate::AccelerationStructureBuildFlags::PREFER_FAST_TRACE) {
        vk_flags |= vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE;
    }

    if flags.contains(crate::AccelerationStructureBuildFlags::PREFER_FAST_BUILD) {
        vk_flags |= vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_BUILD;
    }

    if flags.contains(crate::AccelerationStructureBuildFlags::ALLOW_UPDATE) {
        vk_flags |= vk::BuildAccelerationStructureFlagsKHR::ALLOW_UPDATE;
    }

    if flags.contains(crate::AccelerationStructureBuildFlags::LOW_MEMORY) {
        vk_flags |= vk::BuildAccelerationStructureFlagsKHR::LOW_MEMORY;
    }

    if flags.contains(crate::AccelerationStructureBuildFlags::ALLOW_COMPACTION) {
        vk_flags |= vk::BuildAccelerationStructureFlagsKHR::ALLOW_COMPACTION
    }

    if flags.contains(crate::AccelerationStructureBuildFlags::ALLOW_RAY_HIT_VERTEX_RETURN) {
        vk_flags |= vk::BuildAccelerationStructureFlagsKHR::ALLOW_DATA_ACCESS
    }

    vk_flags
}

pub fn map_acceleration_structure_geometry_flags(
    flags: crate::AccelerationStructureGeometryFlags,
) -> vk::GeometryFlagsKHR {
    let mut vk_flags = vk::GeometryFlagsKHR::empty();

    if flags.contains(crate::AccelerationStructureGeometryFlags::OPAQUE) {
        vk_flags |= vk::GeometryFlagsKHR::OPAQUE;
    }

    if flags.contains(crate::AccelerationStructureGeometryFlags::NO_DUPLICATE_ANY_HIT_INVOCATION) {
        vk_flags |= vk::GeometryFlagsKHR::NO_DUPLICATE_ANY_HIT_INVOCATION;
    }

    vk_flags
}

pub fn map_acceleration_structure_usage_to_barrier(
    usage: crate::AccelerationStructureUses,
    features: wgt::Features,
    queue_flags: vk::QueueFlags,
) -> (vk::PipelineStageFlags, vk::AccessFlags) {
    let mut stages = vk::PipelineStageFlags::empty();
    let mut access = vk::AccessFlags::empty();

    if usage.contains(crate::AccelerationStructureUses::BUILD_INPUT) {
        stages |= vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR;
    }
    if usage.contains(crate::AccelerationStructureUses::QUERY_INPUT) {
        stages |= vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR;
    }
    if usage.contains(crate::AccelerationStructureUses::BUILD_OUTPUT) {
        stages |= vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR;
    }
    if usage.contains(crate::AccelerationStructureUses::SHADER_INPUT)
        && features.contains(wgt::Features::EXPERIMENTAL_RAY_QUERY)
    {
        stages |= shader_stages(queue_flags, features);
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR;
    }
    if usage.contains(crate::AccelerationStructureUses::SHADER_INPUT)
        && features.contains(wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES)
    {
        stages |= vk::PipelineStageFlags::RAY_TRACING_SHADER_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR;
    }
    if usage.contains(crate::AccelerationStructureUses::COPY_SRC) {
        stages |= vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR;
    }
    if usage.contains(crate::AccelerationStructureUses::COPY_DST) {
        stages |= vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
        access |= vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR;
    }

    (stages, access)
}

pub fn map_component_swizzle(swizzle: wgt::ComponentSwizzle) -> vk::ComponentSwizzle {
    match swizzle {
        wgt::ComponentSwizzle::Zero => vk::ComponentSwizzle::ZERO,
        wgt::ComponentSwizzle::One => vk::ComponentSwizzle::ONE,
        wgt::ComponentSwizzle::R => vk::ComponentSwizzle::R,
        wgt::ComponentSwizzle::G => vk::ComponentSwizzle::G,
        wgt::ComponentSwizzle::B => vk::ComponentSwizzle::B,
        wgt::ComponentSwizzle::A => vk::ComponentSwizzle::A,
    }
}

pub fn map_texture_component_swizzle(
    swizzle: wgt::TextureComponentSwizzle,
) -> vk::ComponentMapping {
    vk::ComponentMapping {
        r: map_component_swizzle(swizzle.r),
        g: map_component_swizzle(swizzle.g),
        b: map_component_swizzle(swizzle.b),
        a: map_component_swizzle(swizzle.a),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hole among the colour targets takes the first target's blend state, so that a pipeline
    /// whose targets wgpu-core found to agree has identical attachment states, as a device without
    /// `independentBlend` requires; with no target, the default.
    #[test]
    fn a_colour_target_hole_takes_the_first_targets_blend_state() {
        let target = Some(wgt::ColorTargetState {
            format: wgt::TextureFormat::Rgba8Unorm,
            blend: Some(wgt::BlendState::ALPHA_BLENDING),
            write_mask: wgt::ColorWrites::RED | wgt::ColorWrites::ALPHA,
        });
        let field = |state: &vk::PipelineColorBlendAttachmentState| {
            (
                state.blend_enable,
                state.src_color_blend_factor,
                state.dst_color_blend_factor,
                state.color_blend_op,
                state.src_alpha_blend_factor,
                state.dst_alpha_blend_factor,
                state.alpha_blend_op,
                state.color_write_mask,
            )
        };
        let expected = field(&map_color_target_blend(target.as_ref().unwrap()));
        assert_eq!(expected.0, vk::TRUE);
        for targets in [
            alloc::vec![target.clone(), None],
            alloc::vec![None, target.clone()],
            alloc::vec![None, target.clone(), None, target.clone()],
        ] {
            let states = map_color_targets_blend(&targets);
            assert_eq!(states.len(), targets.len());
            assert!(
                states.iter().all(|state| field(state) == expected),
                "{targets:?}"
            );
        }
        let none = map_color_targets_blend(&[None, None]);
        assert!(none
            .iter()
            .all(|state| field(state) == field(&vk::PipelineColorBlendAttachmentState::default())));
    }

    /// `map_vk_color_space` and `map_surface_color_space` must stay mutually
    /// inverse so that a color space reported in the surface capabilities is
    /// exactly what the swapchain is created with.
    #[test]
    fn color_space_round_trip() {
        for vk_color_space in [
            vk::ColorSpaceKHR::SRGB_NONLINEAR,
            vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT,
            vk::ColorSpaceKHR::EXTENDED_SRGB_NONLINEAR_EXT,
            vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT,
            vk::ColorSpaceKHR::HDR10_ST2084_EXT,
            vk::ColorSpaceKHR::HDR10_HLG_EXT,
        ] {
            let mapped = map_vk_color_space(vk_color_space).unwrap();
            assert_eq!(map_surface_color_space(mapped), vk_color_space);
        }
    }

    #[test]
    fn unknown_color_spaces_are_dropped() {
        for vk_color_space in [
            vk::ColorSpaceKHR::BT2020_LINEAR_EXT,
            vk::ColorSpaceKHR::DOLBYVISION_EXT,
            vk::ColorSpaceKHR::PASS_THROUGH_EXT,
            vk::ColorSpaceKHR::ADOBERGB_NONLINEAR_EXT,
        ] {
            assert!(map_vk_color_space(vk_color_space).is_none());
            assert!(map_vk_surface_formats(vk::SurfaceFormatKHR {
                format: vk::Format::B8G8R8A8_UNORM,
                color_space: vk_color_space,
            })
            .is_none());
        }
    }

    #[test]
    fn hdr10_surface_format_maps() {
        let (format, color_space) = map_vk_surface_formats(vk::SurfaceFormatKHR {
            format: vk::Format::A2B10G10R10_UNORM_PACK32,
            color_space: vk::ColorSpaceKHR::HDR10_ST2084_EXT,
        })
        .unwrap();
        assert_eq!(format, wgt::TextureFormat::Rgb10a2Unorm);
        assert_eq!(color_space, wgt::SurfaceColorSpace::Bt2100Pq);
    }

    #[test]
    fn buffer_shader_stages_follow_queue_flags() {
        let usage = wgt::BufferUses::UNIFORM;

        let (stages, access) =
            map_buffer_usage_to_barrier(usage, vk::QueueFlags::GRAPHICS, wgt::Features::empty());
        assert_eq!(
            stages,
            vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER
        );
        assert_eq!(access, vk::AccessFlags::UNIFORM_READ);

        let (stages, access) =
            map_buffer_usage_to_barrier(usage, vk::QueueFlags::COMPUTE, wgt::Features::empty());
        assert_eq!(stages, vk::PipelineStageFlags::COMPUTE_SHADER);
        assert_eq!(access, vk::AccessFlags::UNIFORM_READ);

        let (stages, access) = map_buffer_usage_to_barrier(
            usage,
            vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
            wgt::Features::empty(),
        );
        assert_eq!(
            stages,
            vk::PipelineStageFlags::VERTEX_SHADER
                | vk::PipelineStageFlags::FRAGMENT_SHADER
                | vk::PipelineStageFlags::COMPUTE_SHADER
        );
        assert_eq!(access, vk::AccessFlags::UNIFORM_READ);
    }

    #[test]
    fn non_shader_buffer_usage_is_unchanged() {
        let (stages, access) = map_buffer_usage_to_barrier(
            wgt::BufferUses::COPY_SRC | wgt::BufferUses::VERTEX,
            vk::QueueFlags::empty(),
            wgt::Features::empty(),
        );

        assert_eq!(
            stages,
            vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::VERTEX_INPUT
        );
        assert_eq!(
            access,
            vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::VERTEX_ATTRIBUTE_READ
        );
    }

    #[test]
    fn texture_shader_stages_follow_queue_flags() {
        let usage = wgt::TextureUses::RESOURCE;

        let (stages, access) = map_texture_usage_to_barrier(
            usage,
            vk::QueueFlags::GRAPHICS,
            false,
            wgt::Features::empty(),
        );
        assert_eq!(
            stages,
            vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER
        );
        assert_eq!(access, vk::AccessFlags::SHADER_READ);

        let (stages, access) = map_texture_usage_to_barrier(
            usage,
            vk::QueueFlags::COMPUTE,
            false,
            wgt::Features::empty(),
        );
        assert_eq!(stages, vk::PipelineStageFlags::COMPUTE_SHADER);
        assert_eq!(access, vk::AccessFlags::SHADER_READ);

        let (stages, access) = map_texture_usage_to_barrier(
            usage,
            vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
            false,
            wgt::Features::empty(),
        );
        assert_eq!(
            stages,
            vk::PipelineStageFlags::VERTEX_SHADER
                | vk::PipelineStageFlags::FRAGMENT_SHADER
                | vk::PipelineStageFlags::COMPUTE_SHADER
        );
        assert_eq!(access, vk::AccessFlags::SHADER_READ);
    }

    #[test]
    fn non_shader_texture_usage_is_unchanged() {
        let (stages, access) = map_texture_usage_to_barrier(
            wgt::TextureUses::COPY_SRC | wgt::TextureUses::COLOR_TARGET,
            vk::QueueFlags::empty(),
            false,
            wgt::Features::empty(),
        );

        assert_eq!(
            stages,
            vk::PipelineStageFlags::TRANSFER | vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
        );
        assert_eq!(
            access,
            vk::AccessFlags::TRANSFER_READ
                | vk::AccessFlags::COLOR_ATTACHMENT_READ
                | vk::AccessFlags::COLOR_ATTACHMENT_WRITE
        );
    }

    #[test]
    fn acceleration_structure_shader_stages_follow_queue_flags() {
        let usage = crate::AccelerationStructureUses::SHADER_INPUT;
        let features = wgt::Features::EXPERIMENTAL_RAY_QUERY;

        let (stages, access) =
            map_acceleration_structure_usage_to_barrier(usage, features, vk::QueueFlags::GRAPHICS);
        assert_eq!(
            stages,
            vk::PipelineStageFlags::VERTEX_SHADER | vk::PipelineStageFlags::FRAGMENT_SHADER
        );
        assert_eq!(access, vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);

        let (stages, access) =
            map_acceleration_structure_usage_to_barrier(usage, features, vk::QueueFlags::COMPUTE);
        assert_eq!(stages, vk::PipelineStageFlags::COMPUTE_SHADER);
        assert_eq!(access, vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);

        let (stages, access) = map_acceleration_structure_usage_to_barrier(
            usage,
            features | wgt::Features::EXPERIMENTAL_RAY_TRACING_PIPELINES,
            vk::QueueFlags::COMPUTE,
        );
        assert_eq!(
            stages,
            vk::PipelineStageFlags::COMPUTE_SHADER | vk::PipelineStageFlags::RAY_TRACING_SHADER_KHR
        );
        assert_eq!(access, vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);

        let (stages, access) = map_acceleration_structure_usage_to_barrier(
            usage,
            features,
            vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE,
        );
        assert_eq!(
            stages,
            vk::PipelineStageFlags::VERTEX_SHADER
                | vk::PipelineStageFlags::FRAGMENT_SHADER
                | vk::PipelineStageFlags::COMPUTE_SHADER
        );
        assert_eq!(access, vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);
    }

    #[test]
    fn non_shader_acceleration_structure_usage_is_unchanged() {
        let (stages, access) = map_acceleration_structure_usage_to_barrier(
            crate::AccelerationStructureUses::BUILD_INPUT
                | crate::AccelerationStructureUses::BUILD_OUTPUT,
            wgt::Features::empty(),
            vk::QueueFlags::empty(),
        );

        assert_eq!(
            stages,
            vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR
        );
        assert_eq!(
            access,
            vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR
                | vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR
        );
    }

    /// Each scaling mode asks for its present scaling, with the gravity that places what does not
    /// fill the surface: centered for an aspect-ratio stretch, the top-left corner for one-to-one.
    /// A stretch fills the surface and asks for no gravity, which the spec allows only on both axes
    /// together (VUID-VkSwapchainPresentScalingCreateInfoKHR-presentGravityX-07765, -07766). `Auto`
    /// asks for nothing.
    #[test]
    fn each_scaling_mode_asks_its_scaling_and_gravity() {
        use wgt::SurfaceScalingMode as Mode;
        assert_eq!(map_surface_scaling_mode(Mode::Auto), None);
        assert_eq!(
            map_surface_scaling_mode(Mode::Stretch),
            Some((
                vk::PresentScalingFlagsEXT::STRETCH,
                vk::PresentGravityFlagsEXT::empty()
            ))
        );
        assert_eq!(
            map_surface_scaling_mode(Mode::AspectRatio),
            Some((
                vk::PresentScalingFlagsEXT::ASPECT_RATIO_STRETCH,
                vk::PresentGravityFlagsEXT::CENTERED
            ))
        );
        assert_eq!(
            map_surface_scaling_mode(Mode::OneToOne),
            Some((
                vk::PresentScalingFlagsEXT::ONE_TO_ONE,
                vk::PresentGravityFlagsEXT::MIN
            ))
        );
    }

    /// A surface lists the scaling modes that it supports with every present mode, each with the
    /// gravity it needs on both axes, so that a configuration may combine any listed scaling mode
    /// with any listed present mode; none for no present mode.
    #[test]
    fn a_surface_lists_the_scaling_modes_of_every_present_mode_with_their_gravity() {
        use wgt::SurfaceScalingMode as Mode;
        let all = PresentScalingSupport {
            scaling: vk::PresentScalingFlagsEXT::STRETCH
                | vk::PresentScalingFlagsEXT::ASPECT_RATIO_STRETCH
                | vk::PresentScalingFlagsEXT::ONE_TO_ONE,
            gravity_x: vk::PresentGravityFlagsEXT::MIN | vk::PresentGravityFlagsEXT::CENTERED,
            gravity_y: vk::PresentGravityFlagsEXT::MIN | vk::PresentGravityFlagsEXT::CENTERED,
        };
        assert_eq!(surface_scaling_modes(&[]), []);
        assert_eq!(
            surface_scaling_modes(&[all]),
            [Mode::Stretch, Mode::AspectRatio, Mode::OneToOne]
        );
        // Another present mode without one-to-one scaling.
        let no_one_to_one = PresentScalingSupport {
            scaling: all.scaling & !vk::PresentScalingFlagsEXT::ONE_TO_ONE,
            ..all
        };
        assert_eq!(
            surface_scaling_modes(&[all, no_one_to_one]),
            [Mode::Stretch, Mode::AspectRatio]
        );
        assert_eq!(
            surface_scaling_modes(&[no_one_to_one, all]),
            [Mode::Stretch, Mode::AspectRatio]
        );
        // One-to-one without the top-left gravity on one axis, the aspect-ratio stretch without
        // centering on the other: neither is listed.
        let partial_gravity = PresentScalingSupport {
            gravity_x: vk::PresentGravityFlagsEXT::CENTERED,
            gravity_y: vk::PresentGravityFlagsEXT::MIN,
            ..all
        };
        assert_eq!(surface_scaling_modes(&[partial_gravity]), [Mode::Stretch]);
        // A stretch needs no gravity.
        let stretch_alone = PresentScalingSupport {
            scaling: vk::PresentScalingFlagsEXT::STRETCH,
            ..PresentScalingSupport::default()
        };
        assert_eq!(surface_scaling_modes(&[stretch_alone]), [Mode::Stretch]);
        // Gravity without a scaling lists nothing.
        let gravity_alone = PresentScalingSupport {
            scaling: vk::PresentScalingFlagsEXT::empty(),
            ..all
        };
        assert_eq!(surface_scaling_modes(&[gravity_alone]), []);
    }

    fn scaled_range(
        min: (u32, u32),
        max: (u32, u32),
    ) -> vk::SurfacePresentScalingCapabilitiesEXT<'static> {
        vk::SurfacePresentScalingCapabilitiesEXT {
            min_scaled_image_extent: vk::Extent2D {
                width: min.0,
                height: min.1,
            },
            max_scaled_image_extent: vk::Extent2D {
                width: max.0,
                height: max.1,
            },
            ..Default::default()
        }
    }

    fn extent(width: u32, height: u32) -> wgt::Extent3d {
        wgt::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        }
    }

    /// The range for scaled presentation bounds the extent on both ends, inclusive, on each axis
    /// alone.
    #[test]
    fn a_scaled_extent_must_be_within_the_range_on_both_axes() {
        let caps = scaled_range((100, 50), (800, 600));
        assert!(extent_within_scaled_range(&caps, extent(100, 50)));
        assert!(extent_within_scaled_range(&caps, extent(400, 300)));
        assert!(extent_within_scaled_range(&caps, extent(800, 600)));
        assert!(!extent_within_scaled_range(&caps, extent(99, 300)));
        assert!(!extent_within_scaled_range(&caps, extent(400, 49)));
        assert!(!extent_within_scaled_range(&caps, extent(801, 300)));
        assert!(!extent_within_scaled_range(&caps, extent(400, 601)));
    }

    /// `(0xFFFFFFFF, 0xFFFFFFFF)` at either end leaves that end unbounded, and only that end; a
    /// single `0xFFFFFFFF` is an ordinary bound.
    #[test]
    fn the_special_extent_leaves_its_end_unbounded() {
        let special = (u32::MAX, u32::MAX);
        let both = scaled_range(special, special);
        assert!(extent_within_scaled_range(&both, extent(1, 1)));
        assert!(extent_within_scaled_range(
            &both,
            extent(u32::MAX - 1, u32::MAX - 1)
        ));
        let min = scaled_range(special, (800, 600));
        assert!(extent_within_scaled_range(&min, extent(1, 1)));
        assert!(!extent_within_scaled_range(&min, extent(801, 600)));
        let max = scaled_range((100, 100), special);
        assert!(extent_within_scaled_range(&max, extent(u32::MAX - 1, 100)));
        assert!(!extent_within_scaled_range(&max, extent(99, 100)));
        let one_axis = scaled_range((u32::MAX, 1), (u32::MAX, u32::MAX));
        assert!(!extent_within_scaled_range(&one_axis, extent(100, 100)));
    }
}
