//! Shared "common" folders for mods and resource packs.
//!
//! Every instance's `mods/` and `resourcepacks/` folder is a link (junction on Windows, symlink elsewhere)
//! to one common folder, so a mod or pack added once shows up in every instance that uses sharing.
use crate::instances::{instances_list, load_settings, Instance, Settings};
use crate::util::*;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

fn folder_name(kind: &str) -> &'static str {
    if kind == "mods" {
        "mods"
    } else {
        "resourcepacks"
    }
}

fn valid_kind(kind: &str) -> Result<()> {
    if kind == "mods" || kind == "packs" {
        Ok(())
    } else {
        Err(anyhow!("unknown folder type"))
    }
}

pub fn default_dir(kind: &str) -> PathBuf {
    root().join("common").join(folder_name(kind))
}

/// The folder currently in use for `kind` ("mods" or "packs"): the custom one if set and usable, else the default.
pub fn active_dir(kind: &str, st: &Settings) -> PathBuf {
    let custom = if kind == "mods" { &st.mods_dir } else { &st.packs_dir };
    let c = custom.trim();
    if !c.is_empty() {
        let p = PathBuf::from(c);
        if p.is_dir() || std::fs::create_dir_all(&p).is_ok() {
            return p;
        }
    }
    default_dir(kind)
}

fn inst_path(id: &str, kind: &str) -> PathBuf {
    game_dir(id).join(folder_name(kind))
}

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn remove_link(p: &Path) -> Result<()> {
    #[cfg(windows)]
    std::fs::remove_dir(p)?;
    #[cfg(not(windows))]
    std::fs::remove_file(p)?;
    Ok(())
}

fn same_target(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn make_link(link: &Path, target: &Path) -> Result<()> {
    if let Some(p) = link.parent() {
        std::fs::create_dir_all(p)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // a junction needs no administrator rights, unlike a symbolic link
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .creation_flags(0x0800_0000)
            .output()?;
        if !out.status.success() {
            let mut msg = String::from_utf8_lossy(&out.stdout).to_string();
            msg.push_str(&String::from_utf8_lossy(&out.stderr));
            return Err(anyhow!("couldn't link the shared folder: {}", msg.trim()));
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

fn move_path(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if from.is_dir() {
        crate::files::copy_dir_all(from, to)?;
        std::fs::remove_dir_all(from)?;
    } else {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// Moves a real folder's contents into the shared folder, then frees the path for the link.
/// Anything that can't be moved (same name already exists) is kept in a "<name>.local-backup" folder.
fn migrate(dir: &Path, target: &Path) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let dest = target.join(e.file_name());
        if dest.exists() {
            continue;
        }
        move_path(&e.path(), &dest)?;
    }
    if std::fs::read_dir(dir)?.next().is_none() {
        std::fs::remove_dir(dir)?;
    } else {
        let base = dir.file_name().and_then(|n| n.to_str()).unwrap_or("folder");
        let mut bk = dir.with_file_name(format!("{base}.local-backup"));
        let mut n = 1;
        while bk.exists() {
            bk = dir.with_file_name(format!("{base}.local-backup-{n}"));
            n += 1;
        }
        std::fs::rename(dir, &bk)?;
    }
    Ok(())
}

/// Makes the instance's mods/resourcepacks folders match its "use shared folders" setting.
pub fn apply(inst: &Instance, st: &Settings) -> Result<()> {
    for kind in ["mods", "packs"] {
        let link = inst_path(&inst.id, kind);
        let target = active_dir(kind, st);
        std::fs::create_dir_all(&target)?;
        if inst.use_shared {
            if is_link(&link) {
                if same_target(&link, &target) {
                    continue;
                }
                remove_link(&link)?;
            } else if link.is_dir() {
                migrate(&link, &target)?;
            } else if link.exists() {
                continue;
            }
            make_link(&link, &target)?;
        } else if is_link(&link) {
            // sharing switched off: the instance keeps its own copy of what it was using
            let src = std::fs::canonicalize(&link).unwrap_or_else(|_| target.clone());
            remove_link(&link)?;
            crate::files::copy_dir_all(&src, &link)?;
        } else {
            std::fs::create_dir_all(&link)?;
        }
    }
    Ok(())
}

/// Removes the links (never the shared content) so deleting an instance can't touch shared files.
pub fn detach_all(id: &str) {
    for kind in ["mods", "packs"] {
        let p = inst_path(id, kind);
        if is_link(&p) {
            let _ = remove_link(&p);
        }
    }
}

fn relink_all(st: &Settings) -> Result<()> {
    let mut failed = 0;
    for inst in instances_list() {
        if crate::launch::is_running(&inst.id) {
            continue;
        }
        if apply(&inst, st).is_err() {
            failed += 1;
        }
    }
    if failed > 0 {
        return Err(anyhow!(
            "{failed} instance(s) couldn't be updated; they will be fixed the next time they start"
        ));
    }
    Ok(())
}

fn write_settings(st: &Settings) -> Result<()> {
    write_json(&root().join("settings.json"), st)
}

/// Chooses a custom shared folder ("mods" or "packs").
#[tauri::command]
pub fn shared_set(kind: String, path: String) -> Result<(), String> {
    let run = || -> Result<()> {
        valid_kind(&kind)?;
        let p = PathBuf::from(path.trim());
        if path.trim().is_empty() {
            return Err(anyhow!("Choose a folder"));
        }
        std::fs::create_dir_all(&p)?;
        let canon = std::fs::canonicalize(&p)?;
        if let Ok(inst_root) = std::fs::canonicalize(instances_dir()) {
            if canon.starts_with(&inst_root) {
                return Err(anyhow!("Pick a folder outside the launcher's instances folder"));
            }
        }
        let mut st = load_settings();
        let other = if kind == "mods" { &st.packs_dir } else { &st.mods_dir };
        if !other.trim().is_empty() {
            if let Ok(o) = std::fs::canonicalize(other.trim()) {
                if o == canon {
                    return Err(anyhow!("Mods and resource packs need two different folders"));
                }
            }
        }
        if kind == "mods" {
            st.mods_dir = p.display().to_string();
        } else {
            st.packs_dir = p.display().to_string();
        }
        write_settings(&st)?;
        relink_all(&st)
    };
    run().map_err(es)
}

/// Goes back to the default folder.
#[tauri::command]
pub fn shared_reset(kind: String) -> Result<(), String> {
    let run = || -> Result<()> {
        valid_kind(&kind)?;
        let mut st = load_settings();
        if kind == "mods" {
            st.mods_dir.clear();
        } else {
            st.packs_dir.clear();
        }
        write_settings(&st)?;
        relink_all(&st)
    };
    run().map_err(es)
}

/// Opens the default shared folder in the file manager.
#[tauri::command]
pub fn shared_open(kind: String) -> Result<(), String> {
    let run = || -> Result<()> {
        valid_kind(&kind)?;
        let d = default_dir(&kind);
        std::fs::create_dir_all(&d)?;
        let s = d.display().to_string();
        #[cfg(windows)]
        spawn_detached("explorer", &[s.as_str()])?;
        #[cfg(target_os = "macos")]
        spawn_detached("open", &[s.as_str()])?;
        #[cfg(all(unix, not(target_os = "macos")))]
        spawn_detached("xdg-open", &[s.as_str()])?;
        Ok(())
    };
    run().map_err(es)
}
