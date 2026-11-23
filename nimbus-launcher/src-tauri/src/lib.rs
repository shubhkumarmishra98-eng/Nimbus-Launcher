mod auth;
mod crash;
mod files;
mod gpu;
mod instances;
mod launch;
mod loaders;
mod mc;
mod migrate;
mod mods;
mod packs;
mod servers;
mod skins;
mod updater;
mod util;

use serde::Serialize;

#[derive(Serialize)]
struct SysInfo {
    total_mb: u64,
    os: String,
}

#[tauri::command]
fn system_info() -> SysInfo {
    let mut s = sysinfo::System::new();
    s.refresh_memory();
    SysInfo {
        total_mb: s.total_memory() / 1024 / 1024,
        os: std::env::consts::OS.to_string(),
    }
}

#[tauri::command]
fn data_dir() -> String {
    util::root().display().to_string()
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("invalid url".into());
    }
    #[cfg(windows)]
    let r = util::spawn_detached("rundll32", &["url.dll,FileProtocolHandler", url.as_str()]);
    #[cfg(target_os = "macos")]
    let r = util::spawn_detached("open", &[url.as_str()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = util::spawn_detached("xdg-open", &[url.as_str()]);
    r.map_err(util::es)
}

#[tauri::command]
fn open_path(path: String) -> Result<(), String> {
    let path = if cfg!(windows) { path.replace('/', "\\") } else { path };
    let _ = std::fs::create_dir_all(&path);
    #[cfg(windows)]
    let r = util::spawn_detached("explorer", &[path.as_str()]);
    #[cfg(target_os = "macos")]
    let r = util::spawn_detached("open", &[path.as_str()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = util::spawn_detached("xdg-open", &[path.as_str()]);
    r.map_err(util::es)
}

pub fn run() {
    updater::cleanup_old();
    let _ = util::ensure_dirs();
    migrate::unshare_all();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            system_info,
            data_dir,
            open_url,
            open_path,
            updater::app_version,
            updater::news_get,
            updater::update_check,
            updater::update_install,
            mc::list_versions,
            loaders::loader_versions,
            instances::settings_get,
            instances::settings_save,
            instances::instances_list,
            instances::instance_create,
            instances::instance_save,
            instances::instance_delete,
            instances::instance_retarget,
            instances::import_mrpack,
            packs::import_pack,
            packs::modpack_install,
            packs::content_install,
            launch::instance_install,
            launch::instance_launch,
            launch::instance_kill,
            launch::instance_running,
            launch::instance_repair,
            launch::instance_cancel,
            servers::servers_list,
            servers::servers_save,
            files::folder_list,
            files::folder_path,
            files::folder_add,
            files::folder_delete,
            files::packs_enabled,
            files::pack_toggle,
            files::options_preset,
            files::instance_open,
            files::world_backup,
            files::instance_duplicate,
            files::instance_export,
            crash::crash_analyze,
            crash::log_tail,
            crash::log_share,
            auth::accounts_list,
            auth::account_add_offline,
            auth::account_select,
            auth::account_remove,
            auth::ms_login_start,
            auth::ms_login_poll,
            auth::ms_login_cancel,
            auth::ms_available,
            skins::skin_profile,
            skins::skin_upload,
            skins::skin_image,
            skins::cape_set,
            skins::offline_skin_set,
            skins::offline_skin_get,
            mods::mods_list,
            mods::mods_info,
            mods::mods_search,
            mods::mods_install,
            mods::mods_toggle,
            mods::mods_delete,
            mods::mods_add_local,
            mods::mods_check_updates,
            mods::mods_apply_updates,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Nimbus Launcher");
}
