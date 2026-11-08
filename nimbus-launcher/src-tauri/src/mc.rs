use crate::util::*;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RUNTIME_ALL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

// ---------------- manifest ----------------
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Manifest {
    pub versions: Vec<MV>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct MV {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub sha1: Option<String>,
    #[serde(rename = "releaseTime")]
    pub release_time: String,
}

pub async fn manifest() -> Result<Manifest> {
    let cache = cache_dir().join("version_manifest.json");
    match get_json::<Manifest>(MANIFEST_URL).await {
        Ok(m) => {
            let _ = write_json(&cache, &m);
            Ok(m)
        }
        Err(e) => read_json::<Manifest>(&cache).map_err(|_| e),
    }
}

#[derive(Serialize)]
pub struct VersionEntry {
    pub id: String,
    pub kind: String,
    pub release_time: String,
}

#[tauri::command]
pub async fn list_versions(snapshots: bool) -> Result<Vec<VersionEntry>, String> {
    let m = manifest().await.map_err(es)?;
    Ok(m.versions
        .into_iter()
        .filter(|v| snapshots || v.kind == "release")
        .map(|v| VersionEntry {
            id: v.id,
            kind: v.kind,
            release_time: v.release_time,
        })
        .collect())
}

pub async fn ensure_vanilla(id: &str) -> Result<()> {
    let path = versions().join(id).join(format!("{id}.json"));
    if path.exists() {
        return Ok(());
    }
    let m = manifest().await?;
    let mv = m
        .versions
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| anyhow!("Minecraft version {id} not found"))?;
    download_file(&mv.url, &path, mv.sha1.as_deref(), None).await
}

// ---------------- version json ----------------
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum StrOrVec {
    One(String),
    Many(Vec<String>),
}
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum Arg {
    Plain(String),
    Ruled { rules: Vec<Rule>, value: StrOrVec },
}
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Arg>,
    #[serde(default)]
    pub jvm: Vec<Arg>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Artifact {
    pub path: Option<String>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    #[serde(default)]
    pub url: String,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct LibDownloads {
    pub artifact: Option<Artifact>,
    pub classifiers: Option<HashMap<String, Artifact>>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibDownloads>,
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub rules: Option<Vec<Rule>>,
    pub clientreq: Option<bool>,
    pub natives: Option<HashMap<String, String>>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Downloads {
    pub client: Option<Artifact>,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub url: String,
}
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct JavaVersion {
    pub component: String,
    #[serde(rename = "majorVersion")]
    pub major: u32,
}
#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "inheritsFrom")]
    pub inherits_from: Option<String>,
    #[serde(rename = "mainClass")]
    pub main_class: Option<String>,
    #[serde(rename = "minecraftArguments")]
    pub minecraft_arguments: Option<String>,
    pub arguments: Option<Arguments>,
    #[serde(rename = "assetIndex")]
    pub asset_index: Option<AssetIndexRef>,
    pub assets: Option<String>,
    pub downloads: Option<Downloads>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(rename = "javaVersion")]
    pub java_version: Option<JavaVersion>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Very old Forge profiles name the vanilla version here instead of `inheritsFrom`.
    pub jar: Option<String>,
    /// id of the root (vanilla) version whose jar is used. Not part of the file.
    #[serde(skip)]
    pub jar_id: String,
}

fn lib_key(name: &str) -> String {
    let base = name.split('@').next().unwrap_or(name);
    let p: Vec<&str> = base.split(':').collect();
    match p.len() {
        0..=2 => base.to_string(),
        3 => format!("{}:{}", p[0], p[1]),
        _ => format!("{}:{}:{}", p[0], p[1], p[3]),
    }
}

pub fn load_version(id: &str) -> Result<VersionJson> {
    let path = versions().join(id).join(format!("{id}.json"));
    let mut v: VersionJson = read_json(&path)?;
    if v.inherits_from.is_none() {
        if let Some(j) = v.jar.clone() {
            if j != v.id {
                v.inherits_from = Some(j);
            }
        }
    }
    match v.inherits_from.clone() {
        Some(parent) => {
            let p = load_version(&parent)?;
            Ok(merge(p, v))
        }
        None => {
            v.jar_id = v.id.clone();
            Ok(v)
        }
    }
}

