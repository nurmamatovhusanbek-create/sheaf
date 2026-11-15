//! Which renderer draws the windows. OpenGL is the normal one: it is what the transparent wheel
//! overlay was built on. On a machine where OpenGL is unusable (a remote desktop session, a
//! virtual machine, no graphics driver: Windows then offers only OpenGL 1.1) the window cannot
//! start, so the shell starts the program again with `SHEAF_RENDERER=wgpu`. That draws through
//! Direct3D 12, which Windows can also run in software (its "Basic Render Driver"), so the
//! window works everywhere. The fallback is chosen only after OpenGL has failed.

/// Set to `wgpu` to draw through Direct3D 12 instead of OpenGL (Windows only).
pub const RENDERER_ENV: &str = "SHEAF_RENDERER";

/// Did the window fail because OpenGL could not be used? Then drawing through the other
/// renderer is worth a try.
pub fn opengl_unusable(result: &eframe::Result) -> bool {
    match result {
        Ok(()) => false,
        Err(e) => {
            let gl = matches!(
                e,
                eframe::Error::OpenGL(_) | eframe::Error::Glutin(_) | eframe::Error::NoGlutinConfigs(..)
            );
            // Only a program that did not already choose the fallback may ask for it.
            gl && cfg!(windows) && std::env::var(RENDERER_ENV).map_or(true, |v| v != "wgpu")
        }
    }
}

/// Window options for the renderer in use.
/// `transparent` is for the wheel overlay, whose window has see-through corners.
pub(crate) fn options(viewport: eframe::egui::ViewportBuilder, transparent: bool) -> eframe::NativeOptions {
    #[cfg(windows)]
    if std::env::var(RENDERER_ENV).is_ok_and(|v| v == "wgpu") {
        return eframe::NativeOptions {
            viewport,
            renderer: eframe::Renderer::Wgpu,
            wgpu_options: software_capable(transparent),
            ..Default::default()
        };
    }
    let _ = transparent;
    eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    }
}

/// Direct3D 12 only, and an adapter chosen by what it is: a real graphics chip first, the
/// software renderer last. (Without this a machine whose only adapter is the software one
/// would be refused.)
#[cfg(windows)]
fn software_capable(transparent: bool) -> egui_wgpu::WgpuConfiguration {
    use egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    use std::sync::Arc;
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    if transparent {
        // A swapchain made straight from the window handle cannot be see-through; presenting through
        // a DirectComposition visual can, and the wheel overlay needs transparent corners.
        setup.instance_descriptor.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    }
    setup.native_adapter_selector = Some(Arc::new(|adapters, surface| {
        let rank = |t: wgpu::DeviceType| match t {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::VirtualGpu => 2,
            wgpu::DeviceType::Cpu => 3,
            wgpu::DeviceType::Other => 4,
        };
        adapters
            .iter()
            .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
            .min_by_key(|a| rank(a.get_info().device_type))
            .cloned()
            .ok_or_else(|| "no graphics adapter can draw to this window".to_string())
    }));
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}
