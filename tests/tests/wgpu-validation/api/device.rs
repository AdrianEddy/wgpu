/// Test that if another error occurs reentrantly in the [`wgpu::Device::on_uncaptured_error`]
/// handler, this does not result in a deadlock (as a previous implementation would have had).
#[cfg(not(target_family = "wasm"))] // test needs wgpu::Device: Send + Sync to achieve reentrance
#[test]
fn recursive_uncaptured_error() {
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
    use std::sync::Arc;

    const ERRONEOUS_TEXTURE_DESC: wgpu::TextureDescriptor = wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 10,
        },
        mip_level_count: 0,
        sample_count: 0,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    };

    let (device, _queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());

    let errors_seen: Arc<AtomicU32> = Arc::default();
    let handler = Arc::new({
        let errors_seen = errors_seen.clone();
        let device = device.clone();
        move |_error| {
            let previous_count = errors_seen.fetch_add(1, Relaxed);
            if previous_count == 0 {
                // Trigger another error recursively
                _ = device.create_texture(&ERRONEOUS_TEXTURE_DESC);
            }
        }
    });

    // Trigger one error which will call the handler
    device.on_uncaptured_error(handler);
    _ = device.create_texture(&ERRONEOUS_TEXTURE_DESC);

    assert_eq!(errors_seen.load(Relaxed), 2);
}

/// Replacing the uncaptured error handler drops the handler it replaces. If that handler held the
/// last `wgpu::Queue`, dropping it drops the queue, which calls the queue's work-done callbacks. A
/// callback that pushes an error scope locks the device's error sink, so the handler must be
/// dropped after the error sink is unlocked.
#[cfg(not(target_family = "wasm"))]
#[test]
fn replacing_the_uncaptured_error_handler_drops_the_old_one_unlocked() {
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    queue.submit([]);
    let (done, replaced) = mpsc::channel();
    thread::spawn(move || {
        queue.on_submitted_work_done({
            let device = device.clone();
            move || drop(device.push_error_scope(wgpu::ErrorFilter::Validation))
        });
        // The handler to be replaced holds the last `wgpu::Queue`.
        device.on_uncaptured_error(Arc::new(move |_error| {
            let _queue = &queue;
        }));
        device.on_uncaptured_error(Arc::new(|_error| ()));
        done.send(()).unwrap();
    });
    replaced
        .recv_timeout(Duration::from_secs(10))
        .expect("replacing the handler did not finish");
}

