//! wgpu calls the application's callbacks only once it holds no lock, so they can call wgpu.
//!
//! Most tests make a noop device's operation fail where wgpu holds locks, so that wgpu loses the
//! device there, and the device lost callback calls into wgpu. With lock validation, as in wgpu's
//! debug builds, a callback called under a lock makes the lock checker panic. Without it, the
//! callback deadlocks.

use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        mpsc, Arc,
    },
};

use wgpu_hal::{api::Noop, noop::DeviceOperation};

pub(super) fn noop() -> (wgpu::Instance, wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions::enabled(),
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .unwrap();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    (instance, device, queue)
}

/// Makes the next `operation` of the noop `device` fail with `error`.
pub(super) fn fail_next(
    device: &wgpu::Device,
    operation: DeviceOperation,
    error: wgpu_hal::DeviceError,
) {
    // SAFETY: The noop device is only told to fail an operation.
    unsafe { device.as_hal::<Noop>() }
        .unwrap()
        .fail_next(operation, error);
}

/// Sets a device lost callback that runs `f`. Returns whether the callback has run.
pub(super) fn on_device_lost(
    device: &wgpu::Device,
    f: impl Fn() + Send + 'static,
) -> Arc<AtomicBool> {
    let lost = Arc::new(AtomicBool::new(false));
    device.set_device_lost_callback({
        let lost = Arc::clone(&lost);
        move |_reason, _message| {
            f();
            lost.store(true, Relaxed);
        }
    });
    lost
}

fn map_read_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// A failed wait loses the device in `Device::poll`, which holds the snatch lock.
#[test]
fn device_lost_callback_of_a_failed_wait() {
    let (_instance, device, queue) = noop();
    let lost = on_device_lost(&device, {
        let queue = queue.clone();
        move || {
            queue.submit([]);
        }
    });

    queue.submit([]);
    fail_next(&device, DeviceOperation::Wait, wgpu_hal::DeviceError::Lost);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    assert!(lost.load(Relaxed));
}

/// A failed mapping loses the device in `Device::poll`, which holds the buffer's map state, the
/// queue's life tracker and the snatch lock (gfx-rs/wgpu#10031).
#[test]
fn device_lost_callback_of_a_failed_buffer_mapping() {
    let (_instance, device, _queue) = noop();
    let buffer = map_read_buffer(&device);
    let lost = on_device_lost(&device, {
        let buffer = buffer.clone();
        move || buffer.unmap()
    });

    fail_next(
        &device,
        DeviceOperation::MapBuffer,
        wgpu_hal::DeviceError::Lost,
    );
    buffer.map_async(wgpu::MapMode::Read, .., |_| ());
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    assert!(lost.load(Relaxed));
}

/// A failed memory check loses the device in `Instance::poll_all`, which locks the instance's
/// list of devices.
#[test]
fn device_lost_callback_of_a_failed_memory_check() {
    let (instance, device, _queue) = noop();
    let lost = on_device_lost(&device, {
        let instance = instance.clone();
        move || {
            let _ = instance.poll_all(false);
        }
    });

    fail_next(
        &device,
        DeviceOperation::CheckIfOom,
        wgpu_hal::DeviceError::OutOfMemory,
    );
    let _ = instance.poll_all(false);

    assert!(lost.load(Relaxed));
}

/// A callback that panics leaves the callbacks after it to the next delivery.
#[test]
fn a_panicking_callback_leaves_the_others_to_the_next_delivery() {
    let (_instance, device, _queue) = noop();
    let buffers = [map_read_buffer(&device), map_read_buffer(&device)];
    let called = Arc::new(AtomicBool::new(false));
    buffers[0].map_async(wgpu::MapMode::Read, .., |_| {
        panic!("a mapping callback panics");
    });
    buffers[1].map_async(wgpu::MapMode::Read, .., {
        let called = Arc::clone(&called);
        move |_| called.store(true, Relaxed)
    });

    let polled = catch_unwind(AssertUnwindSafe(|| device.poll(wgpu::PollType::Poll)));
    assert!(polled.is_err());
    assert!(!called.load(Relaxed));

    device.poll(wgpu::PollType::Poll).unwrap();
    assert!(called.load(Relaxed));
}

