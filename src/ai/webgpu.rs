//! The runtime's WebGPU plugin: found beside the runtime library and registered when the runtime loads.
//!
//! WebGPU ships as a plugin rather than being built into the runtime, so it is registered with
//! `RegisterExecutionProviderLibrary` and attached through the devices it offers, never by name. Nothing here is an
//! error: a machine without WebGPU, a missing plugin, a refused registration or no device each leaves WebGPU unoffered,
//! with the reason logged, and the load goes on.

use std::path::{Path, PathBuf};

use ort::device::Device;
use ort::environment::Environment;

/// The name the plugin's devices report as their execution provider.
pub(super) const WEBGPU_EP: &str = "WebGpuExecutionProvider";

/// The WebGPU plugin in `dir` for `os`, as `std::env::consts::OS` spells it: `libonnxruntime_providers_webgpu.dylib`
/// on macOS, `.so` on Linux and `onnxruntime_providers_webgpu.dll` on Windows. `None` when it is not there.
pub(super) fn plugin_in(dir: &Path, os: &str) -> Option<PathBuf> {
    let name = match os {
        "macos" => "libonnxruntime_providers_webgpu.dylib",
        "windows" => "onnxruntime_providers_webgpu.dll",
        _ => "libonnxruntime_providers_webgpu.so",
    };
    let path = dir.join(name);
    path.is_file().then_some(path)
}

/// Registers the plugin found in `dir`, the runtime's folder, with `environment`, and says whether it offers a device.
pub(super) fn register(environment: &Environment, dir: &Path) -> bool {
    // Before the support check, so a machine whose Vulkan lists only a CPU rasteriser says so too. Empty off Linux.
    for (index, device) in crate::sysinfo::vulkan_devices().iter().enumerate() {
        let (major, minor, patch) = device.api_version;
        tracing::info!(
            index,
            adapter = %device.name,
            kind = %device.device_type,
            vulkan = %format_args!("{major}.{minor}.{patch}"),
            vram = device.device_local_memory.unwrap_or(0),
            "Vulkan device"
        );
    }

    if !crate::sysinfo::is_webgpu_supported() {
        let reason = if cfg!(target_os = "linux") {
            "no Vulkan GPU driver"
        } else {
            "not available on this platform"
        };
        tracing::info!("WebGPU is not offered: {reason}");
        return false;
    }

    let Some(plugin) = plugin_in(dir, std::env::consts::OS) else {
        tracing::info!(dir = %dir.display(), "WebGPU is not offered: the runtime's folder has no WebGPU plugin");
        return false;
    };

    // The registration lasts for the process: nothing here unregisters it.
    if let Err(error) = environment.register_ep_library("WebGPU", &plugin) {
        tracing::warn!(plugin = %plugin.display(), %error, "WebGPU is not offered: the plugin was refused");
        return false;
    }

    let count = devices(environment).count();
    if count == 0 {
        tracing::info!(plugin = %plugin.display(), "WebGPU is not offered: the plugin found no device");
        return false;
    }
    tracing::info!(plugin = %plugin.display(), devices = count, "WebGPU plugin registered");
    true
}

/// The devices the WebGPU plugin registered with `environment`, in the runtime's order.
pub(super) fn devices(environment: &Environment) -> impl Iterator<Item = Device<'_>> {
    environment
        .devices()
        .filter(|device| device.ep().is_ok_and(|ep| ep == WEBGPU_EP))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_platforms_plugin_is_found_beside_the_runtime() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-webgpu").unwrap();
        assert_eq!(plugin_in(dir.path(), "macos"), None, "an empty folder");
        assert_eq!(plugin_in(dir.path(), "linux"), None);
        assert_eq!(plugin_in(dir.path(), "windows"), None);

        for (os, name) in [
            ("macos", "libonnxruntime_providers_webgpu.dylib"),
            ("linux", "libonnxruntime_providers_webgpu.so"),
            ("windows", "onnxruntime_providers_webgpu.dll"),
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
            assert_eq!(plugin_in(dir.path(), os), Some(dir.path().join(name)), "{os}");
        }
    }

    #[test]
    fn another_platforms_plugin_is_not_taken() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-webgpu").unwrap();
        std::fs::write(dir.path().join("libonnxruntime_providers_webgpu.dylib"), b"x").unwrap();
        assert_eq!(plugin_in(dir.path(), "linux"), None);
        assert_eq!(plugin_in(dir.path(), "windows"), None);
    }

    #[test]
    fn a_folder_named_like_the_plugin_is_not_it() {
        let dir = crate::fs::mk_temp_dir("rust-sak-ai-webgpu").unwrap();
        std::fs::create_dir(dir.path().join("libonnxruntime_providers_webgpu.dylib")).unwrap();
        assert_eq!(plugin_in(dir.path(), "macos"), None);
    }
}