fn merge(parent: VersionJson, child: VersionJson) -> VersionJson {
    let child_keys: HashSet<String> = child.libraries.iter().map(|l| lib_key(&l.name)).collect();
    let mut libs = child.libraries.clone();
    libs.extend(
        parent
            .libraries
            .into_iter()
            .filter(|l| !child_keys.contains(&lib_key(&l.name))),
    );
    let arguments = match (parent.arguments, child.arguments) {
        (Some(p), Some(c)) => Some(Arguments {
            game: [p.game, c.game].concat(),
            jvm: [p.jvm, c.jvm].concat(),
        }),
        (p, None) => p,
        (None, c) => c,
    };
    VersionJson {
        id: child.id,
        inherits_from: None,
        main_class: child.main_class.or(parent.main_class),
        minecraft_arguments: child.minecraft_arguments.or(parent.minecraft_arguments),
        arguments,
        asset_index: child.asset_index.or(parent.asset_index),
        assets: child.assets.or(parent.assets),
        downloads: child.downloads.or(parent.downloads),
        libraries: libs,
        java_version: child.java_version.or(parent.java_version),
        kind: child.kind.or(parent.kind),
        jar: None,
        jar_id: parent.jar_id,
    }
}

// ---------------- rules ----------------
pub fn os_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}
fn current_arch() -> &'static str {
    if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

pub fn rules_allow(rules: &[Rule], features: &HashMap<&str, bool>) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for r in rules {
        let mut matches = true;
        if let Some(os) = &r.os {
            if let Some(n) = &os.name {
                if n != os_name() {
                    matches = false;
                }
            }
            if let Some(a) = &os.arch {
                if a != current_arch() {
                    matches = false;
                }
            }
        }
        if let Some(f) = &r.features {
            for (k, v) in f {
                if features.get(k.as_str()).copied().unwrap_or(false) != *v {
                    matches = false;
                }
            }
        }
        if matches {
            allowed = r.action == "allow";
        }
    }
    allowed
}

// ---------------- libraries ----------------
pub struct LibPlan {
    pub jobs: Vec<Job>,
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<PathBuf>,
}

pub fn jar_path(jar_id: &str) -> PathBuf {
    versions().join(jar_id).join(format!("{jar_id}.jar"))
}

pub fn lib_plan(v: &VersionJson) -> Result<LibPlan> {
    let feats: HashMap<&str, bool> = HashMap::new();
    let mut jobs = vec![];
    let mut classpath: Vec<PathBuf> = vec![];
    let mut natives = vec![];
    let mut seen = HashSet::new();
    for lib in &v.libraries {
        if lib.clientreq == Some(false) {
            continue;
        }
        if let Some(r) = &lib.rules {
            if !rules_allow(r, &feats) {
                continue;
            }
        }
        // legacy natives (extracted into the natives directory)
        if let Some(map) = &lib.natives {
            if let Some(cls) = map.get(os_name()) {
                let cls = cls.replace(
                    "${arch}",
                    if cfg!(target_pointer_width = "64") {
                        "64"
                    } else {
                        "32"
                    },
                );
                if let Some(a) = lib
                    .downloads
                    .as_ref()
                    .and_then(|d| d.classifiers.as_ref())
                    .and_then(|c| c.get(&cls))
                {
                    let path = a
                        .path
                        .clone()
                        .unwrap_or_else(|| format!("{}-{}", maven_path(&lib.name), cls));
                    let dest = libraries().join(&path);
                    jobs.push(Job {
                        url: a.url.clone(),
                        dest: dest.clone(),
                        sha1: a.sha1.clone(),
                        size: a.size,
                    });
                    natives.push(dest);
                }
            }
        }
        // regular artifact
        let (path, url, sha1, size) = match lib.downloads.as_ref().and_then(|d| d.artifact.as_ref())
        {
            Some(a) => (
                a.path.clone().unwrap_or_else(|| maven_path(&lib.name)),
                a.url.clone(),
                a.sha1.clone(),
                a.size,
            ),
            None if lib.downloads.is_none() => {
                let base = lib
                    .url
                    .clone()
                    .unwrap_or_else(|| "https://libraries.minecraft.net/".to_string());
                let base = base
                    .replace(
                        "http://files.minecraftforge.net/maven",
                        "https://maven.minecraftforge.net",
                    )
                    .replacen("http://", "https://", 1);
                let base = if base.ends_with('/') {
                    base
                } else {
                    format!("{base}/")
                };
                let p = maven_path(&lib.name);
                (p.clone(), format!("{base}{p}"), lib.sha1.clone(), lib.size)
            }
            None => continue, // natives-only entry
        };
        let dest = libraries().join(&path);
        if seen.insert(dest.clone()) {
            if !url.is_empty() {
                jobs.push(Job {
                    url,
                    dest: dest.clone(),
                    sha1,
                    size,
                });
            }
            classpath.push(dest);
        }
    }
    Ok(LibPlan {
        jobs,
        classpath,
        natives,
    })
}