/// Setting a device lost callback drops the callback it replaces, and what that callback captured,
/// which here sets the device lost callback again as it drops. The replaced callback must be
/// dropped after the device unlocks its callback.
#[cfg(not(target_family = "wasm"))]
#[test]
fn replacing_the_device_lost_callback_drops_the_old_one_unlocked() {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    struct SetsTheCallbackOnDrop(wgpu::Device);
    impl Drop for SetsTheCallbackOnDrop {
        fn drop(&mut self) {
            self.0.set_device_lost_callback(|_reason, _message| ());
        }
    }

    let (device, _queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    let (done, replaced) = mpsc::channel();
    thread::spawn(move || {
        let captured = SetsTheCallbackOnDrop(device.clone());
        device.set_device_lost_callback(move |_reason, _message| {
            let _captured = &captured;
        });
        device.set_device_lost_callback(|_reason, _message| ());
        done.send(()).unwrap();
    });
    replaced
        .recv_timeout(Duration::from_secs(10))
        .expect("replacing the device lost callback did not finish");
}

/// [`wgpu::Device::destroy`] unmaps every buffer of the device. Another thread may drop a buffer
/// meanwhile, so that unmapping it drops the last reference to its map context, whose drop
/// unregisters it from the device's set of buffers. Destroying must not hold that set's lock then.
///
/// Each round creates a buffer, then destroys the device while another thread drops the buffer,
/// after a delay that varies from round to round. Destroying a destroyed device unmaps its buffers
/// again, so one device serves every round. The test fails if the rounds stop making progress.
#[cfg(not(target_family = "wasm"))]
#[test]
fn destroying_a_device_while_another_thread_drops_its_buffers() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    use std::sync::{mpsc, Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    const ROUNDS: usize = 50_000;
    /// The longest delay before the drop, in iterations of a spin loop.
    const MAX_DELAY: u32 = 2_000;

    let (device, _queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    // Creating a buffer on a destroyed device is an error.
    device.on_uncaptured_error(Arc::new(|_error| ()));
    let start = Arc::new(Barrier::new(2));
    let rounds_done = Arc::new(AtomicUsize::new(0));
    let (send_buffer, buffers) = mpsc::channel::<wgpu::Buffer>();

    let dropper = thread::spawn({
        let start = Arc::clone(&start);
        move || {
            for (round, buffer) in buffers.into_iter().enumerate() {
                start.wait();
                let delay = (round as u32).wrapping_mul(2_654_435_761) % MAX_DELAY;
                for _ in 0..delay {
                    core::hint::spin_loop();
                }
                drop(buffer);
            }
        }
    });
    thread::spawn({
        let rounds_done = Arc::clone(&rounds_done);
        move || {
            for _ in 0..ROUNDS {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 4,
                    usage: wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                send_buffer.send(buffer).unwrap();
                start.wait();
                device.destroy();
                rounds_done.fetch_add(1, Relaxed);
            }
        }
    });

    let mut last = (0, Instant::now());
    while rounds_done.load(Relaxed) < ROUNDS {
        thread::sleep(Duration::from_millis(50));
        let done = rounds_done.load(Relaxed);
        if done != last.0 {
            last = (done, Instant::now());
        }
        assert!(
            last.1.elapsed() < Duration::from_secs(10),
            "destroying the device stopped finishing after {done} rounds"
        );
    }
    dropper.join().unwrap();
}

/// A device has no downlevel flag that its adapter lacks: an adapter whose downlevel capabilities
/// a hal user narrows before giving it to wgpu validates its devices without the flags it removed,
/// though its hal device reports them all. Without `CUBE_ARRAY_TEXTURES`, a shader with a cube
/// array texture is a validation error.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_device_has_no_downlevel_flag_that_its_adapter_lacks() {
    use core::future::Future as _;
    use core::pin::pin;
    use core::task;
    use wgpu_hal::Instance as _;

    let backend_options = wgpu::BackendOptions {
        noop: wgpu::NoopBackendOptions::enabled(),
        ..Default::default()
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: backend_options.clone(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    // SAFETY: The noop backend has no requirements.
    let hal_instance = unsafe {
        wgpu_hal::noop::Context::init(&wgpu_hal::InstanceDescriptor {
            name: "narrowed",
            flags: wgpu::InstanceFlags::empty(),
            memory_budget_thresholds: Default::default(),
            backend_options,
            telemetry: None,
            display: None,
        })
    }
    .unwrap();
    // SAFETY: No requirements.
    let mut exposed = unsafe { hal_instance.enumerate_adapters(None) }
        .pop()
        .unwrap();
    let removed = wgpu::DownlevelFlags::CUBE_ARRAY_TEXTURES;
    assert!(exposed.capabilities.downlevel.flags.contains(removed));
    exposed.capabilities.downlevel.flags -= removed;
    // SAFETY: An adapter of the noop backend that this instance enables.
    let adapter = unsafe { instance.create_adapter_from_hal(exposed) };
    let ctx = &mut task::Context::from_waker(task::Waker::noop());
    let task::Poll::Ready(Ok((device, _queue))) =
        pin!(adapter.request_device(&wgpu::DeviceDescriptor::default())).poll(ctx)
    else {
        unreachable!()
    };
    assert_eq!(
        device.downlevel_capabilities().flags,
        adapter.get_downlevel_capabilities().flags
    );

    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            "@group(0) @binding(0) var cubes: texture_cube_array<f32>;".into(),
        ),
    });
    let task::Poll::Ready(error) = pin!(scope.pop()).poll(ctx) else {
        unreachable!()
    };
    assert!(
        error.is_some(),
        "a cube array texture in a shader of a device without CUBE_ARRAY_TEXTURES"
    );
    let task::Poll::Ready(info) = pin!(module.get_compilation_info()).poll(ctx) else {
        unreachable!()
    };
    assert!(
        info.messages.iter().any(|message| {
            message.message_type == wgpu::CompilationMessageType::Error
                && message.message.contains("CUBE_ARRAY_TEXTURES")
        }),
        "{:?}",
        info.messages
    );
}

/// A device made from a hal device may request only the features that the hal device was opened
/// with: wgpu-hal takes its paths from them. Requesting more is an error naming them; requesting
/// them is not. Before, wgpu-core took the descriptor's features unchecked.
#[cfg(not(target_family = "wasm"))]
#[test]
fn a_device_from_a_hal_device_requests_only_what_it_was_opened_with() {
    use core::future::Future as _;
    use core::pin::pin;
    use core::task;
    use wgpu_hal::{api::Noop, Adapter as _};

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions::enabled(),
            ..Default::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let ctx = &mut task::Context::from_waker(task::Waker::noop());
    let task::Poll::Ready(Ok(adapter)) =
        pin!(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).poll(ctx)
    else {
        unreachable!()
    };
    let opened = wgpu::Features::POLYGON_MODE_LINE;
    let open = |features| {
        // SAFETY: Used to open a device.
        let hal = unsafe { adapter.as_hal::<Noop>() }.unwrap();
        // SAFETY: No requirements on the noop backend.
        unsafe { hal.open(features, &wgpu::Limits::default(), &Default::default()) }.unwrap()
    };
    // SAFETY: Opened from this adapter, with default limits.
    let more = unsafe {
        adapter.create_device_from_hal(
            open(opened),
            &wgpu::DeviceDescriptor {
                required_features: opened | wgpu::Features::SHADER_F16,
                ..Default::default()
            },
        )
    };
    let error = more.expect_err("a feature the hal device was not opened with");
    assert!(error.to_string().contains("SHADER_F16"), "{error}");
    // SAFETY: Opened from this adapter, with default limits.
    let (device, _queue) = unsafe {
        adapter.create_device_from_hal(
            open(opened),
            &wgpu::DeviceDescriptor {
                required_features: opened,
                ..Default::default()
            },
        )
    }
    .unwrap();
    assert_eq!(device.features(), opened);
}

