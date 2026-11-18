use anyhow::{anyhow, Context, Result};
use futures_util::{stream, StreamExt};
use serde::{de::DeserializeOwned, Serialize};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

pub const UA: &str = "NimbusLauncher/1.0";

static VERIFY: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
pub fn is_verify() -> bool {
    VERIFY.load(Ordering::Relaxed)
}
pub fn set_cancel(on: bool) {
    CANCEL.store(on, Ordering::Relaxed);
}
pub fn is_cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}
/// When enabled, every file with a known SHA-1 is fully re-hashed (used by "Repair").
pub fn set_verify(on: bool) {
    VERIFY.store(on, Ordering::Relaxed);
}

pub fn es(e: anyhow::Error) -> String {
    format!("{e:#}")
}

// ---------- paths ----------
pub fn root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("NimbusLauncher")
}
pub fn shared() -> PathBuf {
    root().join("shared")
}
pub fn libraries() -> PathBuf {
    shared().join("libraries")
}
pub fn assets() -> PathBuf {
    shared().join("assets")
}
pub fn versions() -> PathBuf {
    shared().join("versions")
}
pub fn runtimes() -> PathBuf {
    root().join("runtimes")
}
pub fn instances_dir() -> PathBuf {
    root().join("instances")
}
pub fn instance_dir(id: &str) -> PathBuf {
    instances_dir().join(id)
}
pub fn game_dir(id: &str) -> PathBuf {
    instance_dir(id).join("minecraft")
}
pub fn cache_dir() -> PathBuf {
    root().join("cache")
}
pub fn skins_dir() -> PathBuf {
    root().join("skins")
}
pub fn ensure_dirs() -> Result<()> {
    for d in [
        libraries(),
        assets(),
        versions(),
        runtimes(),
        instances_dir(),
        cache_dir(),
        skins_dir(),
    ] {
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------- json ----------
pub fn read_json<T: DeserializeOwned>(p: &Path) -> Result<T> {
    let s = std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?;
    serde_json::from_str(&s).with_context(|| format!("parse {}", p.display()))
}
pub fn write_json<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, serde_json::to_vec_pretty(v)?)?;
    Ok(())
}

// ---------- http ----------
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
pub fn http() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(UA)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(32)
            .build()
            .expect("http client")
    })
}

pub async fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    let r = http()
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url}"))?;
    r.json::<T>().await.with_context(|| format!("decode {url}"))
}

pub async fn get_text(url: &str) -> Result<String> {
    let r = http()
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url}"))?;
    Ok(r.text().await?)
}

// ---------- hashing ----------
pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(hex::encode(h.finalize()))
}

// ---------- downloads ----------
#[derive(Clone, Debug)]
pub struct Job {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

fn is_valid(dest: &Path, sha1: Option<&str>, size: Option<u64>) -> bool {
    let Ok(meta) = std::fs::metadata(dest) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if VERIFY.load(Ordering::Relaxed) {
        if let Some(h) = sha1 {
            return sha1_file(dest)
                .map(|x| x.eq_ignore_ascii_case(h))
                .unwrap_or(false);
        }
    }
    // Fast path: a matching size is trusted (keeps relaunches near-instant).
    if let Some(s) = size {
        return s == meta.len();
    }
    if let Some(h) = sha1 {
        return sha1_file(dest)
            .map(|x| x.eq_ignore_ascii_case(h))
            .unwrap_or(false);
    }
    meta.len() > 0
}

async fn try_download(url: &str, dest: &Path, sha1: Option<&str>) -> Result<()> {
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    let have = tokio::fs::metadata(&tmp).await.map(|m| m.len()).unwrap_or(0);
    let mut req = http().get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.send().await?;
    if resp.status().as_u16() == 416 {
        // stale partial file: start over on the next attempt
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(anyhow!("stale partial download, restarting"));
    }
    let resuming = have > 0 && resp.status().as_u16() == 206;
    let resp = resp.error_for_status()?;
    let mut hasher = Sha1::new();
    let mut file = if resuming {
        let mut old = std::fs::File::open(&tmp)?;
        std::io::copy(&mut old, &mut hasher)?;
        tokio::fs::OpenOptions::new().append(true).open(&tmp).await?
    } else {
        tokio::fs::File::create(&tmp).await?
    };
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if is_cancelled() {
            return Err(anyhow!("Cancelled"));
        }
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    if let Some(h) = sha1 {
        let got = hex::encode(hasher.finalize());
        if !got.eq_ignore_ascii_case(h) {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(anyhow!("checksum mismatch for {url}"));
        }
    }
    tokio::fs::rename(&tmp, dest).await?;
    Ok(())
}

pub async fn download_file(
    url: &str,
    dest: &Path,
    sha1: Option<&str>,
    size: Option<u64>,
) -> Result<()> {
    if is_valid(dest, sha1, size) {
        return Ok(());
    }
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..3u64 {
        if is_cancelled() {
            return Err(anyhow!("Cancelled"));
        }
        match try_download(url, dest, sha1).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                tokio::time::sleep(Duration::from_millis(400 * (attempt + 1))).await;
            }
        }
    }
    Err(last
        .unwrap_or_else(|| anyhow!("unknown error"))
        .context(format!("download {url}")))
}

pub async fn download_many<F>(jobs: Vec<Job>, concurrency: usize, on_progress: F) -> Result<()>
where
    F: Fn(usize, usize) + Send + Sync + 'static,
{
    let total = jobs.len();
    if total == 0 {
        return Ok(());
    }
    let done = Arc::new(AtomicUsize::new(0));
    let cb = Arc::new(on_progress);
    let results = stream::iter(jobs)
        .map(|job| {
            let done = done.clone();
            let cb = cb.clone();
            async move {
                download_file(&job.url, &job.dest, job.sha1.as_deref(), job.size).await?;
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                (*cb)(n, total);
                Ok::<(), anyhow::Error>(())
            }
        })
        .buffer_unordered(concurrency.max(1))
        .collect::<Vec<_>>()
        .await;
    for r in results {
        r?;
    }
    Ok(())
}

// ---------- progress events ----------
#[derive(Clone, Serialize)]
struct ProgressEvt {
    task: String,
    label: String,
    done: usize,
    total: usize,
}

pub fn progress(app: &AppHandle, task: &str, label: &str, done: usize, total: usize) {
    let _ = app.emit(
        "progress",
        ProgressEvt {
            task: task.to_string(),
            label: label.to_string(),
            done,
            total,
        },
    );
}

pub fn throttled(app: &AppHandle, task: &str, label: &str, done: usize, total: usize) {
    if done == total || done % 20 == 0 {
        progress(app, task, label, done, total);
    }
}

// ---------- misc ----------
/// "group:artifact:version[:classifier][@ext]" -> maven repository path
pub fn maven_path(name: &str) -> String {
    let (coord, ext) = match name.split_once('@') {
        Some((c, e)) => (c, e),
        None => (name, "jar"),
    };
    let p: Vec<&str> = coord.split(':').collect();
    if p.len() < 3 {
        return coord.to_string();
    }
    let (g, a, v) = (p[0], p[1], p[2]);
    let file = match p.get(3) {
        Some(c) => format!("{a}-{v}-{c}.{ext}"),
        None => format!("{a}-{v}.{ext}"),
    };
    format!("{}/{a}/{v}/{file}", g.replace('.', "/"))
}

pub fn spawn_detached(program: &str, args: &[&str]) -> Result<()> {
    std::process::Command::new(program)
        .args(args)
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;
    Ok(())
}
