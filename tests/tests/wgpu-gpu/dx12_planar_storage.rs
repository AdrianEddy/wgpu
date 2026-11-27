use wgpu::*;
use wgpu_test::{
    apply, gpu_test, FailureCase, GpuTestConfiguration, GpuTestInitializer, TestParameters,
};

pub fn all_tests(vec: &mut Vec<GpuTestInitializer>) {
    vec.push(DX12_PLANAR_STORAGE_FOLLOWS_D3D12);
}

/// On DX12, NV12 and P010 report `STORAGE_BINDING` exactly where D3D12 reports typed UAV loads,
/// typed UAV stores or UAV atomic minimum and maximum for the format and for both of its plane
/// formats, read here from D3D12 itself (`CheckFeatureSupport` on the device). P010 on Intel
/// reports none: its chroma plane's views take the luma plane's extent, a driver defect that the
/// backend works around.
#[apply(gpu_test!)]
static DX12_PLANAR_STORAGE_FOLLOWS_D3D12: GpuTestConfiguration = GpuTestConfiguration::new()
    .parameters(
        TestParameters::default()
            .features(
                Features::TEXTURE_FORMAT_NV12
                    | Features::TEXTURE_FORMAT_P010
                    | Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
            )
            .skip(FailureCase::backend(Backends::all() - Backends::DX12)),
    )
    .run_sync(|ctx| match ctx.adapter_info.backend {
        #[cfg(windows)]
        Backend::Dx12 => windows_impl::storage_follows_d3d12(&ctx),
        other => unreachable!(
            "test is configured to skip all backends except DX12, but ran on {other:?}"
        ),
    });

#[cfg(windows)]
mod windows_impl {
    use wgpu::hal::dx12;
    use windows::Win32::Graphics::{Direct3D12, Dxgi::Common};

    /// The vendor id of Intel's adapters, on which P010 keeps no storage.
    const INTEL: u32 = 0x8086;

    /// D3D12's UAV support for `format`: typed loads, typed stores, and atomic minimum and maximum,
    /// the three that the backend maps to storage.
    fn uav(device: &Direct3D12::ID3D12Device, format: Common::DXGI_FORMAT) -> [bool; 3] {
        let mut data = Direct3D12::D3D12_FEATURE_DATA_FORMAT_SUPPORT {
            Format: format,
            ..Default::default()
        };
        // SAFETY: `data` is the structure that `D3D12_FEATURE_FORMAT_SUPPORT` takes, with its size.
        unsafe {
            device.CheckFeatureSupport(
                Direct3D12::D3D12_FEATURE_FORMAT_SUPPORT,
                <*mut _>::cast(&mut data),
                size_of_val(&data) as u32,
            )
        }
        .unwrap();
        [
            Direct3D12::D3D12_FORMAT_SUPPORT2_UAV_TYPED_LOAD,
            Direct3D12::D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE,
            Direct3D12::D3D12_FORMAT_SUPPORT2_UAV_ATOMIC_UNSIGNED_MIN_OR_MAX,
        ]
        .map(|support| data.Support2.contains(support))
    }

    pub fn storage_follows_d3d12(ctx: &wgpu_test::TestingContext) {
        // SAFETY: The device is only queried.
        let hal_device = unsafe { ctx.device.as_hal::<dx12::Api>() }.unwrap();
        let device = hal_device.raw_device();
        let cases = [
            (
                wgpu::TextureFormat::NV12,
                Common::DXGI_FORMAT_NV12,
                [Common::DXGI_FORMAT_R8_UNORM, Common::DXGI_FORMAT_R8G8_UNORM],
            ),
            (
                wgpu::TextureFormat::P010,
                Common::DXGI_FORMAT_P010,
                [
                    Common::DXGI_FORMAT_R16_UNORM,
                    Common::DXGI_FORMAT_R16G16_UNORM,
                ],
            ),
        ];
        for (format, dxgi, planes) in cases {
            let whole = uav(device, dxgi);
            let [plane0, plane1] = planes.map(|plane| uav(device, plane));
            let expected = (0..3).any(|i| whole[i] && plane0[i] && plane1[i])
                && !(format == wgpu::TextureFormat::P010 && ctx.adapter_info.vendor == INTEL);
            let reported = ctx
                .adapter
                .get_texture_format_features(format)
                .allowed_usages
                .contains(wgpu::TextureUsages::STORAGE_BINDING);
            log::info!(
                "{format:?}: D3D12 UAV typed load, typed store, atomic min/max {whole:?}, planes \
                 {plane0:?} {plane1:?}; storage expected {expected}, reported {reported}"
            );
            assert_eq!(
                reported, expected,
                "{format:?}'s STORAGE_BINDING against D3D12's UAV support"
            );
        }
    }
}
