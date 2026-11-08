use crate::instances::{load_instance, save_instance, valid_id, Instance};
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};

const KINDS: [&str; 4] = ["resourcepacks", "shaderpacks", "saves", "screenshots"];

fn kind_dir(id: &str, kind: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(anyhow!("invalid instance id"));
    }
    if !KINDS.contains(&kind) {
        return Err(anyhow!("unknown folder"));
    }
    Ok(game_dir(id).join(kind))
}

fn safe_name(n: &str) -> Result<String> {
    Path::new(n)
        .file_name()
        .and_then(|s| s.to_str())
        .map(String::from)
        .filter(|s| !s.is_empty() && s != "." && s != "..")
        .ok_or_else(|| anyhow!("unsafe file name"))
}

fn dir_size(p: &Path) -> u64 {
    let mut t = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                if m.is_dir() {
                    t += dir_size(&e.path());
                } else {
                    t += m.len();
                }
            }
        }
    }
    t
}

pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let ty = e.file_type()?;
        let to = dst.join(e.file_name());
        if ty.is_dir() {
            copy_dir_all(&e.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[tauri::command]
pub fn folder_list(instance_id: String, kind: String) -> Result<Vec<Entry>, String> {
    let d = kind_dir(&instance_id, &kind).map_err(es)?;
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(&d) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(m) = e.metadata() else {
                continue;
            };
            let size = if m.is_dir() {
                if kind == "saves" {
                    dir_size(&e.path())
                } else {
                    0
                }
            } else {
                m.len()
            };
            out.push(Entry {
                name,
                is_dir: m.is_dir(),
                size,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

#[tauri::command]
pub fn folder_path(instance_id: String, kind: String) -> Result<String, String> {
    let d = kind_dir(&instance_id, &kind).map_err(es)?;
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d.display().to_string())
}

fn extract_zip(path: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let Some(rel) = e.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut o = std::fs::File::create(&out)?;
            std::io::copy(&mut e, &mut o)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn folder_add(instance_id: String, kind: String, path: String) -> Result<(), String> {
    let run = || -> Result<()> {
        let dir = kind_dir(&instance_id, &kind)?;
        std::fs::create_dir_all(&dir)?;
        let src = PathBuf::from(&path);
        let name = safe_name(src.file_name().and_then(|s| s.to_str()).unwrap_or(""))?;
        if kind == "saves" {
            if !name.to_lowercase().ends_with(".zip") {
                return Err(anyhow!("Add a world as a .zip file"));
            }
            let has_root_level = {
                let f = std::fs::File::open(&src)?;
                let z = zip::ZipArchive::new(f)?;
                let found = z.file_names().any(|n| n == "level.dat");
                found
            };
            let dest = if has_root_level {
                dir.join(name.trim_end_matches(".zip").trim_end_matches(".ZIP"))
            } else {
                dir.clone()
            };
            extract_zip(&src, &dest)
        } else {
            std::fs::copy(&src, dir.join(name))?;
            Ok(())
        }
    };
    run().map_err(es)
}

#[tauri::command]
pub fn folder_delete(instance_id: String, kind: String, name: String) -> Result<(), String> {
    let run = || -> Result<()> {
        let dir = kind_dir(&instance_id, &kind)?;
        let p = dir.join(safe_name(&name)?);
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
        Ok(())
    };
    run().map_err(es)
}

type Zw = zip::ZipWriter<std::fs::File>;

fn add_dir(
    zw: &mut Zw,
    base: &Path,
    dir: &Path,
    opts: zip::write::SimpleFileOptions,
    prefix: &str,
    skip_top: &[&str],
) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let rel = p.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
        let top = rel.split('/').next().unwrap_or("");
        if skip_top.contains(&top) {
            continue;
        }
        let ty = std::fs::metadata(&p)?.file_type();
        if ty.is_dir() {
            add_dir(zw, base, &p, opts.clone(), prefix, skip_top)?;
        } else if ty.is_file() {
            zw.start_file(format!("{prefix}{rel}"), opts.clone())?;
            let mut f = std::fs::File::open(&p)?;
            std::io::copy(&mut f, zw)?;
        }
    }
    Ok(())
}

fn file_opts() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)
}

#[tauri::command]
pub async fn world_backup(instance_id: String, name: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || -> Result<String> {
        let saves = kind_dir(&instance_id, "saves")?;
        let world = safe_name(&name)?;
        let src = saves.join(&world);
        if !src.is_dir() {
            return Err(anyhow!("World not found"));
        }
        let dir = instance_dir(&instance_id).join("backups");
        std::fs::create_dir_all(&dir)?;
        let out = dir.join(format!("{world}-{}.zip", now_secs()));
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&out)?);
        add_dir(&mut zw, &saves, &src, file_opts(), "", &[])?;
        zw.finish()?;
        Ok(out.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(es)
}

#[tauri::command]
pub async fn instance_duplicate(id: String) -> Result<Instance, String> {
    tokio::task::spawn_blocking(move || -> Result<Instance> {
        let src = load_instance(&id)?;
        if crate::launch::is_running(&id) {
            return Err(anyhow!("Stop the game first"));
        }
        let mut inst = src.clone();
        inst.id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        inst.name = format!("{} (copy)", src.name);
        inst.created = now_secs();
        inst.last_played = 0;
        copy_dir_all(&instance_dir(&src.id), &instance_dir(&inst.id))?;
        save_instance(&inst)?;
        let _ = crate::shared::apply(&inst, &crate::instances::load_settings());
        Ok(inst)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(es)
}

/// Exports the instance as a .mrpack whose files all live in `overrides/` (importable by Nimbus, Prism, Modrinth App…).
#[tauri::command]
pub async fn instance_export(id: String, path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<()> {
        let inst = load_instance(&id)?;
        let opts = file_opts();
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&path)?);
        let mut deps = serde_json::Map::new();
        deps.insert("minecraft".into(), json!(inst.mc_version));
        let key = match inst.loader.as_str() {
            "fabric" => Some("fabric-loader"),
            "quilt" => Some("quilt-loader"),
            "forge" => Some("forge"),
            "neoforge" => Some("neoforge"),
            _ => None,
        };
        if let Some(k) = key {
            deps.insert(k.into(), json!(inst.loader_version));
        }
        let index = json!({
            "formatVersion": 1,
            "game": "minecraft",
            "versionId": "1.0.0",
            "name": inst.name,
            "files": [],
            "dependencies": deps
        });
        zw.start_file("modrinth.index.json", opts.clone())?;
        zw.write_all(&serde_json::to_vec_pretty(&index)?)?;
        let base = game_dir(&inst.id);
        add_dir(&mut zw, &base, &base, opts, "overrides/", &["logs", "crash-reports", ".cache"])?;
        zw.finish()?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(es)
}

// ---------------- resource pack selection (options.txt) ----------------
fn mc_minor(v: &str) -> u32 {
    v.split('.')
        .nth(1)
        .and_then(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(99)
}

fn options_path(id: &str) -> PathBuf {
    game_dir(id).join("options.txt")
}

fn read_enabled(id: &str) -> Vec<String> {
    let Ok(txt) = std::fs::read_to_string(options_path(id)) else {
        return vec![];
    };
    for l in txt.lines() {
        if let Some(rest) = l.strip_prefix("resourcePacks:") {
            return serde_json::from_str::<Vec<String>>(rest.trim()).unwrap_or_default();
        }
    }
    vec![]
}

fn write_enabled(id: &str, packs: &[String]) -> Result<()> {
    let path = options_path(id);
    let txt = std::fs::read_to_string(&path).unwrap_or_default();
    let line = format!("resourcePacks:{}", serde_json::to_string(packs)?);
    let mut found = false;
    let mut out: Vec<String> = txt
        .lines()
        .map(|l| {
            if l.starts_with("resourcePacks:") {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&path, out.join("\n") + "\n")?;
    Ok(())
}

/// Entries currently enabled in options.txt ("file/pack.zip" on 1.13+, "pack.zip" before).
#[tauri::command]
pub fn packs_enabled(instance_id: String) -> Result<Vec<String>, String> {
    let inst = load_instance(&instance_id).map_err(es)?;
    Ok(read_enabled(&inst.id))
}

#[tauri::command]
pub fn pack_toggle(instance_id: String, name: String, enabled: bool) -> Result<(), String> {
    let run = || -> Result<()> {
        let inst = load_instance(&instance_id)?;
        if crate::launch::is_running(&inst.id) {
            return Err(anyhow!(
                "Close the game first — Minecraft rewrites its settings when it exits"
            ));
        }
        let name = safe_name(&name)?;
        let modern = mc_minor(&inst.mc_version) >= 13;
        let entry = if modern {
            format!("file/{name}")
        } else {
            name.clone()
        };
        let mut list = read_enabled(&inst.id);
        list.retain(|e| e != &entry);
        if enabled {
            if modern && !list.iter().any(|e| e == "vanilla") {
                list.insert(0, "vanilla".to_string());
            }
            list.push(entry);
        }
        write_enabled(&inst.id, &list)
    };
    run().map_err(es)
}
