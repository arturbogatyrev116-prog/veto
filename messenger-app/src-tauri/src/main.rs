#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "windows")]
mod shell32 {
    #[link(name = "Shell32")]
    extern "system" {
        pub fn SHChangeNotify(
            weventid: i32,
            uflags: u32,
            dwitem1: *const core::ffi::c_void,
            dwitem2: *const core::ffi::c_void,
        );
    }
}

#[cfg(target_os = "windows")]
fn refresh_icon_cache_if_updated() {
    const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
    let version_path = match std::env::var("APPDATA") {
        Ok(p) => std::path::PathBuf::from(p).join("Veto").join(".last_version"),
        Err(_) => return,
    };
    let last_version = std::fs::read_to_string(&version_path).unwrap_or_default();
    if last_version.trim() == CURRENT_VERSION {
        return;
    }
    // SHCNE_ASSOCCHANGED = 0x08000000, SHCNF_IDLIST = 0x0000
    unsafe {
        shell32::SHChangeNotify(0x08000000_i32, 0x0000_u32, std::ptr::null(), std::ptr::null());
    }
    if let Some(parent) = version_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&version_path, CURRENT_VERSION);
}

fn main() {
    #[cfg(target_os = "windows")]
    refresh_icon_cache_if_updated();

    messenger_app_lib::run()
}
