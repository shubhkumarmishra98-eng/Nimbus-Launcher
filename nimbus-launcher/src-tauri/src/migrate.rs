//! One-time cleanup. An earlier build linked each instance's `mods` and `resourcepacks` folders to a
//! common folder. Everything now lives inside the instance, so any such link is replaced by a real
//! folder holding a copy of what the instance was using. Nothing is deleted from the old common folder.
use crate::instances::instances_list;
use crate::util::*;
use std::path::Path;

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

fn remove_link(p: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    return std::fs::remove_dir(p);
    #[cfg(not(windows))]
    return std::fs::remove_file(p);
}

pub fn unshare_instance(id: &str) {
    for sub in ["mods", "resourcepacks"] {
        let p = game_dir(id).join(sub);
        if !is_link(&p) {
            continue;
        }
        let target = std::fs::canonicalize(&p).ok();
        if remove_link(&p).is_err() {
            continue;
        }
        match target {
            Some(t) => {
                let _ = crate::files::copy_dir_all(&t, &p);
            }
            None => {
                let _ = std::fs::create_dir_all(&p);
            }
        }
    }
}

pub fn unshare_all() {
    for inst in instances_list() {
        unshare_instance(&inst.id);
    }
}
