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
        texture_binding_view_dimension: None,
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