/// The callbacks of a submission run on one thread, in order: while one thread runs its mapping
/// callback, another thread's poll does not run its work-done callback, which must come after.
#[test]
fn a_submission_s_callbacks_stay_in_order_across_threads() {
    let (_instance, device, queue) = noop();
    let buffer = map_read_buffer(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.clear_buffer(&buffer, 0, None);
    queue.submit([encoder.finish()]);

    let (entered, wait_for_entered) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel::<()>();
    buffer.map_async(wgpu::MapMode::Read, .., move |_| {
        entered.send(()).unwrap();
        wait_for_release.recv().unwrap();
    });
    let done = Arc::new(AtomicBool::new(false));
    queue.on_submitted_work_done({
        let done = Arc::clone(&done);
        move || done.store(true, Relaxed)
    });

    let poller = std::thread::spawn({
        let device = device.clone();
        move || {
            device.poll(wgpu::PollType::Poll).unwrap();
        }
    });
    wait_for_entered.recv().unwrap();
    device.poll(wgpu::PollType::Poll).unwrap();
    assert!(!done.load(Relaxed));

    release.send(()).unwrap();
    poller.join().unwrap();
    assert!(done.load(Relaxed));
}

/// Dropping the queue delivers the callbacks that its drop completes.
#[test]
fn dropping_the_queue_delivers_its_callbacks() {
    let (_instance, device, queue) = noop();
    let buffer = map_read_buffer(&device);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.clear_buffer(&buffer, 0, None);
    queue.submit([encoder.finish()]);
    let done = Arc::new(AtomicBool::new(false));
    queue.on_submitted_work_done({
        let done = Arc::clone(&done);
        move || done.store(true, Relaxed)
    });

    drop(queue);

    assert!(done.load(Relaxed));
}

/// A call that delivers no callbacks otherwise, here creating a buffer mapped at creation, reports
/// the error of losing the device, and reporting an error delivers callbacks: the device lost
/// callback is called before the call returns. wgpu holds no lock then, so the callback can use
/// another buffer.
#[test]
fn device_lost_callback_of_a_call_that_reports_an_error() {
    let (_instance, device, _queue) = noop();
    let buffer = map_read_buffer(&device);
    let lost = on_device_lost(&device, {
        let buffer = buffer.clone();
        move || buffer.unmap()
    });

    fail_next(
        &device,
        DeviceOperation::MapBuffer,
        wgpu_hal::DeviceError::Lost,
    );
    let _mapped_at_creation = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::MAP_WRITE,
        mapped_at_creation: true,
    });
    assert!(lost.load(Relaxed));
}

/// `Queue::on_submitted_work_done` submits the pending writes, which can lose the device, and
/// discards the error: it delivers the device lost callback itself.
#[test]
fn device_lost_callback_of_on_submitted_work_done() {
    let (_instance, device, queue) = noop();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &[0; 4]);
    let lost = on_device_lost(&device, || ());

    fail_next(
        &device,
        DeviceOperation::Submit,
        wgpu_hal::DeviceError::Lost,
    );
    queue.on_submitted_work_done(|| ());
    assert!(lost.load(Relaxed));
}

/// A callback that polls delivers the rest of its own batch before later batches: here a
/// mapping callback that registers a second work-done callback and polls, in the batch of a first
/// work-done callback registered before.
#[test]
fn work_done_callbacks_stay_in_order_when_a_callback_polls() {
    let (_instance, device, queue) = noop();
    let buffer = map_read_buffer(&device);
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let push =
        |log: &Arc<std::sync::Mutex<Vec<&'static str>>>, entry| log.lock().unwrap().push(entry);
    buffer.map_async(wgpu::MapMode::Read, .., {
        let (log, device, queue) = (Arc::clone(&log), device.clone(), queue.clone());
        move |_| {
            push(&log, "map start");
            queue.on_submitted_work_done({
                let log = Arc::clone(&log);
                move || push(&log, "work done 2")
            });
            device.poll(wgpu::PollType::Poll).unwrap();
            push(&log, "map end");
        }
    });
    queue.on_submitted_work_done({
        let log = Arc::clone(&log);
        move || push(&log, "work done 1")
    });
    device.poll(wgpu::PollType::Poll).unwrap();

    assert_eq!(
        *log.lock().unwrap(),
        ["map start", "work done 1", "work done 2", "map end"]
    );
}

