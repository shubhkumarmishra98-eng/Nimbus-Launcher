use crate::instances::{self, load_settings, Instance, NewInstance};
use crate::mods;
use crate::util::*;
use anyhow::{anyhow, Result};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use tauri::{AppHandle, Emitter};

const CF: &str = "https://api.curseforge.com/v1";

fn file_name_only(n: &str) -> Result<String> {
    Path::new(n)
        .file_name()
        .and_then(|s| s.to_str())
        .map(String::from)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("unsafe file name"))
}

/// Imports a Modrinth `.mrpack` or a CurseForge modpack `.zip`.
#[tauri::command]
pub async fn import_pack(app: AppHandle, path: String) -> Result<Instance, String> {
    run_import(app, path).await.map_err(es)
}

async fn run_import(app: AppHandle, path: String) -> Result<Instance> {
    let kind = {
        let f = std::fs::File::open(&path)?;
        let z = zip::ZipArchive::new(f)?;
        let has = |n: &str| z.file_names().any(|x| x == n);
        if has("modrinth.index.json") {
            "modrinth"
        } else if has("manifest.json") {
            "curseforge"
        } else {
            "unknown"
        }
    };
    match kind {
        "modrinth" => instances::import_inner(app, path).await,
        "curseforge" => import_curseforge(app, path).await,
        _ => Err(anyhow!(
            "Unsupported modpack (expected a Modrinth .mrpack or a CurseForge .zip)"
        )),
    }
}

