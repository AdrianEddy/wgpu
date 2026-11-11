//! A failed submission releases its locks in the reverse order of their acquisition, before it
//! drops the work it did not submit, and before it handles its error.
//!
//! Each test fails a submission on the noop backend at a different point, with lock validation in
//! wgpu's debug builds: dropping the unsubmitted work locks what may not be locked under the
//! submission's locks, and so does the device lost callback, which uses the queue.

use std::sync::{atomic::Ordering::Relaxed, Arc};

use wgpu_hal::{api::Noop, noop::DeviceOperation, Device as _};

use super::user_callbacks::{fail_next, noop, on_device_lost};

/// A submission that the backend fails.
#[test]
fn device_lost_callback_of_a_failed_submission() {
    let (_instance, device, queue) = noop();
    let lost = on_device_lost(&device, {
        let queue = queue.clone();
        move || queue.on_submitted_work_done(|| ())
    });

    fail_next(
        &device,
        DeviceOperation::Submit,
        wgpu_hal::DeviceError::Lost,
    );
    let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    queue.submit([encoder.finish()]);

    assert!(lost.load(Relaxed));
}

/// A submission that fails once it has started to update wgpu's trackers, here because it must
/// initialize a texture that wgpu cannot clear.
#[test]
fn device_lost_callback_of_a_submission_that_cannot_initialize_a_texture() {
    let (_instance, device, queue) = noop();
    device.on_uncaptured_error(Arc::new(|_| {}));
    let lost = on_device_lost(&device, {
        let queue = queue.clone();
        move || queue.on_submitted_work_done(|| ())
    });

    let size = wgpu::Extent3d {
        width: 4,
        height: 4,
        depth_or_array_layers: 1,
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    // SAFETY: The texture is used as the descriptor below describes it.
    let texture = unsafe {
        let hal_texture = device
            .as_hal::<Noop>()
            .unwrap()
            .create_texture(&wgpu_hal::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUses::COPY_SRC,
                memory_flags: wgpu_hal::MemoryFlags::empty(),
                view_formats: Vec::new(),
                texture_binding_view_dimension: None,
            })
            .unwrap();
        // Imported uninitialized: wgpu must clear it before it is read, but it has no way to
        // clear a texture it did not create.
        device.create_texture_from_hal::<Noop>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
                texture_binding_view_dimension: None,
            },
            wgpu::TextureUses::UNINITIALIZED,
            false,
        )
    };
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4 * 256,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: None,
            },
        },
        size,
    );
    queue.submit([encoder.finish()]);

    assert!(lost.load(Relaxed));
}

/// A submission whose pending writes cannot be ended.
#[test]
fn device_lost_callback_of_a_submission_whose_pending_writes_fail() {
    let (_instance, device, queue) = noop();
    let lost = on_device_lost(&device, {
        let queue = queue.clone();
        move || queue.on_submitted_work_done(|| ())
    });

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[0; 4]);
    fail_next(
        &device,
        DeviceOperation::EndEncoding,
        wgpu_hal::DeviceError::Lost,
    );
    queue.submit([]);

    assert!(lost.load(Relaxed));
}

/// A submission whose transition pass, which brings a command buffer's resources into the states
/// it needs, fails to end: here, the transition of a buffer that a command buffer clears.
#[test]
fn device_lost_callback_of_a_submission_whose_transition_pass_fails() {
    let (_instance, device, queue) = noop();
    let lost = on_device_lost(&device, {
        let queue = queue.clone();
        move || queue.on_submitted_work_done(|| ())
    });
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.clear_buffer(&buffer, 0, None);
    let command_buffer = encoder.finish();
    fail_next(
        &device,
        DeviceOperation::EndEncoding,
        wgpu_hal::DeviceError::Lost,
    );
    queue.submit([command_buffer]);

    assert!(lost.load(Relaxed));
}
