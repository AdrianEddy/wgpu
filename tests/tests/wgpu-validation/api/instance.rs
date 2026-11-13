mod multi_instance {
    #![cfg(not(any(target_arch = "wasm32", miri)))]

    async fn get() -> wgpu::Adapter {
        let adapter = {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::from_env().unwrap_or_default(),
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap()
        };

        log::info!("Selected adapter: {:?}", adapter.get_info());

        adapter
    }

    #[test]
    pub fn multi_instance() {
        {
            wgpu_test::init_logger();

            // Sequential instances.
            for _ in 0..3 {
                pollster::block_on(get());
            }

            // Concurrent instances
            let _instances: Vec<_> = (0..3).map(|_| pollster::block_on(get())).collect();
        }
    }
}

mod request_adapter_error {
    fn id(backends: wgpu::Backends) -> wgpu::InstanceDescriptor {
        wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        }
    }

    fn adapter_error(desc: wgpu::InstanceDescriptor) -> String {
        let instance = wgpu::Instance::new(desc);
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn no_backends_requested() {
        assert_eq!(
            adapter_error(id(wgpu::Backends::empty())),
            "No suitable graphics adapter found; \
            noop not requested, \
            vulkan not requested, \
            metal not requested, \
            dx12 not requested, \
            gl not requested, \
            webgpu not requested"
        );
    }

    /// This test nominally tests the noop backend, but it also exercises the logic for other backends
    /// that fail to return any adapter.
    #[test]
    fn noop_not_enabled() {
        assert_eq!(
            adapter_error(id(wgpu::Backends::NOOP)),
            "No suitable graphics adapter found; \
            noop not explicitly enabled, \
            vulkan not requested, \
            metal not requested, \
            dx12 not requested, \
            gl not requested, \
            webgpu not requested"
        );
    }

    #[test]
    fn no_compiled_support() {
        // Whichever platform we are on, try asking for a backend that definitely will be
        // cfged out regardless of feature flags. (Not that these tests run on wasm at all yet.)

        #[cfg(target_family = "wasm")]
        assert_eq!(
            adapter_error(id(wgpu::Backends::METAL)),
            "No suitable graphics adapter found; \
            noop not requested, \
            vulkan not requested, \
            metal support not compiled in, \
            dx12 not requested, \
            gl not requested, \
            webgpu not requested"
        );

        #[cfg(not(target_family = "wasm"))]
        assert_eq!(
            adapter_error(id(wgpu::Backends::BROWSER_WEBGPU)),
            "No suitable graphics adapter found; \
            noop not requested, \
            vulkan not requested, \
            metal not requested, \
            dx12 not requested, \
            gl not requested, \
            webgpu support not compiled in"
        );
    }
}

/// `request_adapter` lets each backend select its adapter without exposing the others
/// (`wgpu_hal::Instance::request_adapter`), and checks that selection as it checks the adapters of
/// a full enumeration.
mod lazy_adapter {
    /// These expose the adapters of every native backend (DX12 creates a device for each one it
    /// exposes), so they need the machine's drivers. They skip where there is no adapter of their
    /// kind.
    #[cfg(not(any(target_arch = "wasm32", miri)))]
    mod device_tests {
        use std::sync::Arc;

        use wgpu_core as wgc;
        use wgpu_types as wgt;

        const OPTIONS: [(wgt::PowerPreference, bool); 6] = [
            (wgt::PowerPreference::None, false),
            (wgt::PowerPreference::LowPower, false),
            (wgt::PowerPreference::HighPerformance, false),
            (wgt::PowerPreference::None, true),
            (wgt::PowerPreference::LowPower, true),
            (wgt::PowerPreference::HighPerformance, true),
        ];

        fn request(
            power_preference: wgt::PowerPreference,
            force_fallback_adapter: bool,
        ) -> wgt::RequestAdapterOptions<&'static wgc::instance::Surface> {
            wgt::RequestAdapterOptions {
                power_preference,
                force_fallback_adapter,
                compatible_surface: None,
                apply_limit_buckets: false,
            }
        }

        fn instance(
            backends: wgt::Backends,
            telemetry: Option<wgpu_hal::Telemetry>,
        ) -> Arc<wgc::instance::Instance> {
            wgc::instance::Instance::new(
                "test",
                wgt::instance::InstanceDescriptor {
                    backends,
                    ..wgt::instance::InstanceDescriptor::new_without_display_handle()
                },
                telemetry,
            )
        }

