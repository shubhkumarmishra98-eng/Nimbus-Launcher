use crate::instances::{load_instance, load_settings, Instance};
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MR: &str = "https://api.modrinth.com/v2";
const CF: &str = "https://api.curseforge.com/v1";

fn mods_dir(id: &str) -> PathBuf {
    game_dir(id).join("mods")
}
fn index_path(id: &str) -> PathBuf {
    // lives inside the mods folder, so instances sharing that folder also share what is known about it
    mods_dir(id).join(".nimbus-index.json")
}
fn load_index(id: &str) -> HashMap<String, String> {
    read_json(&index_path(id)).unwrap_or_default()
}
fn save_index(id: &str, i: &HashMap<String, String>) {
    let _ = write_json(&index_path(id), i);
}
fn loaders_for(l: &str) -> Vec<String> {
    match l {
        "quilt" => vec!["quilt".to_string(), "fabric".to_string()],
        o => vec![o.to_string()],
    }
}
fn jarr(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string())
}
fn safe_name(n: &str) -> Result<String> {
    Path::new(n)
        .file_name()
        .and_then(|s| s.to_str())
        .map(String::from)
        .filter(|s| !s.is_empty() && s != "." && s != "..")
        .ok_or_else(|| anyhow!("unsafe file name"))
}
fn rm_both(dir: &Path, base: &str) {
    let _ = std::fs::remove_file(dir.join(base));
    let _ = std::fs::remove_file(dir.join(format!("{base}.disabled")));
}

