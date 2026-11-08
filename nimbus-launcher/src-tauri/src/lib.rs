mod auth;
mod crash;
mod files;
mod instances;
mod launch;
mod loaders;
mod mc;
mod mods;
mod packs;
mod servers;
mod shared;
mod skins;
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
    let _ = std::fs::create_dir_all(&path);
    #[cfg(windows)]
    let r = util::spawn_detached("explorer", &[path.as_str()]);
    #[cfg(target_os = "macos")]
    let r = util::spawn_detached("open", &[path.as_str()]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = util::spawn_detached("xdg-open", &[path.as_str()]);
    r.map_err(util::es)
}

#[derive(Serialize)]
struct LauncherUpdate {
    version: String,
    url: String,
}

/// Checks the latest GitHub release of `owner/repo` against this build's version.
#[tauri::command]
async fn launcher_update_check(repo: String) -> Result<Option<LauncherUpdate>, String> {
    let run = async {
        let ok = repo.matches('/').count() == 1
            && repo
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c));
        if !ok {
            return Err(anyhow::anyhow!("Use the form owner/repo"));
        }
        let r: serde_json::Value =
            util::get_json(&format!("https://api.github.com/repos/{repo}/releases/latest")).await?;
        let tag = r["tag_name"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('v')
            .to_string();
        let url = r["html_url"].as_str().unwrap_or("").to_string();
        let parse = |s: &str| -> Vec<u64> {
            s.split('.')
                .map(|p| {
                    p.chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0)
                })
                .collect()
        };
        if !tag.is_empty() && parse(&tag) > parse(env!("CARGO_PKG_VERSION")) {
            Ok(Some(LauncherUpdate { version: tag, url }))
        } else {
            Ok(None)
        }
    };
    run.await.map_err(util::es)
}

pub fn run() {
    let _ = util::ensure_dirs();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            system_info,
            data_dir,
            open_url,
            open_path,
            launcher_update_check,
            mc::list_versions,
            loaders::loader_versions,
            instances::settings_get,
            instances::settings_save,
            instances::instances_list,
            instances::instance_create,
            instances::instance_save,
            instances::instance_delete,
            instances::instance_retarget,
            shared::shared_set,
            shared::shared_reset,
            shared::shared_open,
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
            files::world_backup,
            files::instance_duplicate,
            files::instance_export,
            crash::crash_analyze,
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
