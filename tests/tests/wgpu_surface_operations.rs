//! Surface operations on real windows and backends.
//!
//! `wgpu-validation`'s `api::surface` tests check wgpu-core's surface operations against the
//! noop backend's surfaces. These check what only real backends and windows show, on every
//! adapter that supports the window's surface, with validation layers on:
//!
//! - `held_views`: reconfiguring while the application still holds a presented texture, a view
//!   of it and, where the surface offers `TEXTURE_BINDING`, a bind group using it. DX12's
//!   `ResizeBuffers` fails while anything references a swapchain buffer, which left the surface
//!   unconfigured (<https://github.com/gfx-rs/wgpu/issues/4105>).
//! - `released_texture`: reconfiguring after a `SurfaceTexture` was dropped while unwinding,
//!   while the application holds its texture. The held texture kept its swapchain buffer
//!   referenced, so DX12's `ResizeBuffers` failed the same way.
//! - `discarded_images`: dropping more acquired textures unpresented than the swapchain has
//!   images, rendered to and not, then presenting enough frames for Vulkan's ring of acquire
//!   semaphores to wrap, and configuring again while a discarded image is kept for the next
//!   acquisition. Vulkan's discard did nothing, so the images stayed acquired, and acquiring
//!   waited out its timeout once too many were (<https://github.com/gfx-rs/wgpu/issues/5723>).
//!   It prints whether Vulkan's validation layer is installed: without it, the trial checks
//!   less.
//! - `two_surfaces`: two surfaces of one device, configured on two threads at once, a bounded
//!   number of times. DX12 waits for its queue to become idle with a fence that the device's
//!   surfaces share, and Vulkan's swapchain teardown waits for its queue to become idle, which
//!   both need the configurations excluded from each other and from queue operations
//!   (<https://github.com/gfx-rs/wgpu/pull/10675>).
//! - `configure_race`: one surface reconfigured a bounded number of times, with changing present
//!   modes and frame latencies, while another thread submits; then configured for another device
//!   and back; then dropped and created again.
//! - `scaling_modes`: what each surface lists of `SurfaceScalingMode`, and that each listed mode
//!   configures and presents, and an unlisted one is a validation error. DX12 lists `Stretch` and
//!   `OneToOne` for a window's swapchain, made with their `DXGI_SCALING`, and none for a
//!   composition swapchain. On Vulkan it prints what a frame presented to a window that changed
//!   its size gives, with each scaling and with none.
//!
//! Each adapter gets windows of its own: a window that one API presented to is not reused with
//! another. Like `wgpu_surface`, this binary owns the main thread, which `winit` requires, and runs
//! its trials on it, sharing one event loop.
//!
//! Every trial prints the adapters it runs on, and fails if it ran on none. Setting
//! `WGPU_SURFACE_TEST_ADAPTER` limits the trials to the adapters whose `"{backend:?} {name}"` contains
//! its value, ignoring case. A failure names the adapter, the phase and the round, with the errors
//! and validation messages collected so far; a lost device counts as an error.

#[cfg(any(
    target_arch = "wasm32",
    all(target_vendor = "apple", not(target_os = "macos")),
    target_os = "android",
))]
fn main() {}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(not(target_vendor = "apple"), target_os = "macos"),
    not(target_os = "android"),
))]
fn main() {
    native::main();
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(not(target_vendor = "apple"), target_os = "macos"),
    not(target_os = "android"),
))]
mod native {
    use std::{
        cell::RefCell,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
        thread,
    };

    use winit::{
        application::ApplicationHandler,
        dpi::PhysicalSize,
        event_loop::{ActiveEventLoop, EventLoop},
        platform::run_on_demand::EventLoopExtRunOnDemand,
        window::{Window, WindowAttributes},
    };

    /// The size of every window, and of every configuration of its surface: some backends require
    /// the swapchain to match the window.
    const SIZE: u32 = 256;
    /// How many times `two_surfaces` configures each surface.
    const TWO_SURFACES_ROUNDS: u32 = 20;
    /// How many times `configure_race` reconfigures its surface while another thread submits.
    const RACE_ROUNDS: u32 = 40;
    /// How many frames `configure_race` presents with each device and surface it switches to.
    const FRAMES: u32 = 5;
    /// The most submissions `configure_race`'s submitting thread makes, should it not be stopped.
    const MAX_SUBMISSIONS: u32 = 2000;
    /// Limits the trials to the adapters whose [`adapter_name`] contains its value, ignoring case.
    const ADAPTER_FILTER: &str = "WGPU_SURFACE_TEST_ADAPTER";

    type Scenario = fn(&ActiveEventLoop, &wgpu::Instance) -> Result<(), String>;

    thread_local! {
        static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
    }

    /// Runs `scenario` once the event loop runs.
    struct Harness {
        scenario: Scenario,
        result: Option<Result<(), String>>,
    }