async fn import_curseforge(app: AppHandle, path: String) -> Result<Instance> {
    let man: Value = {
        let f = std::fs::File::open(&path)?;
        let mut z = zip::ZipArchive::new(f)?;
        let mut e = z.by_name("manifest.json")?;
        let mut s = String::new();
        e.read_to_string(&mut s)?;
        let v: Value = serde_json::from_str(&s)?;
        v
    };
    let mc = man["minecraft"]["version"]
        .as_str()
        .ok_or_else(|| anyhow!("manifest has no Minecraft version"))?
        .to_string();
    let primary = man["minecraft"]["modLoaders"]
        .as_array()
        .and_then(|a| a.iter().find(|m| m["primary"] == true).or(a.first()))
        .and_then(|m| m["id"].as_str())
        .unwrap_or("")
        .to_string();
    let (loader, lv) = match primary.split_once('-') {
        Some((l, v)) if ["forge", "fabric", "neoforge", "quilt"].contains(&l) => {
            (l.to_string(), v.to_string())
        }
        _ => ("vanilla".to_string(), String::new()),
    };
    let key = mods::cf_key()?;
    let st = load_settings();
    let inst = instances::create(NewInstance {
        name: man["name"].as_str().unwrap_or("CurseForge pack").to_string(),
        mc_version: mc,
        loader,
        loader_version: lv,
        memory_mb: st.default_memory_mb.max(4096),
        perf_pack: false,
    })?;
    let gdir = game_dir(&inst.id);
    progress(&app, &inst.id, "Resolving modpack files", 0, 0);

    let ids: Vec<u64> = man["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["fileID"].as_u64())
        .collect();
    let mut jobs: Vec<Job> = vec![];
    for chunk in ids.chunks(200) {
        let r: Value = http()
            .post(format!("{CF}/mods/files"))
            .header("x-api-key", key.clone())
            .json(&json!({ "fileIds": chunk }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        for f in r["data"].as_array().into_iter().flatten() {
            let Ok(name) = file_name_only(f["fileName"].as_str().unwrap_or("")) else {
                continue;
            };
            let fid = f["id"].as_u64().unwrap_or(0);
            let url = match f["downloadUrl"].as_str() {
                Some(u) if !u.is_empty() => u.to_string(),
                _ => format!(
                    "https://edge.forgecdn.net/files/{}/{}/{}",
                    fid / 1000,
                    fid % 1000,
                    name
                ),
            };
            let sub = if name.to_lowercase().ends_with(".jar") {
                "mods"
            } else {
                "resourcepacks"
            };
            let sha1 = f["hashes"]
                .as_array()
                .and_then(|h| h.iter().find(|x| x["algo"] == 1))
                .and_then(|x| x["value"].as_str())
                .map(String::from);
            jobs.push(Job {
                url,
                dest: gdir.join(sub).join(&name),
                sha1,
                size: None,
            });
        }
    }

    progress(&app, &inst.id, "Downloading modpack files", 0, 0);
    let results = stream::iter(jobs)
        .map(|j| async move {
            let r = download_file(&j.url, &j.dest, j.sha1.as_deref(), None).await;
            let n = j
                .dest
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            (n, r.is_ok())
        })
        .buffer_unordered(st.concurrency.max(4))
        .collect::<Vec<_>>()
        .await;
    let failed: Vec<String> = results
        .into_iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| n)
        .collect();

    let ov = man["overrides"].as_str().unwrap_or("overrides").to_string();
    {
        let f = std::fs::File::open(&path)?;
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            let Some(enc) = e.enclosed_name() else {
                continue;
            };
            let Ok(rel) = enc.strip_prefix(&ov) else {
                continue;
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            let out = gdir.join(rel);
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
    }
    progress(&app, &inst.id, "Ready", 1, 1);
    if !failed.is_empty() {
        let _ = app.emit(
            "notice",
            format!(
                "{} file(s) couldn't be downloaded automatically (their authors block third-party launchers). Download them from CurseForge and drop them into the mods folder: {}",
                failed.len(),
                failed.join(", ")
            ),
        );
    }
    Ok(inst)
}

/// Downloads the newest version of a Modrinth modpack and imports it as a new instance.
#[tauri::command]
pub async fn modpack_install(app: AppHandle, project_id: String) -> Result<Instance, String> {
    let run = async {
        let ver = mods::pick_version(&project_id, &[], "").await?;
        let files = ver["files"].as_array().cloned().unwrap_or_default();
        let f = files
            .iter()
            .find(|f| f["primary"] == true)
            .or(files.first())
            .ok_or_else(|| anyhow!("modpack has no files"))?;
        let url = f["url"]
            .as_str()
            .ok_or_else(|| anyhow!("modpack has no download url"))?;
        let name = file_name_only(f["filename"].as_str().unwrap_or("pack.mrpack"))?;
        let dest = cache_dir().join("modpacks").join(name);
        download_file(url, &dest, f["hashes"]["sha1"].as_str(), f["size"].as_u64()).await?;
        instances::import_inner(app, dest.display().to_string()).await
    };
    run.await.map_err(es)
}

/// Installs a Modrinth resource pack or shader pack into an instance.
#[tauri::command]
pub async fn content_install(
    instance_id: String,
    project_id: String,
    kind: String,
) -> Result<String, String> {
    let run = async {
        let inst = instances::load_instance(&instance_id)?;
        let sub = match kind.as_str() {
            "resourcepack" => "resourcepacks",
            "shader" => "shaderpacks",
            _ => return Err(anyhow!("unsupported content type")),
        };
        let ver = mods::pick_version(&project_id, &[], &inst.mc_version).await?;
        let files = ver["files"].as_array().cloned().unwrap_or_default();
        let f = files
            .iter()
            .find(|f| f["primary"] == true)
            .or(files.first())
            .ok_or_else(|| anyhow!("no files"))?;
        let url = f["url"].as_str().ok_or_else(|| anyhow!("no download url"))?;
        let name = file_name_only(f["filename"].as_str().unwrap_or("download.zip"))?;
        let dir = game_dir(&inst.id).join(sub);
        download_file(
            url,
            &dir.join(&name),
            f["hashes"]["sha1"].as_str(),
            f["size"].as_u64(),
        )
        .await?;
        Ok::<String, anyhow::Error>(name)
    };
    run.await.map_err(es)
}
