//! Windows: ask Windows to run Java on the high-performance GPU.
//!
//! Laptops with two graphics chips often run Minecraft on the weak integrated one. Windows keeps a
//! per-program preference (Settings > System > Display > Graphics); we set it for the Java we launch.
//! It is stored under the current user, needs no administrator rights, and can be changed in Windows
//! Settings at any time.
use std::path::Path;

#[cfg(windows)]
pub fn prefer_high_performance(java: &Path) {
    use std::os::windows::process::CommandExt;
    let Some(bin) = java.parent() else {
        return;
    };
    let marker = bin.join(".gpu-pref");
    if marker.exists() {
        return;
    }
    let mut ok = true;
    for exe in [java.to_path_buf(), bin.join("javaw.exe")] {
        let res = std::process::Command::new("reg")
            .args(["add", "HKCU\\Software\\Microsoft\\DirectX\\UserGpuPreferences", "/v"])
            .arg(&exe)
            .args(["/t", "REG_SZ", "/d", "GpuPreference=2;", "/f"])
            .creation_flags(0x0800_0000)
            .output();
        ok &= res.map(|o| o.status.success()).unwrap_or(false);
    }
    if ok {
        let _ = std::fs::write(marker, "ok");
    }
}

#[cfg(not(windows))]
pub fn prefer_high_performance(_java: &Path) {}