// ---------------- types ----------------
#[derive(Serialize)]
pub struct ModHit {
    pub id: String,
    pub title: String,
    pub description: String,
    pub icon: Option<String>,
    pub downloads: u64,
    pub author: String,
    pub source: String,
}
#[derive(Serialize)]
pub struct InstalledMod {
    pub file: String,
    pub enabled: bool,
    pub size: u64,
}
#[derive(Serialize)]
pub struct UpdateInfo {
    pub file: String,
    pub enabled: bool,
    pub project_id: String,
    pub title: String,
    pub latest: String,
    pub new_file: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

// ---------------- Modrinth ----------------
pub async fn pick_version(project: &str, loaders: &[String], mc: &str) -> Result<Value> {
    let mut q: Vec<(&str, String)> = vec![];
    if !loaders.is_empty() {
        q.push(("loaders", jarr(loaders)));
    }
    if !mc.is_empty() {
        q.push(("game_versions", jarr(&[mc.to_string()])));
    }
    let list: Vec<Value> = http()
        .get(format!("{MR}/project/{project}/version"))
        .query(&q)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    list.iter()
        .find(|v| v["version_type"] == "release")
        .or_else(|| list.first())
        .cloned()
        .ok_or_else(|| anyhow!("No compatible version for Minecraft {mc}"))
}

/// Installs a project (slug or id) plus its required dependencies. Returns file names.
pub async fn install_modrinth(inst: &Instance, project: &str) -> Result<Vec<String>> {
    let dir = mods_dir(&inst.id);
    tokio::fs::create_dir_all(&dir).await?;
    let loaders = loaders_for(&inst.loader);
    let mut index = load_index(&inst.id);
    let mut queue = vec![project.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    let mut done: Vec<String> = vec![];
    let mut failure: Option<anyhow::Error> = None;

    while let Some(p) = queue.pop() {
        if !seen.insert(p.clone()) {
            continue;
        }
        let is_root = p == project;
        let ver = match pick_version(&p, &loaders, &inst.mc_version).await {
            Ok(v) => v,
            Err(e) => {
                if is_root {
                    failure = Some(e);
                    break;
                }
                continue;
            }
        };
        let real_id = ver["project_id"].as_str().unwrap_or(&p).to_string();
        if !is_root
            && index.iter().any(|(f, pid)| {
                *pid == real_id
                    && (dir.join(f).exists() || dir.join(format!("{f}.disabled")).exists())
            })
        {
            continue;
        }
        let files = ver["files"].as_array().cloned().unwrap_or_default();
        let Some(f) = files.iter().find(|f| f["primary"] == true).or(files.first()) else {
            continue;
        };
        let name = match f["filename"].as_str().map(safe_name) {
            Some(Ok(n)) => n,
            _ => continue,
        };
        let Some(url) = f["url"].as_str() else {
            continue;
        };
        // replace an older version of the same project
        let olds: Vec<String> = index
            .iter()
            .filter(|(file, pid)| **pid == real_id && **file != name)
            .map(|(file, _)| file.clone())
            .collect();
        for o in olds {
            rm_both(&dir, &o);
            index.remove(&o);
        }
        if let Err(e) = download_file(
            url,
            &dir.join(&name),
            f["hashes"]["sha1"].as_str(),
            f["size"].as_u64(),
        )
        .await
        {
            failure = Some(e);
            break;
        }
        index.insert(name.clone(), real_id);
        done.push(name);
        for d in ver["dependencies"].as_array().into_iter().flatten() {
            if d["dependency_type"] == "required" {
                if let Some(pid) = d["project_id"].as_str() {
                    queue.push(pid.to_string());
                }
            }
        }
    }
    save_index(&inst.id, &index);
    match failure {
        Some(e) => Err(e),
        None => Ok(done),
    }
}

async fn search_modrinth(
    q: &str,
    mc: &str,
    loader: &str,
    offset: u32,
    kind: &str,
) -> Result<Vec<ModHit>> {
    let mut facets: Vec<Vec<String>> = vec![vec![format!("project_type:{kind}")]];
    if !mc.is_empty() {
        facets.push(vec![format!("versions:{mc}")]);
    }
    if kind == "mod" && !loader.is_empty() && loader != "vanilla" {
        facets.push(
            loaders_for(loader)
                .into_iter()
                .map(|l| format!("categories:{l}"))
                .collect(),
        );
    }
    let index = if q.is_empty() { "downloads" } else { "relevance" };
    let r: Value = http()
        .get(format!("{MR}/search"))
        .query(&[
            ("query", q.to_string()),
            ("facets", serde_json::to_string(&facets)?),
            ("limit", "20".to_string()),
            ("offset", offset.to_string()),
            ("index", index.to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(r["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| ModHit {
            id: h["project_id"].as_str().unwrap_or("").to_string(),
            title: h["title"].as_str().unwrap_or("").to_string(),
            description: h["description"].as_str().unwrap_or("").to_string(),
            icon: h["icon_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
            downloads: h["downloads"].as_u64().unwrap_or(0),
            author: h["author"].as_str().unwrap_or("").to_string(),
            source: "modrinth".to_string(),
        })
        .collect())
}

// ---------------- CurseForge ----------------
fn cf_loader_type(l: &str) -> &'static str {
    match l {
        "forge" => "1",
        "fabric" => "4",
        "quilt" => "5",
        "neoforge" => "6",
        _ => "0",
    }
}

pub fn cf_key() -> Result<String> {
    let k = load_settings().curseforge_key.trim().to_string();
    if k.is_empty() {
        return Err(anyhow!(
            "Add a CurseForge API key in Settings to browse CurseForge."
        ));
    }
    Ok(k)
}

async fn search_curseforge(q: &str, mc: &str, loader: &str, offset: u32) -> Result<Vec<ModHit>> {
    let key = cf_key()?;
    let mut params: Vec<(&str, String)> = vec![
        ("gameId", "432".into()),
        ("classId", "6".into()),
        ("searchFilter", q.to_string()),
        ("pageSize", "20".into()),
        ("index", offset.to_string()),
        ("sortField", "2".into()),
        ("sortOrder", "desc".into()),
    ];
    if !mc.is_empty() {
        params.push(("gameVersion", mc.to_string()));
    }
    if loader != "vanilla" && !loader.is_empty() {
        params.push(("modLoaderType", cf_loader_type(loader).to_string()));
    }
    let r: Value = http()
        .get(format!("{CF}/mods/search"))
        .header("x-api-key", key)
        .query(&params)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(r["data"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| ModHit {
            id: m["id"].as_u64().unwrap_or(0).to_string(),
            title: m["name"].as_str().unwrap_or("").to_string(),
            description: m["summary"].as_str().unwrap_or("").to_string(),
            icon: m["logo"]["thumbnailUrl"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(String::from),
            downloads: m["downloadCount"].as_f64().unwrap_or(0.0) as u64,
            author: m["authors"][0]["name"].as_str().unwrap_or("").to_string(),
            source: "curseforge".to_string(),
        })
        .collect())
}

async fn install_curseforge(inst: &Instance, mod_id: &str) -> Result<Vec<String>> {
    let key = cf_key()?;
    let dir = mods_dir(&inst.id);
    tokio::fs::create_dir_all(&dir).await?;
    let mut index = load_index(&inst.id);
    let mut queue = vec![mod_id.to_string()];
    let mut seen: HashSet<String> = HashSet::new();
    let mut done = vec![];
    let mut failure: Option<anyhow::Error> = None;
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let is_root = id == mod_id;
        let pid = format!("cf:{id}");
        if !is_root
            && index.iter().any(|(f, p)| {
                *p == pid && (dir.join(f).exists() || dir.join(format!("{f}.disabled")).exists())
            })
        {
            continue;
        }
        let mut params: Vec<(&str, String)> = vec![
            ("gameVersion", inst.mc_version.clone()),
            ("pageSize", "20".into()),
        ];
        if inst.loader != "vanilla" {
            params.push(("modLoaderType", cf_loader_type(&inst.loader).to_string()));
        }
        let r: Value = http()
            .get(format!("{CF}/mods/{id}/files"))
            .header("x-api-key", key.clone())
            .query(&params)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let data = r["data"].as_array().cloned().unwrap_or_default();
        let Some(f) = data.iter().max_by_key(|f| {
            (
                f["releaseType"].as_u64() == Some(1),
                f["fileDate"].as_str().unwrap_or("").to_string(),
            )
        }) else {
            if is_root {
                failure = Some(anyhow!("No compatible file for Minecraft {}", inst.mc_version));
                break;
            }
            continue;
        };
        let file_name = safe_name(f["fileName"].as_str().unwrap_or(""))?;
        let file_id = f["id"].as_u64().unwrap_or(0);
        let url = match f["downloadUrl"].as_str() {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => format!(
                "https://edge.forgecdn.net/files/{}/{}/{}",
                file_id / 1000,
                file_id % 1000,
                file_name
            ),
        };
        let sha1 = f["hashes"]
            .as_array()
            .and_then(|h| h.iter().find(|x| x["algo"] == 1))
            .and_then(|x| x["value"].as_str())
            .map(String::from);
        let olds: Vec<String> = index
            .iter()
            .filter(|(file, p)| **p == pid && **file != file_name)
            .map(|(file, _)| file.clone())
            .collect();
        for o in olds {
            rm_both(&dir, &o);
            index.remove(&o);
        }
        if let Err(e) = download_file(&url, &dir.join(&file_name), sha1.as_deref(), None).await {
            failure = Some(e.context(
                "Download failed — this mod's author may block third-party launchers; download it manually from CurseForge and use \"Add .jar\"",
            ));
            break;
        }
        index.insert(file_name.clone(), pid);
        done.push(file_name);
        for d in f["dependencies"].as_array().into_iter().flatten() {
            if d["relationType"].as_u64() == Some(3) {
                if let Some(m) = d["modId"].as_u64() {
                    queue.push(m.to_string());
                }
            }
        }
    }
    save_index(&inst.id, &index);
    match failure {
        Some(e) => Err(e),
        None => Ok(done),
    }
}

// ---------------- helpers used by other modules ----------------
pub async fn ensure_by_slug(inst: &Instance, slug: &str) -> Result<()> {
    let needle = slug.to_lowercase();
    if let Ok(rd) = std::fs::read_dir(mods_dir(&inst.id)) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if n.contains(&needle) && n.ends_with(".jar") {
                return Ok(());
            }
        }
    }
    install_modrinth(inst, slug).await.map(|_| ())
}

pub async fn install_perf_pack(inst: &Instance) -> Result<Vec<String>> {
    let mut all = vec![];
    for slug in [
        "sodium",
        "lithium",
        "ferrite-core",
        "modernfix",
        "immediatelyfast",
        "entityculling",
    ] {
        if let Ok(mut v) = install_modrinth(inst, slug).await {
            all.append(&mut v);
        }
    }
    Ok(all)
}

// ---------------- commands ----------------
#[tauri::command]
pub async fn mods_search(
    query: String,
    mc_version: String,
    loader: String,
    source: String,
    offset: u32,
    kind: String,
) -> Result<Vec<ModHit>, String> {
    let r = if source == "curseforge" {
        if kind != "mod" {
            Err(anyhow!("CurseForge browsing supports mods only — switch the source to Modrinth"))
        } else {
            search_curseforge(&query, &mc_version, &loader, offset).await
        }
    } else {
        search_modrinth(&query, &mc_version, &loader, offset, &kind).await
    };
    r.map_err(es)
}

#[tauri::command]
pub async fn mods_install(
    instance_id: String,
    project_id: String,
    source: String,
) -> Result<Vec<String>, String> {
    let run = async {
        let inst = load_instance(&instance_id)?;
        if inst.loader == "vanilla" {
            return Err(anyhow!("Vanilla instances can't use mods — pick a mod loader first"));
        }
        if source == "curseforge" {
            install_curseforge(&inst, &project_id).await
        } else {
            install_modrinth(&inst, &project_id).await
        }
    };
    run.await.map_err(es)
}

#[tauri::command]
pub fn mods_list(instance_id: String) -> Result<Vec<InstalledMod>, String> {
    load_instance(&instance_id).map_err(es)?;
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(mods_dir(&instance_id)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            if let Some(base) = name.strip_suffix(".jar.disabled") {
                out.push(InstalledMod {
                    file: format!("{base}.jar"),
                    enabled: false,
                    size,
                });
            } else if name.ends_with(".jar") {
                out.push(InstalledMod {
                    file: name,
                    enabled: true,
                    size,
                });
            }
        }
    }
    out.sort_by(|a, b| a.file.to_lowercase().cmp(&b.file.to_lowercase()));
    Ok(out)
}

#[tauri::command]
pub fn mods_toggle(instance_id: String, file: String, enabled: bool) -> Result<(), String> {
    load_instance(&instance_id).map_err(es)?;
    let f = safe_name(&file).map_err(es)?;
    let dir = mods_dir(&instance_id);
    let on = dir.join(&f);
    let off = dir.join(format!("{f}.disabled"));
    if enabled {
        std::fs::rename(&off, &on).map_err(|e| e.to_string())
    } else {
        std::fs::rename(&on, &off).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn mods_delete(instance_id: String, file: String) -> Result<(), String> {
    load_instance(&instance_id).map_err(es)?;
    let f = safe_name(&file).map_err(es)?;
    rm_both(&mods_dir(&instance_id), &f);
    let mut idx = load_index(&instance_id);
    idx.remove(&f);
    save_index(&instance_id, &idx);
    Ok(())
}

#[tauri::command]
pub fn mods_add_local(instance_id: String, path: String) -> Result<(), String> {
    load_instance(&instance_id).map_err(es)?;
    let src = PathBuf::from(&path);
    let name = safe_name(&src.file_name().and_then(|s| s.to_str()).unwrap_or("")).map_err(es)?;
    if !name.to_lowercase().ends_with(".jar") {
        return Err("Only .jar files can be added".into());
    }
    let dir = mods_dir(&instance_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::copy(&src, dir.join(name)).map_err(|e| e.to_string())?;
    Ok(())
}

async fn check_updates(inst: &Instance) -> Result<Vec<UpdateInfo>> {
    let dir = mods_dir(&inst.id);
    let mut files: Vec<(String, bool, PathBuf)> = vec![];
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(base) = name.strip_suffix(".jar.disabled") {
                files.push((format!("{base}.jar"), false, e.path()));
            } else if name.ends_with(".jar") {
                files.push((name, true, e.path()));
            }
        }
    }
    let hashed: Vec<(String, bool, String)> = tokio::task::spawn_blocking(move || {
        files
            .into_iter()
            .filter_map(|(n, en, p)| sha1_file(&p).ok().map(|h| (n, en, h)))
            .collect()
    })
    .await?;
    if hashed.is_empty() {
        return Ok(vec![]);
    }
    let hashes: Vec<String> = hashed.iter().map(|x| x.2.clone()).collect();
    let res: Value = http()
        .post(format!("{MR}/version_files/update"))
        .json(&json!({
            "hashes": hashes,
            "algorithm": "sha1",
            "loaders": loaders_for(&inst.loader),
            "game_versions": [inst.mc_version.clone()]
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut index = load_index(&inst.id);
    let mut out: Vec<UpdateInfo> = vec![];
    for (name, enabled, h) in &hashed {
        let Some(nv) = res.get(h.as_str()) else {
            continue;
        };
        let pid = nv["project_id"].as_str().unwrap_or("").to_string();
        if pid.is_empty() {
            continue;
        }
        index.entry(name.clone()).or_insert_with(|| pid.clone());
        let files = nv["files"].as_array().cloned().unwrap_or_default();
        let Some(f) = files.iter().find(|f| f["primary"] == true).or(files.first()) else {
            continue;
        };
        let new_sha = f["hashes"]["sha1"].as_str().unwrap_or("");
        if new_sha.is_empty() || new_sha.eq_ignore_ascii_case(h) {
            continue;
        }
        out.push(UpdateInfo {
            file: name.clone(),
            enabled: *enabled,
            project_id: pid,
            title: name.clone(),
            latest: nv["version_number"].as_str().unwrap_or("").to_string(),
            new_file: f["filename"].as_str().unwrap_or("").to_string(),
            url: f["url"].as_str().unwrap_or("").to_string(),
            sha1: new_sha.to_string(),
            size: f["size"].as_u64().unwrap_or(0),
        });
    }
    save_index(&inst.id, &index);

    if !out.is_empty() {
        let ids: Vec<String> = out.iter().map(|u| u.project_id.clone()).collect();
        let list: Vec<Value> = http()
            .get(format!("{MR}/projects"))
            .query(&[("ids", jarr(&ids))])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        for u in out.iter_mut() {
            if let Some(p) = list.iter().find(|p| p["id"].as_str() == Some(u.project_id.as_str())) {
                if let Some(t) = p["title"].as_str() {
                    u.title = t.to_string();
                }
            }
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn mods_check_updates(instance_id: String) -> Result<Vec<UpdateInfo>, String> {
    let run = async {
        let inst = load_instance(&instance_id)?;
        check_updates(&inst).await
    };
    run.await.map_err(es)
}

#[tauri::command]
pub async fn mods_apply_updates(instance_id: String) -> Result<usize, String> {
    let run = async {
        let inst = load_instance(&instance_id)?;
        let ups = check_updates(&inst).await?;
        let dir = mods_dir(&inst.id);
        let mut index = load_index(&inst.id);
        let mut n = 0usize;
        for u in ups {
            let new_name = safe_name(&u.new_file)?;
            let dest_name = if u.enabled {
                new_name.clone()
            } else {
                format!("{new_name}.disabled")
            };
            download_file(&u.url, &dir.join(&dest_name), Some(u.sha1.as_str()), Some(u.size)).await?;
            if new_name != u.file {
                rm_both(&dir, &u.file);
                index.remove(&u.file);
            }
            index.insert(new_name, u.project_id.clone());
            n += 1;
        }
        save_index(&inst.id, &index);
        Ok::<usize, anyhow::Error>(n)
    };
    run.await.map_err(es)
}

#[derive(Serialize)]
pub struct ModInfo {
    pub file: String,
    pub title: String,
    pub icon: Option<String>,
    pub version: String,
}

/// Looks the installed jars up on Modrinth by hash to show proper names and icons.
#[tauri::command]
pub async fn mods_info(instance_id: String) -> Result<Vec<ModInfo>, String> {
    let run = async {
        let inst = load_instance(&instance_id)?;
        let dir = mods_dir(&inst.id);
        let mut files: Vec<(String, PathBuf)> = vec![];
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(base) = name.strip_suffix(".jar.disabled") {
                    files.push((format!("{base}.jar"), e.path()));
                } else if name.ends_with(".jar") {
                    files.push((name, e.path()));
                }
            }
        }
        let hashed: Vec<(String, String)> = tokio::task::spawn_blocking(move || {
            files
                .into_iter()
                .filter_map(|(n, p)| sha1_file(&p).ok().map(|h| (n, h)))
                .collect()
        })
        .await?;
        if hashed.is_empty() {
            return Ok(vec![]);
        }
        let hashes: Vec<String> = hashed.iter().map(|x| x.1.clone()).collect();
        let vers: Value = http()
            .post(format!("{MR}/version_files"))
            .json(&json!({ "hashes": hashes, "algorithm": "sha1" }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut pids: Vec<String> = vec![];
        for (_, h) in &hashed {
            if let Some(p) = vers.get(h.as_str()).and_then(|v| v["project_id"].as_str()) {
                if !pids.iter().any(|x| x == p) {
                    pids.push(p.to_string());
                }
            }
        }
        let projects: Vec<Value> = if pids.is_empty() {
            vec![]
        } else {
            http()
                .get(format!("{MR}/projects"))
                .query(&[("ids", jarr(&pids))])
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        };
        let mut out = vec![];
        for (name, h) in &hashed {
            let Some(v) = vers.get(h.as_str()) else {
                continue;
            };
            let pid = v["project_id"].as_str().unwrap_or("");
            let Some(p) = projects.iter().find(|p| p["id"].as_str() == Some(pid)) else {
                continue;
            };
            out.push(ModInfo {
                file: name.clone(),
                title: p["title"].as_str().unwrap_or(name.as_str()).to_string(),
                icon: p["icon_url"].as_str().filter(|s| !s.is_empty()).map(String::from),
                version: v["version_number"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok::<Vec<ModInfo>, anyhow::Error>(out)
    };
    run.await.map_err(es)
}