    impl ApplicationHandler for Harness {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
                    Box::new(event_loop.owned_display_handle()),
                ));
            let scenario = self.scenario;
            self.result = Some(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scenario(event_loop, &instance)
                }))
                .unwrap_or_else(|payload| Err(panic_message(&*payload))),
            );
            event_loop.exit();
        }

        fn window_event(
            &mut self,
            _: &ActiveEventLoop,
            _: winit::window::WindowId,
            _: winit::event::WindowEvent,
        ) {
        }
    }

    fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
        if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_owned()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            "panic with a non-string payload".to_owned()
        }
    }

    fn run(scenario: Scenario) -> Result<(), libtest_mimic::Failed> {
        // Messages left by an earlier trial are not this one's.
        let _ = wgpu_hal::VALIDATION_CANARY.get_and_reset();
        EVENT_LOOP.with_borrow_mut(|event_loop| {
            let event_loop = event_loop.get_or_insert_with(|| {
                EventLoop::builder()
                    .build()
                    .unwrap_or_else(|error| panic!("no event loop (no display?): {error}"))
            });
            let mut harness = Harness {
                scenario,
                result: None,
            };
            event_loop.run_app_on_demand(&mut harness).unwrap();
            harness
                .result
                .expect("the event loop ran no scenario")
                .map_err(libtest_mimic::Failed::from)
        })
    }

    type Errors = Arc<Mutex<Vec<String>>>;

    /// A device of `adapter`, whose uncaptured errors, and loss, are collected in the returned list.
    fn device(adapter: &wgpu::Adapter) -> (wgpu::Device, wgpu::Queue, Errors) {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
        // Which wait a Vulkan swapchain's teardown makes: on its presents' fences, or for the queue.
        #[cfg(any(windows, target_os = "linux"))]
        {
            // SAFETY: Only inspected.
            if let Some(hal) = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() } {
                let path = if hal.presents_signal_fences() {
                    "the teardown waits on present fences"
                } else {
                    "no present fences: the teardown waits for the queue"
                };
                println!("{}: {path}", adapter_name(adapter));
            }
        }
        let errors = Errors::default();
        let sink = Arc::clone(&errors);
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            sink.lock().unwrap().push(error.to_string());
        }));
        // A lost device reports no uncaptured error. Dropping the device reports `Destroyed`.
        let sink = Arc::clone(&errors);
        device.set_device_lost_callback(move |reason, message| {
            if !matches!(reason, wgpu::DeviceLostReason::Destroyed) {
                sink.lock()
                    .unwrap()
                    .push(format!("device lost: {reason:?}: {message}"));
            }
        });
        (device, queue, errors)
    }

    /// The errors collected in `errors` so far and the validation layers' messages, for a failure
    /// message.
    fn dump(errors: &[&Errors]) -> String {
        let collected = errors
            .iter()
            .flat_map(|errors| errors.lock().unwrap().clone())
            .collect::<Vec<_>>();
        let messages = wgpu_hal::VALIDATION_CANARY.get_and_reset();
        format!("errors {collected:#?}, validation messages {messages:#?}")
    }

    /// Fails if a device of `adapter` reported an uncaptured error or was lost, or a validation layer
    /// reported a message. Call it once the devices, queues and surfaces it checks are dropped, so
    /// that the messages of their teardown count too.
    fn check(adapter: &str, errors: &[Errors]) -> Result<(), String> {
        let errors = errors
            .iter()
            .flat_map(|errors| core::mem::take(&mut *errors.lock().unwrap()))
            .collect::<Vec<_>>();
        let messages = wgpu_hal::VALIDATION_CANARY.get_and_reset();
        if errors.is_empty() && messages.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{adapter}: errors {errors:?}, validation messages {messages:?}"
            ))
        }
    }

    fn window(event_loop: &ActiveEventLoop) -> Arc<Window> {
        Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_visible(false)
                        .with_inner_size(PhysicalSize::new(SIZE, SIZE)),
                )
                .expect("failed to create window"),
        )
    }

    /// An adapter, with windows of its own and their surfaces.
    type AdapterWithSurfaces = (wgpu::Adapter, Vec<(Arc<Window>, wgpu::Surface<'static>)>);

    /// Each adapter that [`ADAPTER_FILTER`] selects and whose surfaces `windows` new windows of its
    /// own support, with the windows and their surfaces. Prints each adapter it returns, and fails
    /// if there is none: a trial that runs on no adapter checks nothing.
    fn adapters(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        windows: usize,
    ) -> Result<Vec<AdapterWithSurfaces>, String> {
        let filter = std::env::var(ADAPTER_FILTER)
            .ok()
            .map(|filter| filter.to_lowercase());
        let mut skipped = Vec::new();
        let mut selected = Vec::new();
        for adapter in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
            let name = adapter_name(&adapter);
            if filter
                .as_ref()
                .is_some_and(|filter| !name.to_lowercase().contains(filter))
            {
                skipped.push(format!("{name}: not selected by {ADAPTER_FILTER}"));
                continue;
            }
            let surfaces = (0..windows)
                .map(|_| {
                    let window = window(event_loop);
                    let surface = instance.create_surface(Arc::clone(&window)).ok()?;
                    adapter
                        .is_surface_supported(&surface)
                        .then_some((window, surface))
                })
                .collect::<Option<Vec<_>>>();
            match surfaces {
                Some(surfaces) => {
                    println!("adapter: {:?}", adapter.get_info());
                    selected.push((adapter, surfaces));
                }
                None => skipped.push(format!("{name}: does not support the window's surface")),
            }
        }
        if selected.is_empty() {
            Err(format!("ran on no adapter: {skipped:#?}"))
        } else {
            Ok(selected)
        }
    }

    /// `"{backend:?} {name}"`: how failures and [`ADAPTER_FILTER`] name an adapter.
    fn adapter_name(adapter: &wgpu::Adapter) -> String {
        let info = adapter.get_info();
        format!("{:?} {}", info.backend, info.name)
    }

    /// The default configuration of `surface`, at the windows' size, with every usage it offers
    /// that the scenarios use.
    fn config(surface: &wgpu::Surface<'_>, adapter: &wgpu::Adapter) -> wgpu::SurfaceConfiguration {
        let mut config = surface.get_default_config(adapter, SIZE, SIZE).unwrap();
        config.usage |=
            surface.get_capabilities(adapter).usages & wgpu::TextureUsages::TEXTURE_BINDING;
        config
    }

    /// Configures `surface`, or panics with `at` (the adapter, phase and round), the configuration
    /// error and [`dump`]: a failed configuration otherwise surfaces only at the next acquisition.
    fn configure(
        surface: &wgpu::Surface<'_>,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        at: &str,
        errors: &[&Errors],
    ) {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(device, config);
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("{at}: configuring failed: {error}; {}", dump(errors));
        }
    }

    /// Acquires a texture of `surface`, or panics with `at` and [`dump`].
    fn acquire(surface: &wgpu::Surface<'_>, at: &str, errors: &[&Errors]) -> wgpu::SurfaceTexture {
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => panic!("{at}: no texture acquired: {other:?}; {}", dump(errors)),
        }
    }

    fn render(device: &wgpu::Device, queue: &wgpu::Queue, view: &wgpu::TextureView) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        queue.submit([encoder.finish()]);
    }

    /// Acquires a texture of `surface`, renders to it and presents it.
    fn present_frame(
        surface: &wgpu::Surface<'_>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        at: &str,
        errors: &[&Errors],
    ) {
        let frame = acquire(surface, at, errors);
        render(
            device,
            queue,
            &frame.texture.create_view(&Default::default()),
        );
        queue.present(frame);
    }

    fn held_views(event_loop: &ActiveEventLoop, instance: &wgpu::Instance) -> Result<(), String> {
        for (adapter, surfaces) in adapters(event_loop, instance, 1)? {
            let name = adapter_name(&adapter);
            let errors = {
                let (_, surface) = &surfaces[0];
                let (device, queue, errors) = device(&adapter);
                let errs = [&errors];
                let config = config(surface, &adapter);
                let at = format!("{name}: configuring");
                configure(surface, &device, &config, &at, &errs);
                let frame = acquire(surface, &format!("{name}: first frame"), &errs);
                let texture = frame.texture.clone();
                let view = frame.texture.create_view(&Default::default());
                let bind_group = config
                    .usage
                    .contains(wgpu::TextureUsages::TEXTURE_BINDING)
                    .then(|| bind_group(&device, &view));
                render(&device, &queue, &view);
                queue.present(frame);

                let at = format!("{name}: configuring with the views held");
                configure(surface, &device, &config, &at, &errs);
                present_frame(surface, &device, &queue, &at, &errs);
                drop((texture, view, bind_group));
                errors
            };
            drop(surfaces);
            check(&name, &[errors])?;
        }
        Ok(())
    }

    /// A bind group using `view`, as an application holding one would.
    fn bind_group(device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            }],
        })
    }

    fn released_texture(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
    ) -> Result<(), String> {
        for (adapter, surfaces) in adapters(event_loop, instance, 1)? {
            let name = adapter_name(&adapter);
            let errors = {
                let (_, surface) = &surfaces[0];
                let (device, queue, errors) = device(&adapter);
                let errs = [&errors];
                let config = config(surface, &adapter);
                let at = format!("{name}: configuring");
                configure(surface, &device, &config, &at, &errs);
                let held = Mutex::new(None);
                let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let frame = acquire(surface, &format!("{name}: the frame to unwind"), &errs);
                    *held.lock().unwrap() = Some(frame.texture.clone());
                    std::panic::resume_unwind(Box::new("dropping the frame while unwinding"));
                }));
                assert!(unwound.is_err());

                let at = format!("{name}: configuring with the released texture held");
                configure(surface, &device, &config, &at, &errs);
                present_frame(surface, &device, &queue, &at, &errs);
                drop(held);
                errors
            };
            drop(surfaces);
            check(&name, &[errors])?;
        }
        Ok(())
    }

    fn discarded_images(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
    ) -> Result<(), String> {
        report_validation_layer(instance);
        for (adapter, surfaces) in adapters(event_loop, instance, 1)? {
            let name = adapter_name(&adapter);
            let errors = {
                let (_, surface) = &surfaces[0];
                let (device, queue, errors) = device(&adapter);
                let errs = [&errors];
                let config = config(surface, &adapter);
                let at = format!("{name}: configuring");
                configure(surface, &device, &config, &at, &errs);
                // More than a swapchain of this configuration has images, on usual drivers.
                let frames = 2 * (config.desired_maximum_frame_latency + 1) + 1;
                for i in 0..frames {
                    let frame = acquire(surface, &format!("{name}: rendered discard {i}"), &errs);
                    render(
                        &device,
                        &queue,
                        &frame.texture.create_view(&Default::default()),
                    );
                    // Dropped unpresented: the swapchain gets the image back.
                    drop(frame);
                }
                for i in 0..frames {
                    // Dropped without being rendered to.
                    let at = format!("{name}: bare discard {i}");
                    drop(acquire(surface, &at, &errs));
                }
                // Enough presentations for Vulkan's ring of acquire semaphores to wrap.
                for i in 0..frames {
                    let at = format!("{name}: presentation {i}");
                    present_frame(surface, &device, &queue, &at, &errs);
                }
                // Configured again while Vulkan keeps a discarded image for the next acquisition.
                let at = format!("{name}: discard before configuring");
                let frame = acquire(surface, &at, &errs);
                render(
                    &device,
                    &queue,
                    &frame.texture.create_view(&Default::default()),
                );
                drop(frame);
                let at = format!("{name}: configuring with a discarded image kept");
                configure(surface, &device, &config, &at, &errs);
                present_frame(surface, &device, &queue, &at, &errs);
                errors
            };
            drop(surfaces);
            check(&name, &[errors])?;
        }
        Ok(())
    }

    /// Prints whether Vulkan's validation layer is installed. Without it, a Vulkan trial that
    /// passes was checked by no layer: `check` finds no validation message.
    fn report_validation_layer(instance: &wgpu::Instance) {
        #[cfg(any(windows, target_os = "linux"))]
        {
            // SAFETY: The instance is only inspected.
            let Some(vulkan) = (unsafe { instance.as_hal::<wgpu_hal::api::Vulkan>() }) else {
                return;
            };
            // SAFETY: Enumerating the layers has no requirements.
            let layers = unsafe {
                vulkan
                    .shared_instance()
                    .entry()
                    .enumerate_instance_layer_properties()
            }
            .unwrap_or_default();
            let installed = layers
                .iter()
                .any(|layer| layer.layer_name_as_c_str() == Ok(c"VK_LAYER_KHRONOS_validation"));
            println!("Vulkan validation layer installed: {installed}");
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = instance;
    }

    fn two_surfaces(event_loop: &ActiveEventLoop, instance: &wgpu::Instance) -> Result<(), String> {
        for (adapter, surfaces) in adapters(event_loop, instance, 2)? {
            let name = adapter_name(&adapter);
            let errors = {
                let (device, queue, errors) = device(&adapter);
                thread::scope(|scope| {
                    for (index, (_, surface)) in surfaces.iter().enumerate() {
                        let config = config(surface, &adapter);
                        let (device, queue, errors, name) = (&device, &queue, &errors, &name);
                        scope.spawn(move || {
                            let errs = [errors];
                            for round in 0..TWO_SURFACES_ROUNDS {
                                let at = format!("{name}: surface {index} round {round}");
                                configure(surface, device, &config, &at, &errs);
                                present_frame(surface, device, queue, &at, &errs);
                            }
                        });
                    }
                });
                errors
            };
            drop(surfaces);
            check(&name, &[errors])?;
        }
        Ok(())
    }

    /// Sets its flag when dropped, also while the thread unwinds.
    struct StopOnDrop<'a>(&'a AtomicBool);

    impl Drop for StopOnDrop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    /// Submits small batches of work on `device` until `stop` is set, at most [`MAX_SUBMISSIONS`].
    fn submit_until_stopped(device: &wgpu::Device, queue: &wgpu::Queue, stop: &AtomicBool) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for _ in 0..MAX_SUBMISSIONS {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            encoder.clear_buffer(&buffer, 0, None);
            queue.submit([encoder.finish()]);
            // Keep the GPU from falling behind without bound.
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
    }

    fn configure_race(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
    ) -> Result<(), String> {
        for (adapter, mut surfaces) in adapters(event_loop, instance, 1)? {
            let name = adapter_name(&adapter);
            let (window, surface) = surfaces.pop().unwrap();
            let (device, queue, errors) = device(&adapter);
            let (other_device, other_queue, other_errors) = self::device(&adapter);
            let errs = [&errors, &other_errors];
            let config = config(&surface, &adapter);
            let present_modes = surface.get_capabilities(&adapter).present_modes;

            // Reconfiguring while another thread submits.
            let stop = AtomicBool::new(false);
            thread::scope(|scope| {
                scope.spawn(|| submit_until_stopped(&device, &queue, &stop));
                // Stops the submitting thread before the scope joins it, even if this one panics.
                let _stop = StopOnDrop(&stop);
                for round in 0..RACE_ROUNDS {
                    let config = wgpu::SurfaceConfiguration {
                        present_mode: present_modes[round as usize % present_modes.len()],
                        desired_maximum_frame_latency: 1 + round % 3,
                        ..config.clone()
                    };
                    let at = format!("{name}: race round {round}");
                    configure(&surface, &device, &config, &at, &errs);
                    present_frame(&surface, &device, &queue, &at, &errs);
                }
            });

            // Configured for another device, and back.
            let at = format!("{name}: configuring for another device");
            configure(&surface, &other_device, &config, &at, &errs);
            for i in 0..FRAMES {
                let at = format!("{name}: other device frame {i}");
                present_frame(&surface, &other_device, &other_queue, &at, &errs);
            }
            let at = format!("{name}: configuring back");
            configure(&surface, &device, &config, &at, &errs);
            for i in 0..FRAMES {
                let at = format!("{name}: back frame {i}");
                present_frame(&surface, &device, &queue, &at, &errs);
            }

            // Dropped, and created again for the same window.
            drop(surface);
            let surface = instance
                .create_surface(Arc::clone(&window))
                .map_err(|error| error.to_string())?;
            let at = format!("{name}: configuring the recreated surface");
            configure(&surface, &device, &config, &at, &errs);
            for i in 0..FRAMES {
                let at = format!("{name}: recreated frame {i}");
                present_frame(&surface, &device, &queue, &at, &errs);
            }

            drop(surface);
            drop((device, queue, other_device, other_queue));
            check(&name, &[errors, other_errors])?;
        }
        Ok(())
    }

    /// Failures that a trial collects, rather than stopping at the first, so that one run shows
    /// every check that fails.
    #[derive(Default)]
    struct Failures(Vec<String>);

    impl Failures {
        fn push(&mut self, failure: String) {
            println!("FAILED: {failure}");
            self.0.push(failure);
        }

        fn into_result(self) -> Result<(), String> {
            if self.0.is_empty() {
                Ok(())
            } else {
                Err(self.0.join("\n"))
            }
        }
    }

    /// Configures `surface`, returning the validation error it reports, if any.
    fn try_configure(
        surface: &wgpu::Surface<'_>,
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) -> Option<wgpu::Error> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(device, config);
        pollster::block_on(scope.pop())
    }

    /// Acquires a texture of `surface`, and renders to and presents it if there is one. Returns
    /// what the acquisition gave.
    fn acquire_and_present(
        surface: &wgpu::Surface<'_>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> String {
        match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => {
                render(
                    device,
                    queue,
                    &frame.texture.create_view(&Default::default()),
                );
                queue.present(frame);
                "Success".to_owned()
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                render(
                    device,
                    queue,
                    &frame.texture.create_view(&Default::default()),
                );
                queue.present(frame);
                "Suboptimal".to_owned()
            }
            other => format!("{other:?}"),
        }
    }

    const EXPLICIT_SCALING_MODES: [wgpu::SurfaceScalingMode; 3] = [
        wgpu::SurfaceScalingMode::Stretch,
        wgpu::SurfaceScalingMode::AspectRatio,
        wgpu::SurfaceScalingMode::OneToOne,
    ];

    /// `scaling_modes`: on each adapter, the scaling modes its surface lists (printed); `Auto` and
    /// each listed mode configure and present with no error or validation message; a mode the
    /// surface does not list is a validation error that keeps the configuration. On DX12, a
    /// window's swapchain lists `Stretch` and `OneToOne`, has the `DXGI_SCALING` of the mode it was
    /// configured with, and is replaced when wgpu-hal is asked for another scaling on a configured
    /// surface (`ResizeBuffers` keeps a swapchain's scaling), while wgpu-hal refuses the unlisted
    /// `AspectRatio` with `SurfaceError::UnsupportedScalingMode`, keeping the swapchain; a
    /// composition swapchain lists none, and wgpu-hal refuses each. On OpenGL, which lists none,
    /// wgpu-hal refuses each.
    /// On Vulkan, a caller's present scaling in the swapchain's creation chain is the only one
    /// (a second would break the chain's unique structure types), and for each scaling, and for
    /// the platform's own placement, it prints what a frame presented to a window that changed
    /// its size gives, and the scaled extents the surface reports.
    fn scaling_modes(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
    ) -> Result<(), String> {
        let mut failures = Failures::default();
        for (adapter, surfaces) in adapters(event_loop, instance, 1)? {
            let name = adapter_name(&adapter);
            let surface = &surfaces[0].1;
            let caps = surface.get_capabilities(&adapter);
            println!("{name}: scaling modes {:?}", caps.scaling_modes);
            let (device, queue, errors) = device(&adapter);
            let errs = [&errors];
            let base = config(surface, &adapter);

            // `Auto`, and each listed mode.
            for scaling_mode in
                core::iter::once(wgpu::SurfaceScalingMode::Auto).chain(caps.scaling_modes.clone())
            {
                let config = wgpu::SurfaceConfiguration {
                    scaling_mode,
                    ..base.clone()
                };
                if let Some(error) = try_configure(surface, &device, &config) {
                    failures.push(format!("{name}: {scaling_mode:?}: {error}"));
                    continue;
                }
                for _ in 0..FRAMES {
                    let got = acquire_and_present(surface, &device, &queue);
                    if !matches!(got.as_str(), "Success" | "Suboptimal") {
                        failures.push(format!("{name}: {scaling_mode:?}: acquired {got}"));
                        break;
                    }
                }
            }

            // Each mode the surface does not list: a validation error, the configuration kept.
            for scaling_mode in EXPLICIT_SCALING_MODES
                .into_iter()
                .filter(|mode| !caps.scaling_modes.contains(mode))
            {
                configure(surface, &device, &base, &name, &errs);
                let config = wgpu::SurfaceConfiguration {
                    scaling_mode,
                    ..base.clone()
                };
                let expected = format!(
                    "The requested scaling mode {scaling_mode:?} could not be applied; \
                     supported modes: {:?}",
                    caps.scaling_modes
                );
                match try_configure(surface, &device, &config) {
                    Some(error) if error.to_string().contains(&expected) => {}
                    other => failures.push(format!(
                        "{name}: unlisted {scaling_mode:?}: expected a validation error, got \
                         {other:?}"
                    )),
                }
                let got = acquire_and_present(surface, &device, &queue);
                if !matches!(got.as_str(), "Success" | "Suboptimal") {
                    failures.push(format!(
                        "{name}: after the unlisted {scaling_mode:?}: acquired {got}"
                    ));
                }
            }

            #[cfg(any(windows, target_os = "linux"))]
            if adapter.get_info().backend == wgpu::Backend::Vulkan {
                vulkan_scaling(
                    instance,
                    &adapter,
                    &surfaces[0].0,
                    surface,
                    (&device, &queue),
                    &base,
                    &caps,
                    &mut failures,
                );
            }

            drop(surfaces);
            drop((device, queue));
            if let Err(failure) = check(&name, &[errors]) {
                failures.push(failure);
            }
        }

        #[cfg(windows)]
        dx12_scaling(event_loop, &mut failures);
        #[cfg(any(windows, target_os = "linux"))]
        gl_scaling(event_loop, &mut failures);

        failures.into_result()
    }

    /// A wgpu-hal configuration of a window's surface at the windows' size with `scaling_mode`.
    #[cfg(any(windows, target_os = "linux"))]
    fn hal_config(scaling_mode: wgpu::SurfaceScalingMode) -> wgpu_hal::SurfaceConfiguration {
        wgpu_hal::SurfaceConfiguration {
            maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::Fifo,
            composite_alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            scaling_mode,
            format: wgpu::TextureFormat::Bgra8Unorm,
            color_space: wgpu::SurfaceColorSpace::Srgb,
            extent: wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
            usage: wgpu::TextureUses::COLOR_TARGET,
            view_formats: Vec::new(),
        }
    }

    /// Whether wgpu-hal refuses to configure `surface` for `device` with each explicit scaling
    /// mode, by name; a failure is pushed for each mode it does not.
    ///
    /// # Safety
    ///
    /// wgpu-core did not configure `surface`, and nothing uses it.
    #[cfg(any(windows, target_os = "linux"))]
    unsafe fn hal_refuses_each_mode<A: wgpu_hal::Api>(
        name: &str,
        surface: &A::Surface,
        device: &A::Device,
        failures: &mut Failures,
    ) {
        use wgpu_hal::Surface as _;
        for scaling_mode in EXPLICIT_SCALING_MODES {
            // SAFETY: Our caller's contract; a configuration that succeeds is undone at once.
            let configured = unsafe { surface.configure(device, &hal_config(scaling_mode)) };
            println!("{name}: wgpu-hal configured with {scaling_mode:?}: {configured:?}");
            match configured {
                Err(wgpu_hal::SurfaceError::UnsupportedScalingMode) => {}
                Ok(()) => {
                    // SAFETY: As above.
                    unsafe { surface.unconfigure(device) };
                    failures.push(format!(
                        "{name}: wgpu-hal configured with the unlisted {scaling_mode:?}"
                    ));
                }
                Err(other) => failures.push(format!(
                    "{name}: wgpu-hal configured with {scaling_mode:?}: {other:?}"
                )),
            }
        }
    }

    /// The OpenGL half of [`scaling_modes`]: its surfaces list no mode, and wgpu-hal refuses each
    /// by name (wgpu-core never asks it for one).
    #[cfg(any(windows, target_os = "linux"))]
    fn gl_scaling(event_loop: &ActiveEventLoop, failures: &mut Failures) {
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
            event_loop.owned_display_handle(),
        ));
        desc.backends = wgpu::Backends::GL;
        let instance = wgpu::Instance::new(desc);
        let gl_adapters = match adapters(event_loop, &instance, 1) {
            Ok(adapters) => adapters,
            Err(error) => return failures.push(format!("GL: {error}")),
        };
        for (adapter, surfaces) in gl_adapters {
            let name = adapter_name(&adapter);
            let (device, queue, errors) = device(&adapter);
            {
                // SAFETY: Only wgpu-hal uses the surface, which wgpu-core did not configure.
                let hal_surface = unsafe { surfaces[0].1.as_hal::<wgpu_hal::api::Gles>() }.unwrap();
                // SAFETY: As above.
                let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Gles>() }.unwrap();
                // SAFETY: As above.
                unsafe {
                    hal_refuses_each_mode::<wgpu_hal::api::Gles>(
                        &name,
                        &hal_surface,
                        &hal_device,
                        failures,
                    )
                };
            }
            drop(surfaces);
            drop((device, queue));
            if let Err(failure) = check(&name, &[errors]) {
                failures.push(failure);
            }
        }
    }

    /// The Vulkan half of [`scaling_modes`], on `surface` of `window`, configured for `device`.
    #[cfg(any(windows, target_os = "linux"))]
    #[allow(clippy::too_many_arguments)]
    fn vulkan_scaling(
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        window: &Window,
        surface: &wgpu::Surface<'_>,
        (device, queue): (&wgpu::Device, &wgpu::Queue),
        base: &wgpu::SurfaceConfiguration,
        caps: &wgpu::SurfaceCapabilities,
        failures: &mut Failures,
    ) {
        use ash::vk;

        let name = adapter_name(adapter);
        // The scaled extents and the scaling the surface reports, for FIFO.
        if !caps.scaling_modes.is_empty() {
            // SAFETY: Only inspected.
            let (entry, raw_instance) = {
                let hal = unsafe { instance.as_hal::<wgpu_hal::api::Vulkan>() }.unwrap();
                let shared = hal.shared_instance();
                (shared.entry().clone(), shared.raw_instance().clone())
            };
            // SAFETY: Only inspected.
            let physical_device = unsafe { adapter.as_hal::<wgpu_hal::api::Vulkan>() }
                .unwrap()
                .raw_physical_device();
            // SAFETY: Only inspected.
            let raw_surface = unsafe {
                surface
                    .as_hal::<wgpu_hal::api::Vulkan>()
                    .unwrap()
                    .raw_native_handle()
            }
            .unwrap();
            let functor = ash::khr::get_surface_capabilities2::Instance::new(&entry, &raw_instance);
            let mut scaling = vk::SurfacePresentScalingCapabilitiesEXT::default();
            let mut caps2 = vk::SurfaceCapabilities2KHR::default().push_next(&mut scaling);
            let mut mode =
                vk::SurfacePresentModeEXT::default().present_mode(vk::PresentModeKHR::FIFO);
            let info = vk::PhysicalDeviceSurfaceInfo2KHR::default()
                .surface(raw_surface)
                .push_next(&mut mode);
            // SAFETY: The surface lists scaling modes, so the instance has the extensions.
            let queried = unsafe {
                functor.get_physical_device_surface_capabilities2(
                    physical_device,
                    &info,
                    &mut caps2,
                )
            };
            // Read out before `scaling`, which `caps2` borrows.
            let surface_caps = caps2.surface_capabilities;
            match queried {
                Ok(()) => println!(
                    "{name}: FIFO: scaling {:?}, gravity {:?} / {:?}, scaled extents {}x{} to \
                     {}x{}, image extents {}x{} to {}x{}, current {}x{}",
                    scaling.supported_present_scaling,
                    scaling.supported_present_gravity_x,
                    scaling.supported_present_gravity_y,
                    scaling.min_scaled_image_extent.width,
                    scaling.min_scaled_image_extent.height,
                    scaling.max_scaled_image_extent.width,
                    scaling.max_scaled_image_extent.height,
                    surface_caps.min_image_extent.width,
                    surface_caps.min_image_extent.height,
                    surface_caps.max_image_extent.width,
                    surface_caps.max_image_extent.height,
                    surface_caps.current_extent.width,
                    surface_caps.current_extent.height,
                ),
                Err(error) => failures.push(format!("{name}: surface capabilities: {error}")),
            }
        }

        // The configurations whose mismatched presents are printed: each listed mode, `Auto`,
        // and a caller's chain asking for no scaling behaviour, the platform's own placement.
        let mut cases: Vec<(String, wgpu::SurfaceConfiguration, bool)> = caps
            .scaling_modes
            .iter()
            .map(|&scaling_mode| {
                (
                    format!("{scaling_mode:?}"),
                    wgpu::SurfaceConfiguration {
                        scaling_mode,
                        ..base.clone()
                    },
                    false,
                )
            })
            .collect();
        cases.push(("Auto".to_owned(), base.clone(), false));
        cases.push((
            "a caller's scaling behaviour 0 (the platform's own)".to_owned(),
            base.clone(),
            true,
        ));
        for (case, config, callers_zero_scaling) in cases {
            let _ = window.request_inner_size(PhysicalSize::new(SIZE, SIZE));
            let mut zero = vk::SwapchainPresentScalingCreateInfoEXT::default();
            if callers_zero_scaling {
                // SAFETY: `zero` is a valid chain of one structure, which outlives the
                // configuration below, and the device has the extension where the surface lists
                // scaling modes; without it, a zero structure is valid too
                // (VUID-VkSwapchainPresentScalingCreateInfoKHR-swapchainMaintenance1-10154).
                unsafe {
                    surface
                        .as_hal::<wgpu_hal::api::Vulkan>()
                        .unwrap()
                        .set_next_swapchain_create_chain((&raw mut zero).cast());
                }
            }
            if let Some(error) = try_configure(surface, device, &config) {
                failures.push(format!("{name}: {case}: {error}"));
                continue;
            }
            for _ in 0..2 {
                let got = acquire_and_present(surface, device, queue);
                if !matches!(got.as_str(), "Success" | "Suboptimal") {
                    failures.push(format!("{name}: {case}: before the resize, acquired {got}"));
                }
            }
            let _ = window.request_inner_size(PhysicalSize::new(SIZE + 48, SIZE + 32));
            let size = window.inner_size();
            if size == PhysicalSize::new(SIZE, SIZE) {
                failures.push(format!("{name}: {case}: the window kept its size"));
            }
            let first = acquire_and_present(surface, device, queue);
            let second = acquire_and_present(surface, device, queue);
            println!(
                "{name}: {case}: swapchain {SIZE}x{SIZE}, window {}x{}: {first}, then {second}",
                size.width, size.height
            );
        }
        let _ = window.request_inner_size(PhysicalSize::new(SIZE, SIZE));
    }

    /// The DX12 half of [`scaling_modes`], on instances that make each kind of swapchain.
    #[cfg(windows)]
    fn dx12_scaling(event_loop: &ActiveEventLoop, failures: &mut Failures) {
        use wgpu_hal::Surface as _;
        use windows::Win32::Graphics::Dxgi::{
            DXGI_SCALING, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH,
        };

        /// The scaling of `surface`'s swapchain.
        fn swapchain_scaling(surface: &wgpu_hal::dx12::Surface) -> Option<DXGI_SCALING> {
            let swap_chain = surface.swap_chain()?;
            // SAFETY: A live swapchain.
            unsafe { swap_chain.GetDesc1() }
                .ok()
                .map(|desc| desc.Scaling)
        }

        let instance = |kind| {
            let mut desc = wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
                event_loop.owned_display_handle(),
            ));
            desc.backends = wgpu::Backends::DX12;
            desc.backend_options.dx12.presentation_system = kind;
            wgpu::Instance::new(desc)
        };

        // A window's swapchain.
        let hwnd = instance(wgpu::Dx12SwapchainKind::DxgiFromHwnd);
        let window_adapters = match adapters(event_loop, &hwnd, 2) {
            Ok(adapters) => adapters,
            Err(error) => return failures.push(format!("DX12, window swapchains: {error}")),
        };
        for (adapter, surfaces) in window_adapters {
            let name = adapter_name(&adapter);
            let (_, surface) = &surfaces[0];
            let caps = surface.get_capabilities(&adapter);
            if caps.scaling_modes
                != [
                    wgpu::SurfaceScalingMode::Stretch,
                    wgpu::SurfaceScalingMode::OneToOne,
                ]
            {
                failures.push(format!(
                    "{name}, a window's swapchain: scaling modes {:?}",
                    caps.scaling_modes
                ));
            }
            let (device, queue, errors) = device(&adapter);
            let base = config(surface, &adapter);
            for (scaling_mode, expected) in [
                (wgpu::SurfaceScalingMode::Auto, DXGI_SCALING_STRETCH),
                (wgpu::SurfaceScalingMode::Stretch, DXGI_SCALING_STRETCH),
                (wgpu::SurfaceScalingMode::OneToOne, DXGI_SCALING_NONE),
            ] {
                let config = wgpu::SurfaceConfiguration {
                    scaling_mode,
                    ..base.clone()
                };
                if let Some(error) = try_configure(surface, &device, &config) {
                    failures.push(format!("{name}: {scaling_mode:?}: {error}"));
                    continue;
                }
                // SAFETY: Only inspected.
                let scaling =
                    swapchain_scaling(&unsafe { surface.as_hal::<wgpu_hal::api::Dx12>() }.unwrap());
                println!("{name}: {scaling_mode:?}: the swapchain's scaling {scaling:?}");
                if scaling != Some(expected) {
                    failures.push(format!(
                        "{name}: {scaling_mode:?}: the swapchain's scaling {scaling:?}, not \
                         {expected:?}"
                    ));
                }
                let got = acquire_and_present(surface, &device, &queue);
                if !matches!(got.as_str(), "Success" | "Suboptimal") {
                    failures.push(format!("{name}: {scaling_mode:?}: acquired {got}"));
                }
            }

            // wgpu-hal asked for another scaling on a configured surface, which wgpu-core never
            // does (it unconfigures first), on the other window's surface, which wgpu-core did not
            // configure.
            let (_, other) = &surfaces[1];
            {
                // SAFETY: Only wgpu-hal configures the surface, with no GPU work using it, and the
                // device is idle; the surface is unconfigured before its device is dropped.
                let hal_surface = unsafe { other.as_hal::<wgpu_hal::api::Dx12>() }.unwrap();
                let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Dx12>() }.unwrap();
                for (scaling_mode, expected) in [
                    (wgpu::SurfaceScalingMode::Stretch, DXGI_SCALING_STRETCH),
                    (wgpu::SurfaceScalingMode::OneToOne, DXGI_SCALING_NONE),
                    (wgpu::SurfaceScalingMode::Stretch, DXGI_SCALING_STRETCH),
                ] {
                    // SAFETY: As above.
                    let configured =
                        unsafe { hal_surface.configure(&hal_device, &hal_config(scaling_mode)) };
                    let scaling = swapchain_scaling(&hal_surface);
                    println!(
                        "{name}: wgpu-hal configured with {scaling_mode:?}: {configured:?}, the \
                         swapchain's scaling {scaling:?}"
                    );
                    if configured.is_err() || scaling != Some(expected) {
                        failures.push(format!(
                            "{name}: wgpu-hal configured with {scaling_mode:?}: {configured:?}, \
                             the swapchain's scaling {scaling:?}, not {expected:?}"
                        ));
                    }
                }
                // The unlisted `AspectRatio`: refused, before the swapchain is touched.
                // SAFETY: As above.
                let configured = unsafe {
                    hal_surface.configure(
                        &hal_device,
                        &hal_config(wgpu::SurfaceScalingMode::AspectRatio),
                    )
                };
                let scaling = swapchain_scaling(&hal_surface);
                println!(
                    "{name}: wgpu-hal configured with AspectRatio: {configured:?}, the \
                     swapchain's scaling {scaling:?}"
                );
                if !matches!(
                    configured,
                    Err(wgpu_hal::SurfaceError::UnsupportedScalingMode)
                ) || scaling != Some(DXGI_SCALING_STRETCH)
                {
                    failures.push(format!(
                        "{name}: wgpu-hal configured with AspectRatio: {configured:?}, the \
                         swapchain's scaling {scaling:?}, not refused with the swapchain kept"
                    ));
                }
                // SAFETY: As above.
                unsafe { hal_surface.unconfigure(&hal_device) };
            }

            drop(surfaces);
            drop((device, queue));
            if let Err(failure) = check(&name, &[errors]) {
                failures.push(failure);
            }
        }

        // A composition swapchain, placed by its visual.
        let visual = instance(wgpu::Dx12SwapchainKind::DxgiFromVisual);
        let composition_adapters = match adapters(event_loop, &visual, 1) {
            Ok(adapters) => adapters,
            Err(error) => return failures.push(format!("DX12, composition swapchains: {error}")),
        };
        for (adapter, surfaces) in composition_adapters {
            let name = adapter_name(&adapter);
            let caps = surfaces[0].1.get_capabilities(&adapter);
            println!(
                "{name}, a composition swapchain: scaling modes {:?}",
                caps.scaling_modes
            );
            if !caps.scaling_modes.is_empty() {
                failures.push(format!(
                    "{name}, a composition swapchain: scaling modes {:?}",
                    caps.scaling_modes
                ));
            }
            let (device, queue, errors) = device(&adapter);
            {
                // SAFETY: Only wgpu-hal uses the surface, which wgpu-core did not configure.
                let hal_surface = unsafe { surfaces[0].1.as_hal::<wgpu_hal::api::Dx12>() }.unwrap();
                // SAFETY: As above.
                let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Dx12>() }.unwrap();
                // SAFETY: As above.
                unsafe {
                    hal_refuses_each_mode::<wgpu_hal::api::Dx12>(
                        &format!("{name}, a composition swapchain"),
                        &hal_surface,
                        &hal_device,
                        failures,
                    )
                };
            }
            drop(surfaces);
            drop((device, queue));
            if let Err(failure) = check(&name, &[errors]) {
                failures.push(failure);
            }
        }
    }

    pub fn main() {
        let mut args = libtest_mimic::Arguments::from_args();
        // `winit` requires the main thread; this makes libtest-mimic run inline.
        args.test_threads = Some(1);

        let trials = [
            ("held_views", held_views as Scenario),
            ("released_texture", released_texture),
            ("discarded_images", discarded_images),
            ("two_surfaces", two_surfaces),
            ("configure_race", configure_race),
            ("scaling_modes", scaling_modes),
        ]
        .map(|(name, scenario)| {
            let trial = libtest_mimic::Trial::test(name, move || run(scenario));
            // As in `wgpu_surface`: ignored on Linux without a display.
            #[cfg(any(
                target_os = "linux",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            ))]
            let trial = {
                let have_display = ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY"]
                    .iter()
                    .any(|var| std::env::var(var).is_ok_and(|s| !s.is_empty()));
                trial.with_ignored_flag(!have_display)
            };
            trial
        });

        libtest_mimic::run(&args, trials.into()).exit_if_failed();
    }
}