        /// A backend that does not select its adapter itself is enumerated, so
        /// `request_adapter` selects the adapter that `request_adapters` ranks first. DX12
        /// selects its adapter itself (see [`dx12`]).
        #[test]
        fn enumerating_backends_select_what_ranks_first() {
            wgpu_test::init_logger();

            let mut checked = Vec::new();
            for backend in [wgt::Backend::Vulkan, wgt::Backend::Metal, wgt::Backend::Gl] {
                let instance = instance(backend.into(), None);
                for (power_preference, force_fallback_adapter) in OPTIONS {
                    let request = request(power_preference, force_fallback_adapter);
                    let ranked_first = instance
                        .request_adapters(&request, backend.into())
                        .map(|adapters| adapters[0].get_info());
                    let selected = instance
                        .request_adapter(&request, backend.into())
                        .map(|adapter| adapter.get_info());
                    assert_eq!(
                        selected, ranked_first,
                        "{backend:?}, {power_preference:?}, \
                        force_fallback_adapter: {force_fallback_adapter}"
                    );
                }
                let names: Vec<String> = instance
                    .enumerate_adapters(backend.into(), false)
                    .iter()
                    .map(|adapter| adapter.get_info().name)
                    .collect();
                if !names.is_empty() {
                    checked.push(format!("{backend:?}: {}", names.join(", ")));
                }
            }
            if checked.is_empty() {
                println!("skipped: no Vulkan, Metal or GL adapter");
                return;
            }
            println!("{}", checked.join("\n"));
        }

        /// DX12 ranks DXGI's adapters before it exposes any: by
        /// `IDXGIFactory6::EnumAdapterByGpuPreference` for `LowPower` and `HighPerformance`, with
        /// software adapters last, in `IDXGIFactory1::EnumAdapters1` order for `None`, and only
        /// its software adapters for `force_fallback_adapter`. It exposes them in that order until
        /// one exposes, so only the adapters up to the one it selects get a device.
        #[cfg(windows)]
        mod dx12 {
            use std::sync::Mutex;

            use windows::Win32::{Foundation::LUID, Graphics::Dxgi::*};

            use super::*;

            /// The adapters that DX12 tried to expose, in order, and whether each exposed: DX12's
            /// telemetry reports every attempt. Only [`selects_in_dxgi_order`]'s instances report.
            static TRIED: Mutex<Vec<(LUID, bool)>> = Mutex::new(Vec::new());

            fn take_tried() -> Vec<(LUID, bool)> {
                core::mem::take(&mut *TRIED.lock().unwrap())
            }

            const TELEMETRY: wgpu_hal::Telemetry = wgpu_hal::Telemetry {
                d3d12_expose_adapter: |desc, _driver_version, result| {
                    let exposed = matches!(result, wgpu_hal::D3D12ExposeAdapterResult::Success(..));
                    TRIED.lock().unwrap().push((desc.AdapterLuid, exposed));
                },
            };

            /// DXGI 1.6's factory, which the preferences need, if this Windows has it.
            fn factory() -> Option<IDXGIFactory6> {
                unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.ok()
            }

