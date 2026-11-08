use crate::util::*;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Read;
use std::path::{Component, Path};
use tauri::AppHandle;

// ---------------- settings ----------------
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub ms_client_id: String,
    pub curseforge_key: String,
    pub default_memory_mb: u32,
    pub concurrency: usize,
    pub show_snapshots: bool,
    pub java_path: String,
    pub theme: String,
    pub update_repo: String,
    /// custom shared folders (empty = default); never sent to the UI
    pub mods_dir: String,
    pub packs_dir: String,
    /// computed for the UI: is a custom folder in use?
    pub mods_custom: bool,
    pub packs_custom: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Settings {
            ms_client_id: String::new(),
            curseforge_key: String::new(),
            default_memory_mb: 4096,
            concurrency: 24,
            show_snapshots: false,
            java_path: String::new(),
            theme: "dark".to_string(),
            update_repo: String::new(),
            mods_dir: String::new(),
            packs_dir: String::new(),
            mods_custom: false,
            packs_custom: false,
        }
    }
}
pub fn load_settings() -> Settings {
    read_json(&root().join("settings.json")).unwrap_or_default()
}
#[tauri::command]
pub fn settings_get() -> Settings {
    // the custom folder paths stay on the Rust side; the UI only learns whether one is set
    let mut s = load_settings();
    s.mods_custom = !s.mods_dir.trim().is_empty();
    s.packs_custom = !s.packs_dir.trim().is_empty();
    s.mods_dir.clear();
    s.packs_dir.clear();
    s
}
#[tauri::command]
pub fn settings_save(settings: Settings) -> Result<(), String> {
    let mut s = settings;
    s.concurrency = s.concurrency.clamp(4, 64);
    s.default_memory_mb = s.default_memory_mb.max(512);
    let cur = load_settings();
    s.mods_dir = cur.mods_dir;
    s.packs_dir = cur.packs_dir;
    s.mods_custom = false;
    s.packs_custom = false;
    write_json(&root().join("settings.json"), &s).map_err(es)
}

// ---------------- instances ----------------
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub loader: String,
    #[serde(default)]
    pub loader_version: String,
    #[serde(default)]
    pub resolved_id: String,
    #[serde(default)]
    pub memory_mb: u32,
    #[serde(default)]
    pub java_path: String,
    #[serde(default)]
    pub jvm_args: String,
    #[serde(default)]
    pub perf_pack: bool,
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub last_played: u64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub wrapper: String,
    #[serde(default = "yes")]
    pub use_shared: bool,
}