/// A callback that maps another buffer and polls until that mapping's callback has run, as a
/// blocking read-back helper does, finds it called.
#[test]
fn a_callback_s_poll_delivers_the_callback_it_waits_for() {
    let (_instance, device, _queue) = noop();
    let first = map_read_buffer(&device);
    let second = map_read_buffer(&device);
    let seen = Arc::new(std::sync::Mutex::new(None));
    first.map_async(wgpu::MapMode::Read, .., {
        let (device, seen) = (device.clone(), Arc::clone(&seen));
        move |_| {
            let mapped = Arc::new(AtomicBool::new(false));
            second.map_async(wgpu::MapMode::Read, .., {
                let mapped = Arc::clone(&mapped);
                move |_| mapped.store(true, Relaxed)
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            *seen.lock().unwrap() = Some(mapped.load(Relaxed));
        }
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(true));
}

/// The uncaptured error handler of a failed acquisition can use the surface: wgpu releases the
/// surface's lock before it reports the error.
#[test]
fn uncaptured_error_handler_of_a_failed_acquisition() {
    use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WebDisplayHandle, WebWindowHandle};

    let (instance, device, _queue) = noop();
    // SAFETY: The noop backend does not use the handles.
    let surface = unsafe {
        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(RawDisplayHandle::Web(WebDisplayHandle::new())),
            raw_window_handle: RawWindowHandle::Web(WebWindowHandle::new(1)),
        })
    }
    .unwrap();
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .unwrap();
    surface.configure(
        &device,
        &surface.get_default_config(&adapter, 4, 4).unwrap(),
    );
    let surface = Arc::new(surface);
    let reentered = Arc::new(AtomicBool::new(false));
    device.on_uncaptured_error(Arc::new({
        let surface = Arc::clone(&surface);
        let reentered = Arc::clone(&reentered);
        move |_error| {
            // Acquiring again reports the same error, once.
            if !reentered.swap(true, Relaxed) {
                let _ = surface.get_current_texture();
            }
        }
    }));

    let wgpu::CurrentSurfaceTexture::Success(texture) = surface.get_current_texture() else {
        panic!("the first acquisition fails");
    };
    // Acquiring while a texture is acquired is an error. Run it on another thread, so that a
    // deadlock fails the test instead of hanging it.
    let (done, wait_for_done) = mpsc::channel();
    std::thread::spawn({
        let surface = Arc::clone(&surface);
        move || {
            let _ = surface.get_current_texture();
            done.send(()).unwrap();
        }
    });
    wait_for_done
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the uncaptured error handler deadlocked");
    assert!(reentered.load(Relaxed));
    drop(texture);
}

/// The uncaptured error handler runs as if no call were in progress on its thread, so the wgpu
/// calls it makes deliver callbacks: here, a handler that reads a buffer back, polling until the
/// mapping is done, as a blocking read-back helper does. Called inside the scope of the call that
/// reported the error, its poll delivered nothing, and such a helper waited forever.
#[test]
fn uncaptured_error_handler_s_own_calls_deliver_callbacks() {
    let (_instance, device, _queue) = noop();
    let readback = map_read_buffer(&device);
    let mapped = Arc::new(AtomicBool::new(false));
    let mapped_in_handler = Arc::new(std::sync::Mutex::new(None));
    let handled = AtomicBool::new(false);
    device.on_uncaptured_error(Arc::new({
        let device = device.clone();
        let mapped = Arc::clone(&mapped);
        let mapped_in_handler = Arc::clone(&mapped_in_handler);
        move |_error| {
            if handled.swap(true, Relaxed) {
                return;
            }
            readback.map_async(wgpu::MapMode::Read, .., {
                let mapped = Arc::clone(&mapped);
                move |_| mapped.store(true, Relaxed)
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            *mapped_in_handler.lock().unwrap() = Some(mapped.load(Relaxed));
        }
    }));

    // Mapping a buffer without a map usage is a validation error, reported inside `map_async`.
    let unmappable = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    unmappable.map_async(wgpu::MapMode::Read, .., |_| ());
    assert_eq!(*mapped_in_handler.lock().unwrap(), Some(true));
}