/// Dropping the queue waits for its submissions on their fence before it waits for the queue to
/// become idle, as `wgpu_hal::Queue::wait_for_idle` requires: the idle wait must not wait for one
/// of them. The noop queue records an idle wait with a submission incomplete; here the GPU stalls,
/// and resumes only inside a fence wait.
#[cfg(not(target_family = "wasm"))]
#[test]
fn dropping_the_queue_waits_for_its_submissions_before_waiting_for_it_to_be_idle() {
    use wgpu_hal::{api::Noop, noop::DeviceViolation};

    let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    // SAFETY: The noop device is only given hooks and inspected.
    let noop = unsafe { device.as_hal::<Noop>() }.unwrap();
    noop.stall_gpu();
    queue.submit([]);
    let resuming = device.clone();
    noop.run_during_next_wait(move || {
        // SAFETY: The noop device is only told to resume.
        unsafe { resuming.as_hal::<Noop>() }.unwrap().resume_gpu();
    });
    drop(queue);
    assert_eq!(noop.take_violations(), [] as [DeviceViolation; 0]);
}

/// Runs `probe` on another thread, and returns whether it finished within ten seconds: a probe
/// that must not block is waited for only until it finishes.
#[cfg(not(target_family = "wasm"))]
fn finishes_in_time(probe: impl FnOnce() + Send + 'static) -> bool {
    let (sender, receiver) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        // Dropped when `probe` returns or unwinds.
        let _sender = sender;
        probe();
    });
    matches!(
        receiver.recv_timeout(std::time::Duration::from_secs(10)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    )
}

/// Dropping the queue waits for its submissions holding no lock: another thread polls the device
/// meanwhile. No submission can follow a dropped queue, so the lock is taken only for the idle
/// wait.
#[cfg(not(target_family = "wasm"))]
#[test]
fn dropping_the_queue_waits_for_its_submissions_without_blocking_polls() {
    use std::sync::{Arc, Mutex};
    use wgpu_hal::api::Noop;

    let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    // SAFETY: The noop device is only given hooks.
    let noop = unsafe { device.as_hal::<Noop>() }.unwrap();
    noop.stall_gpu();
    queue.submit([]);
    let polled = Arc::new(Mutex::new(None));
    noop.run_during_next_wait({
        let (device, polled) = (device.clone(), Arc::clone(&polled));
        move || {
            let polling = device.clone();
            let finished = finishes_in_time(move || {
                let _ = polling.poll(wgpu::PollType::Poll);
            });
            *polled.lock().unwrap() = Some(finished);
            // SAFETY: The noop device is only told to resume.
            unsafe { device.as_hal::<Noop>() }.unwrap().resume_gpu();
        }
    });
    drop(queue);
    assert_eq!(*polled.lock().unwrap(), Some(true));
}

/// A failed wait while dropping the queue loses the device, and skips the idle wait, which would
/// wait for the submission: dropping cannot panic.
#[cfg(not(target_family = "wasm"))]
#[test]
fn dropping_the_queue_when_its_wait_fails_loses_the_device_without_an_idle_wait() {
    use std::sync::{Arc, Mutex};
    use wgpu_hal::{
        api::Noop,
        noop::{DeviceOperation, DeviceViolation},
    };

    let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    let lost = Arc::new(Mutex::new(None));
    device.set_device_lost_callback({
        let lost = Arc::clone(&lost);
        move |reason, _message| *lost.lock().unwrap() = Some(reason)
    });
    // SAFETY: The noop device is only given hooks and inspected.
    let noop = unsafe { device.as_hal::<Noop>() }.unwrap();
    noop.stall_gpu();
    queue.submit([]);
    noop.fail_next(DeviceOperation::Wait, wgpu_hal::DeviceError::OutOfMemory);
    drop(queue);
    assert_eq!(*lost.lock().unwrap(), Some(wgpu::DeviceLostReason::Unknown));
    assert_eq!(noop.take_violations(), [] as [DeviceViolation; 0]);
    noop.resume_gpu();
}

/// When the wait for the submissions gives up, as GL's does after about two seconds, the idle
/// wait follows: `wgpu_hal::Queue::wait_for_idle` allows it, for the submissions that wait gave up
/// on.
#[cfg(not(target_family = "wasm"))]
#[test]
fn dropping_the_queue_after_its_wait_gave_up_meets_the_idle_waits_requirement() {
    use wgpu_hal::{api::Noop, noop::DeviceViolation};

    let (device, queue) = wgpu::Device::noop(&wgpu::DeviceDescriptor::default());
    // SAFETY: The noop device is only given hooks and inspected.
    let noop = unsafe { device.as_hal::<Noop>() }.unwrap();
    noop.stall_gpu();
    queue.submit([]);
    drop(queue);
    assert_eq!(noop.take_violations(), [] as [DeviceViolation; 0]);
    noop.resume_gpu();
}