pub fn extract_natives(jars: &[PathBuf], dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for jar in jars {
        let f = std::fs::File::open(jar)?;
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            let name = e.name().to_string();
            let lower = name.to_lowercase();
            if e.is_dir() || name.starts_with("META-INF") {
                continue;
            }
            if !(lower.ends_with(".dll")
                || lower.ends_with(".so")
                || lower.ends_with(".dylib")
                || lower.ends_with(".jnilib"))
            {
                continue;
            }
            let Some(fname) = Path::new(&name).file_name() else {
                continue;
            };
            let mut out = std::fs::File::create(dest.join(fname))?;
            std::io::copy(&mut e, &mut out)?;
        }
    }
    Ok(())
}

// ---------------- assets + install ----------------
#[derive(Deserialize)]
struct AssetObj {
    hash: String,
    size: u64,
}
#[derive(Deserialize)]
struct AssetIndex {
    objects: HashMap<String, AssetObj>,
    #[serde(default)]
    r#virtual: bool,
    #[serde(default)]
    map_to_resources: bool,
}

pub async fn install_game(
    app: &AppHandle,
    version_id: &str,
    task: &str,
    conc: usize,
    natives_dir: &Path,
    game_dir: &Path,
) -> Result<VersionJson> {
    let v = load_version(version_id)?;
    let plan = lib_plan(&v)?;
    let mut jobs = plan.jobs.clone();
    let client = v
        .downloads
        .as_ref()
        .and_then(|d| d.client.clone())
        .ok_or_else(|| anyhow!("version has no client download"))?;
    jobs.push(Job {
        url: client.url,
        dest: jar_path(&v.jar_id),
        sha1: client.sha1,
        size: client.size,
    });

    let ai = v
        .asset_index
        .clone()
        .ok_or_else(|| anyhow!("missing asset index"))?;
    let idx_path = assets().join("indexes").join(format!("{}.json", ai.id));
    download_file(&ai.url, &idx_path, ai.sha1.as_deref(), ai.size).await?;
    let idx: AssetIndex = read_json(&idx_path)?;
    let mut seen = HashSet::new();
    let asset_jobs: Vec<Job> = idx
        .objects
        .values()
        .filter(|o| o.hash.len() >= 2 && seen.insert(o.hash.clone()))
        .map(|o| {
            let p = &o.hash[..2];
            Job {
                url: format!("https://resources.download.minecraft.net/{p}/{}", o.hash),
                dest: assets().join("objects").join(p).join(&o.hash),
                sha1: Some(o.hash.clone()),
                size: Some(o.size),
            }
        })
        .collect();

    let (a1, a2) = (app.clone(), app.clone());
    let (t1, t2) = (task.to_string(), task.to_string());
    tokio::try_join!(
        download_many(jobs, conc, move |d, t| throttled(
            &a1,
            &t1,
            "Downloading libraries",
            d,
            t
        )),
        download_many(asset_jobs, conc, move |d, t| throttled(
            &a2,
            &t2,
            "Downloading assets",
            d,
            t
        )),
    )?;

    if !plan.natives.is_empty() {
        extract_natives(&plan.natives, natives_dir)?;
    }
    // Pre-1.7 versions read assets from a "virtual" folder or the game's resources/ dir.
    if idx.r#virtual || idx.map_to_resources {
        let target = if idx.r#virtual {
            assets().join("virtual").join(&ai.id)
        } else {
            game_dir.join("resources")
        };
        for (name, o) in &idx.objects {
            if o.hash.len() < 2
                || Path::new(name)
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            let dest = target.join(name);
            if dest.exists() {
                continue;
            }
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            let src = assets().join("objects").join(&o.hash[..2]).join(&o.hash);
            std::fs::copy(&src, &dest)?;
        }
    }
    Ok(v)
}

