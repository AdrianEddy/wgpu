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