fn yes() -> bool {
    true
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

pub fn load_instance(id: &str) -> Result<Instance> {
    if !valid_id(id) {
        return Err(anyhow!("invalid instance id"));
    }
    read_json(&instance_dir(id).join("instance.json"))
}
pub fn save_instance(i: &Instance) -> Result<()> {
    write_json(&instance_dir(&i.id).join("instance.json"), i)
}

#[tauri::command]
pub fn instances_list() -> Vec<Instance> {
    let mut out: Vec<Instance> = vec![];
    if let Ok(rd) = std::fs::read_dir(instances_dir()) {
        for e in rd.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if let Ok(i) = load_instance(name) {
                    out.push(i);
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.last_played
            .max(b.created)
            .cmp(&a.last_played.max(a.created))
    });
    out
}

#[derive(Deserialize)]
pub struct NewInstance {
    pub name: String,
    pub mc_version: String,
    pub loader: String,
    #[serde(default)]
    pub loader_version: String,
    pub memory_mb: u32,
    #[serde(default)]
    pub perf_pack: bool,
}

pub fn create(spec: NewInstance) -> Result<Instance> {
    create_with(spec, true)
}

pub fn create_with(spec: NewInstance, shared: bool) -> Result<Instance> {
    let name = spec.name.trim().to_string();
    if name.is_empty() {
        return Err(anyhow!("Please enter a name"));
    }
    if spec.mc_version.trim().is_empty() {
        return Err(anyhow!("Please choose a Minecraft version"));
    }
    if !["vanilla", "fabric", "quilt", "forge", "neoforge"].contains(&spec.loader.as_str()) {
        return Err(anyhow!("Unknown mod loader"));
    }
    let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let inst = Instance {
        id,
        name,
        mc_version: spec.mc_version,
        loader: spec.loader,
        loader_version: spec.loader_version,
        resolved_id: String::new(),
        memory_mb: spec.memory_mb.max(512),
        java_path: String::new(),
        jvm_args: String::new(),
        perf_pack: spec.perf_pack,
        installed: false,
        created: now_secs(),
        last_played: 0,
        width: 0,
        height: 0,
        server: String::new(),
        fullscreen: false,
        wrapper: String::new(),
        use_shared: shared,
    };
    std::fs::create_dir_all(game_dir(&inst.id).join("mods"))?;
    save_instance(&inst)?;
    let _ = crate::shared::apply(&inst, &load_settings());
    Ok(inst)
}

#[tauri::command]
pub fn instance_create(spec: NewInstance) -> Result<Instance, String> {
    create(spec).map_err(es)
}

#[tauri::command]
pub fn instance_save(instance: Instance) -> Result<Instance, String> {
    let mut cur = load_instance(&instance.id).map_err(es)?;
    let n = instance.name.trim();
    if !n.is_empty() {
        cur.name = n.to_string();
    }
    cur.memory_mb = instance.memory_mb.max(512);
    cur.java_path = instance.java_path.trim().to_string();
    cur.jvm_args = instance.jvm_args.trim().to_string();
    cur.width = instance.width;
    cur.height = instance.height;
    cur.server = instance.server.trim().to_string();
    cur.fullscreen = instance.fullscreen;
    cur.wrapper = instance.wrapper.trim().to_string();
    cur.use_shared = instance.use_shared;
    save_instance(&cur).map_err(es)?;
    crate::shared::apply(&cur, &load_settings()).map_err(es)?;
    Ok(cur)
}

#[tauri::command]
pub fn instance_delete(id: String) -> Result<(), String> {
    if !valid_id(&id) {
        return Err("invalid id".into());
    }
    if crate::launch::is_running(&id) {
        return Err("Stop the game before deleting this instance".into());
    }
    crate::shared::detach_all(&id);
    std::fs::remove_dir_all(instance_dir(&id)).map_err(|e| e.to_string())
}

// ---------------- .mrpack import ----------------
fn safe_rel(rel: &str) -> bool {
    !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

#[tauri::command]
pub async fn import_mrpack(app: AppHandle, path: String) -> Result<Instance, String> {
    import_inner(app, path).await.map_err(es)
}

pub async fn import_inner(app: AppHandle, path: String) -> Result<Instance> {
    let idx: Value = {
        let f = std::fs::File::open(&path)?;
        let mut z = zip::ZipArchive::new(f)?;
        let mut e = z
            .by_name("modrinth.index.json")
            .map_err(|_| anyhow!("Not a valid .mrpack (modrinth.index.json missing)"))?;
        let mut s = String::new();
        e.read_to_string(&mut s)?;
        let v: Value = serde_json::from_str(&s)?;
        v
    };
    let deps = &idx["dependencies"];
    let mc = deps["minecraft"]
        .as_str()
        .ok_or_else(|| anyhow!("modpack has no Minecraft version"))?
        .to_string();
    let (loader, lv) = if let Some(v) = deps["fabric-loader"].as_str() {
        ("fabric", v)
    } else if let Some(v) = deps["quilt-loader"].as_str() {
        ("quilt", v)
    } else if let Some(v) = deps["neoforge"].as_str() {
        ("neoforge", v)
    } else if let Some(v) = deps["forge"].as_str() {
        ("forge", v)
    } else {
        ("vanilla", "")
    };
    let name = idx["name"].as_str().unwrap_or("Imported modpack").to_string();
    let st = load_settings();
    let inst = create_with(NewInstance {
        name,
        mc_version: mc,
        loader: loader.to_string(),
        loader_version: lv.to_string(),
        memory_mb: st.default_memory_mb,
        perf_pack: false,
    }, false)?;
    let gdir = game_dir(&inst.id);

    let mut jobs = vec![];
    for f in idx["files"].as_array().into_iter().flatten() {
        if f["env"]["client"].as_str() == Some("unsupported") {
            continue;
        }
        let Some(rel) = f["path"].as_str() else {
            continue;
        };
        if !safe_rel(rel) {
            continue;
        }
        let Some(url) = f["downloads"][0].as_str() else {
            continue;
        };
        jobs.push(Job {
            url: url.to_string(),
            dest: gdir.join(rel),
            sha1: f["hashes"]["sha1"].as_str().map(String::from),
            size: f["fileSize"].as_u64(),
        });
    }
    let (a, t) = (app.clone(), inst.id.clone());
    download_many(jobs, st.concurrency, move |d, tot| {
        throttled(&a, &t, "Importing modpack", d, tot)
    })
    .await?;

    let f = std::fs::File::open(&path)?;
    let mut z = zip::ZipArchive::new(f)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let Some(enc) = e.enclosed_name() else {
            continue;
        };
        let rel = if let Ok(r) = enc.strip_prefix("overrides") {
            r.to_path_buf()
        } else if let Ok(r) = enc.strip_prefix("client-overrides") {
            r.to_path_buf()
        } else {
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = gdir.join(&rel);
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
    Ok(inst)
}

/// Switches an existing instance to another Minecraft version and/or mod loader (worlds and settings are kept).
#[tauri::command]
pub fn instance_retarget(
    id: String,
    mc_version: String,
    loader: String,
    loader_version: String,
) -> Result<Instance, String> {
    if crate::launch::is_running(&id) {
        return Err("Stop the game first".into());
    }
    if !["vanilla", "fabric", "quilt", "forge", "neoforge"].contains(&loader.as_str()) {
        return Err("Unknown mod loader".into());
    }
    if mc_version.trim().is_empty() {
        return Err("Choose a Minecraft version".into());
    }
    let mut i = load_instance(&id).map_err(es)?;
    i.mc_version = mc_version;
    i.loader = loader;
    i.loader_version = loader_version;
    i.resolved_id = String::new();
    let _ = std::fs::remove_dir_all(instance_dir(&id).join("natives"));
    save_instance(&i).map_err(es)?;
    Ok(i)
}
