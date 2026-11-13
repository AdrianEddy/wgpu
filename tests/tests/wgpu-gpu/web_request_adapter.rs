//! The WebGPU backend when the browser has no adapter for it: `Instance::request_adapter` reports
//! `RequestAdapterError::NotFound`, and `util::new_instance_with_webgpu_detection` falls back to
//! WebGL.
//!
//! The failures are made deterministic by replacing `navigator.gpu.requestAdapter` (with a promise
//! of `null`, or a rejected promise) or hiding `navigator.gpu`. Each test runs in a page of its
//! own, restores what it replaced, and checks that wgpu called the replacement: a browser without a
//! WebGPU adapter resolves `null` itself.
#![cfg(wasm_test)]

use wgpu_test::{apply, gpu_test, GpuTestConfiguration, GpuTestInitializer, TestParameters};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.extend([
        REQUEST_ADAPTER_RESOLVING_NULL,
        REQUEST_ADAPTER_REJECTING,
        NAVIGATOR_GPU_MISSING,
        WEBGPU_DETECTION_FALLS_BACK_TO_WEBGL,
    ]);
}

/// `requestAdapter` resolving `null` is the WebGPU backend finding no adapter.
#[apply(gpu_test!)]
static REQUEST_ADAPTER_RESOLVING_NULL: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default())
    .run_async(|_ctx| async move {
        #[cfg(target_arch = "wasm32")]
        {
            web::request_adapter_resolves_null();
            assert_eq!(web::webgpu_request_error().await, web::no_webgpu_adapter());
            assert_eq!(web::replacement_calls(), 1);
        }
    });

/// `requestAdapter` rejecting is the WebGPU backend finding no adapter too.
#[apply(gpu_test!)]
static REQUEST_ADAPTER_REJECTING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default())
    .run_async(|_ctx| async move {
        #[cfg(target_arch = "wasm32")]
        {
            web::request_adapter_rejects();
            assert_eq!(web::webgpu_request_error().await, web::no_webgpu_adapter());
            assert_eq!(web::replacement_calls(), 1);
        }
    });

/// Without `navigator.gpu`, `Instance::new` does not use the WebGPU backend, so the instance has
/// no active backend: `BROWSER_WEBGPU` was the only one requested.
#[apply(gpu_test!)]
static NAVIGATOR_GPU_MISSING: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default())
    .run_async(|_ctx| async move {
        #[cfg(target_arch = "wasm32")]
        {
            web::hide_navigator_gpu();
            assert_eq!(
                web::webgpu_request_error().await,
                wgpu::RequestAdapterError::NotFound {
                    active_backends: wgpu::Backends::empty(),
                    requested_backends: wgpu::Backends::BROWSER_WEBGPU,
                    // wgpu-core's backends, which never include WebGPU.
                    supported_backends: wgpu::Instance::enabled_backend_features()
                        - wgpu::Backends::BROWSER_WEBGPU,
                    no_fallback_backends: wgpu::Backends::empty(),
                    no_adapter_backends: wgpu::Backends::empty(),
                    incompatible_surface_backends: wgpu::Backends::empty(),
                }
            );
        }
    });

/// When `requestAdapter` gives no adapter, an instance made with WebGPU detection leaves WebGPU
/// out, and requesting an adapter for a canvas gives a WebGL one.
#[apply(gpu_test!)]
static WEBGPU_DETECTION_FALLS_BACK_TO_WEBGL: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(TestParameters::default())
    .run_async(|_ctx| async move {
        #[cfg(target_arch = "wasm32")]
        {
            web::request_adapter_resolves_null();
            let instance =
                wgpu::util::new_instance_with_webgpu_detection(wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
                    display: Some(Box::new(wgpu_test::WebDisplayHandle)),
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                })
                .await;
            web::restore_navigator_gpu();
            // The detection's own `requestAdapter`.
            assert_eq!(web::replacement_calls(), 1);

            let surface = instance
                .create_surface(wgpu::SurfaceTarget::Canvas(
                    wgpu_test::initialize_html_canvas(),
                ))
                .expect("could not create a surface from a canvas");
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                })
                .await
                .expect("no adapter for the canvas");
            assert_eq!(adapter.get_info().backend, wgpu::Backend::Gl);
        }
    });