            /// DXGI's adapters, in the order that DX12 is to try them.
            fn dxgi_order(
                power_preference: wgt::PowerPreference,
                force_fallback_adapter: bool,
            ) -> Vec<LUID> {
                let factory = factory().expect("the test checked for DXGI 1.6");
                let preference = match power_preference {
                    wgt::PowerPreference::None => None,
                    wgt::PowerPreference::LowPower => Some(DXGI_GPU_PREFERENCE_MINIMUM_POWER),
                    wgt::PowerPreference::HighPerformance => {
                        Some(DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
                    }
                };
                let mut adapters = Vec::new();
                for index in 0.. {
                    let adapter: windows::core::Result<IDXGIAdapter1> = match preference {
                        Some(preference) => unsafe {
                            factory.EnumAdapterByGpuPreference(index, preference)
                        },
                        None => unsafe { factory.EnumAdapters1(index) },
                    };
                    match adapter {
                        Ok(adapter) => {
                            let desc = unsafe { adapter.GetDesc1() }.unwrap();
                            let software = DXGI_ADAPTER_FLAG(desc.Flags as i32)
                                .contains(DXGI_ADAPTER_FLAG_SOFTWARE);
                            adapters.push((desc.AdapterLuid, software));
                        }
                        Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                        Err(error) => panic!("enumerating DXGI's adapters failed: {error}"),
                    }
                }
                if force_fallback_adapter {
                    adapters.retain(|&(_, software)| software);
                } else if preference.is_some() {
                    adapters.sort_by_key(|&(_, software)| software);
                }
                adapters.into_iter().map(|(luid, _)| luid).collect()
            }

            fn luid(adapter: Arc<wgc::instance::Adapter>) -> LUID {
                let raw = unsafe { adapter.as_hal::<wgpu_hal::api::Dx12>() }.unwrap();
                unsafe { raw.as_raw().GetDesc2() }.unwrap().AdapterLuid
            }

            /// Also with Vulkan's adapters, which are all exposed: DX12 still tries only the
            /// adapters it tries alone, and creates a device for the one it selects even where a
            /// Vulkan adapter is selected.
            #[test]
            fn selects_in_dxgi_order() {
                wgpu_test::init_logger();

                if factory().is_none() {
                    println!("skipped: no DXGI 1.6 (`IDXGIFactory6`)");
                    return;
                }
                let dx12 = instance(wgt::Backends::DX12, Some(TELEMETRY));
                let with_vulkan =
                    instance(wgt::Backends::DX12 | wgt::Backends::VULKAN, Some(TELEMETRY));
                for (power_preference, force_fallback_adapter) in OPTIONS {
                    let context = format!(
                        "{power_preference:?}, force_fallback_adapter: {force_fallback_adapter}"
                    );
                    let request = request(power_preference, force_fallback_adapter);

                    // The adapters that DX12 keeps, and whether each exposes, from a full
                    // enumeration, and wgpu-core's ranking of them.
                    take_tried();
                    let ranked = dx12.request_adapters(&request, wgt::Backends::DX12);
                    let enumerated = take_tried();
                    if enumerated.is_empty() {
                        // Every option enumerates the same adapters.
                        println!("skipped: DX12 has no adapter");
                        return;
                    }

                    // DX12 tries them in DXGI's order until one exposes. If none does, it selects
                    // none, and wgpu-core enumerates them all.
                    let order: Vec<(LUID, bool)> =
                        dxgi_order(power_preference, force_fallback_adapter)
                            .into_iter()
                            .filter_map(|luid| {
                                enumerated.iter().find(|&&(kept, _)| kept == luid).copied()
                            })
                            .collect();
                    let selected = order.iter().position(|&(_, exposed)| exposed);
                    let expected = match selected {
                        Some(selected) => order[..=selected].to_vec(),
                        None => order.iter().chain(&enumerated).copied().collect(),
                    };

                    let adapter = dx12.request_adapter(&request, wgt::Backends::DX12);
                    assert_eq!(take_tried(), expected, "{context}: the adapters tried");
                    let Some(selected) = selected else {
                        assert!(
                            adapter.is_err(),
                            "{context}: an adapter DXGI does not order"
                        );
                        continue;
                    };
                    let adapter =
                        adapter.unwrap_or_else(|error| panic!("{context}: no adapter: {error}"));
                    let info = adapter.get_info();
                    println!("{context}: {} ({:?})", info.name, info.device_type);
                    assert_eq!(luid(adapter), order[selected].0, "{context}: the selection");

                    // DXGI's order agrees with wgpu-core's ranking of a full enumeration for
                    // `None` (both are enumeration order) and for `force_fallback_adapter`
                    // (`Cpu` only). For a preference it follows DXGI's own adapter classes and
                    // can follow a per-app GPU preference in Windows' graphics settings, so its
                    // selection need not have the device type that ranks first.
                    let ranked = ranked.unwrap_or_else(|error| panic!("{context}: {error}"));
                    let ranked_first = ranked[0].get_info();
                    if force_fallback_adapter {
                        assert_eq!(info.device_type, wgt::DeviceType::Cpu, "{context}");
                    }
                    let same_type = info.device_type == ranked_first.device_type;
                    if power_preference == wgt::PowerPreference::None {
                        assert_eq!(info, ranked_first, "{context}: not the first enumerated");
                    } else if !same_type {
                        println!("{context}: DXGI's order selects {info:?}");
                        println!("{context}: the full ranking puts first {ranked_first:?}");
                    }

                    // With Vulkan, DX12 still tries only these adapters, and wgpu-core ranks its
                    // selection with Vulkan's adapters.
                    let context = format!("{context}, with Vulkan");
                    let selected = with_vulkan.request_adapter(&request, wgt::Backends::all());
                    assert_eq!(take_tried(), expected, "{context}: the adapters tried");
                    let selected = selected
                        .unwrap_or_else(|error| panic!("{context}: no adapter: {error}"))
                        .get_info();
                    let ranked = with_vulkan
                        .request_adapters(&request, wgt::Backends::all())
                        .unwrap_or_else(|error| panic!("{context}: {error}"));
                    let ranked_first = ranked[0].get_info();
                    take_tried();
                    println!(
                        "{context}: {} ({:?}, {:?})",
                        selected.name, selected.backend, selected.device_type
                    );
                    if ranked_first.backend == wgt::Backend::Vulkan {
                        assert_eq!(selected, ranked_first, "{context}");
                    } else if same_type {
                        // DX12's selection has the best rank, which no Vulkan adapter has.
                        assert_eq!(selected.backend, wgt::Backend::Dx12, "{context}");
                        assert_eq!(selected.device_type, ranked_first.device_type, "{context}");
                    }
                }
            }
        }
    }
}