// ---------------- java runtime ----------------
fn runtime_platform() -> &'static str {
    if cfg!(all(windows, target_arch = "x86_64")) {
        "windows-x64"
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        "windows-arm64"
    } else if cfg!(windows) {
        "windows-x86"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "mac-os-arm64"
    } else if cfg!(target_os = "macos") {
        "mac-os"
    } else if cfg!(target_arch = "x86") {
        "linux-i386"
    } else {
        "linux"
    }
}

/// Minecraft 1.18 and older ship no arm64 natives, so Apple Silicon runs them through an x64 Java (Rosetta).
fn needs_x64_java(jar_id: &str) -> bool {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return false;
    }
    let mut it = jar_id.split('.');
    let major = it.next().and_then(|p| p.parse::<u32>().ok());
    let minor = it.next().and_then(|p| p.parse::<u32>().ok());
    major == Some(1) && minor.map(|m| m < 19).unwrap_or(false)
}

pub fn find_java(dir: &Path) -> Option<PathBuf> {
    [
        "bin/java.exe",
        "bin/java",
        "jre.bundle/Contents/Home/bin/java",
        "Contents/Home/bin/java",
    ]
    .iter()
    .map(|p| dir.join(p))
    .find(|p| p.exists())
}

pub async fn ensure_java(
    app: &AppHandle,
    v: &VersionJson,
    task: &str,
    override_path: &str,
) -> Result<PathBuf> {
    let ov = override_path.trim();
    if !ov.is_empty() {
        let p = PathBuf::from(ov);
        if p.exists() {
            return Ok(p);
        }
        return Err(anyhow!("Java path not found: {ov}"));
    }
    let comp = v
        .java_version
        .as_ref()
        .map(|j| j.component.clone())
        .unwrap_or_else(|| "jre-legacy".to_string());
    let x64 = needs_x64_java(&v.jar_id);
    let dir = runtimes().join(if x64 {
        format!("{comp}-x64")
    } else {
        comp.clone()
    });
    let marker = dir.join(".installed");
    if marker.exists() {
        if let Some(j) = find_java(&dir) {
            return Ok(j);
        }
    }
    progress(app, task, "Downloading Java runtime", 0, 0);
    let all: Value = get_json(RUNTIME_ALL).await?;
    let plat = if x64 { "mac-os" } else { runtime_platform() };
    let manifest_url = all[plat][&comp][0]["manifest"]["url"]
        .as_str()
        .ok_or_else(|| {
            anyhow!("No Mojang Java runtime '{comp}' for this platform ({plat}). Set a Java path in the instance settings.")
        })?
        .to_string();
    let man: Value = get_json(&manifest_url).await?;
    let files = man["files"]
        .as_object()
        .ok_or_else(|| anyhow!("bad runtime manifest"))?;
    let mut jobs = vec![];
    let mut links: Vec<(PathBuf, String)> = vec![];
    let mut execs: Vec<PathBuf> = vec![];
    for (name, f) in files {
        match f["type"].as_str() {
            Some("directory") => {
                std::fs::create_dir_all(dir.join(name))?;
            }
            Some("file") => {
                let raw = &f["downloads"]["raw"];
                if let Some(url) = raw["url"].as_str() {
                    jobs.push(Job {
                        url: url.to_string(),
                        dest: dir.join(name),
                        sha1: raw["sha1"].as_str().map(String::from),
                        size: raw["size"].as_u64(),
                    });
                    if f["executable"].as_bool() == Some(true) {
                        execs.push(dir.join(name));
                    }
                }
            }
            Some("link") => {
                if let Some(t) = f["target"].as_str() {
                    links.push((dir.join(name), t.to_string()));
                }
            }
            _ => {}
        }
    }
    let (a, t) = (app.clone(), task.to_string());
    download_many(jobs, 32, move |d, tot| {
        throttled(&a, &t, "Downloading Java runtime", d, tot)
    })
    .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for e in &execs {
            if let Ok(m) = std::fs::metadata(e) {
                let mut p = m.permissions();
                p.set_mode(0o755);
                let _ = std::fs::set_permissions(e, p);
            }
        }
        for (l, t) in &links {
            let _ = std::fs::remove_file(l);
            if let Some(p) = l.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::os::unix::fs::symlink(t, l);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (&execs, &links);
    }
    std::fs::write(&marker, "ok")?;
    find_java(&dir).ok_or_else(|| anyhow!("Java runtime installed but executable not found"))
}