/// The page's `navigator.gpu`, replaced and restored through `js_sys`. Not with `inline_js`:
/// wasm-bindgen names a crate's snippets `snippets/wgpu-test-<hash>/inlineN.js`, the hash being
/// of the package's name and version, so this binary's snippet (`inline0.js`) and the
/// `wgpu-test` library's (`inline1.js`) share a directory with the snippets of every binary that
/// links the library. `cargo xtask test-wasm` then binds `wgpu-examples` into the same
/// `tests/wasm/dist`, whose `inline0.js`, the library's snippet, overwrites this binary's, and the
/// `wgpu-test` module no longer links.
#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use js_sys::{Error, Object, Promise, Reflect};
    use wasm_bindgen::{closure::Closure, JsCast as _, JsValue};

    /// The calls of the `requestAdapter` that [`replace_request_adapter`] installed last.
    static CALLS: AtomicUsize = AtomicUsize::new(0);

    pub fn replacement_calls() -> usize {
        CALLS.load(Ordering::Relaxed)
    }

    fn navigator() -> JsValue {
        Reflect::get(&js_sys::global(), &"navigator".into()).expect("no `navigator`")
    }

    /// Defines `object[key]` as an own, configurable property, which shadows its prototype's.
    fn define(object: &JsValue, key: &str, value: &JsValue) {
        let descriptor = Object::new();
        Reflect::set(&descriptor, &"value".into(), value).unwrap();
        Reflect::set(&descriptor, &"configurable".into(), &JsValue::TRUE).unwrap();
        Object::define_property(object.unchecked_ref::<Object>(), &key.into(), &descriptor);
    }

    /// Makes `navigator.gpu.requestAdapter` return `request_adapter()`. A browser without WebGPU
    /// is given a `navigator.gpu` first.
    fn replace_request_adapter(request_adapter: fn() -> Promise) {
        let navigator = navigator();
        let mut gpu = Reflect::get(&navigator, &"gpu".into()).unwrap();
        if gpu.is_undefined() {
            gpu = Object::new().into();
            define(&navigator, "gpu", &gpu);
        }
        CALLS.store(0, Ordering::Relaxed);
        let request_adapter = Closure::<dyn Fn() -> Promise>::new(move || {
            CALLS.fetch_add(1, Ordering::Relaxed);
            request_adapter()
        })
        .into_js_value();
        define(&gpu, "requestAdapter", &request_adapter);
    }

    pub fn request_adapter_resolves_null() {
        replace_request_adapter(|| Promise::resolve(&JsValue::NULL));
    }

    pub fn request_adapter_rejects() {
        replace_request_adapter(|| Promise::reject(&Error::new("requestAdapter failed").into()));
    }

    pub fn hide_navigator_gpu() {
        define(&navigator(), "gpu", &JsValue::UNDEFINED);
    }

    /// Removes the own properties defined above, which shadow `Navigator.prototype.gpu` and
    /// `GPU.prototype.requestAdapter`.
    pub fn restore_navigator_gpu() {
        let navigator = navigator();
        Reflect::delete_property(navigator.unchecked_ref::<Object>(), &"gpu".into()).unwrap();
        let gpu = Reflect::get(&navigator, &"gpu".into()).unwrap();
        if !gpu.is_undefined() {
            Reflect::delete_property(gpu.unchecked_ref::<Object>(), &"requestAdapter".into())
                .unwrap();
        }
    }

    /// The error of requesting an adapter from an instance that requests only WebGPU, after which
    /// `navigator.gpu` is restored.
    pub async fn webgpu_request_error() -> wgpu::RequestAdapterError {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let result = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await;
        restore_navigator_gpu();
        match result {
            Err(error) => error,
            Ok(adapter) => panic!("expected no adapter, got {:?}", adapter.get_info()),
        }
    }

    /// The WebGPU backend's error when `requestAdapter` gives no adapter.
    pub fn no_webgpu_adapter() -> wgpu::RequestAdapterError {
        wgpu::RequestAdapterError::NotFound {
            active_backends: wgpu::Backends::BROWSER_WEBGPU,
            requested_backends: wgpu::Backends::BROWSER_WEBGPU,
            supported_backends: wgpu::Backends::BROWSER_WEBGPU,
            no_fallback_backends: wgpu::Backends::empty(),
            no_adapter_backends: wgpu::Backends::BROWSER_WEBGPU,
            incompatible_surface_backends: wgpu::Backends::empty(),
        }
    }
}
