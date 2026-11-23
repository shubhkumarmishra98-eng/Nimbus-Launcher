//! In-launcher updates from GitHub Releases.
//!
//! Every push to the repo is built by GitHub Actions and published as a release containing the .exe
//! (plus a .sha256 file). The launcher compares the newest release with its own version and, on Windows,
//! can download + swap itself in place.
use crate::instances::load_settings;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

const DEFAULT_REPO: &str = "shubhkumarmishra98-eng/Nimbus-Launcher";

pub fn current_version() -> String {
    option_env!("NIMBUS_VERSION")
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .trim()
        .trim_start_matches('v')
        .to_string()
}

/// Update source: a repo typed into settings (advanced), else the one baked in at build time, else the default.
pub fn repo() -> String {
    let s = load_settings().update_repo.trim().to_string();
    if !s.is_empty() {
        return s;
    }
    option_env!("NIMBUS_UPDATE_REPO")
        .filter(|r| !r.trim().is_empty())
        .unwrap_or(DEFAULT_REPO)
        .trim()
        .to_string()
}

fn valid_repo(r: &str) -> bool {
    r.matches('/').count() == 1 && r.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
}

fn parse(s: &str) -> Vec<u64> {
    s.split('.')
        .map(|p| {
            p.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

async fn latest_release(repo: &str) -> Result<Option<Value>> {
    let r = http()
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    if r.status().as_u16() == 404 {
        return Ok(None);
    }
    Ok(Some(r.error_for_status()?.json().await?))
}

/// (file name, download url, size) of the release's .exe
fn exe_asset(rel: &Value) -> Option<(String, String, u64)> {
    rel["assets"].as_array()?.iter().find_map(|a| {
        let n = a["name"].as_str()?;
        if n.to_lowercase().ends_with(".exe") {
            Some((
                n.to_string(),
                a["browser_download_url"].as_str()?.to_string(),
                a["size"].as_u64().unwrap_or(0),
            ))
        } else {
            None
        }
    })
}

fn short_notes(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(140)
        .collect()
}

#[derive(Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub size: u64,
}

#[tauri::command]
pub fn app_version() -> String {
    current_version()
}

/// Returns the newest release if it is newer than this build.
#[tauri::command]
pub async fn update_check() -> Result<Option<UpdateInfo>, String> {
    let run = async {
        let repo = repo();
        if !valid_repo(&repo) {
            return Err(anyhow!("The update source is invalid"));
        }
        let Some(rel) = latest_release(&repo).await? else {
            return Ok(None);
        };
        let tag = rel["tag_name"]
            .as_str()
            .unwrap_or("")
            .trim()
            .trim_start_matches('v')
            .to_string();
        if tag.is_empty() || parse(&tag) <= parse(&current_version()) {
            return Ok(None);
        }
        let size = exe_asset(&rel).map(|a| a.2).unwrap_or(0);
        Ok::<Option<UpdateInfo>, anyhow::Error>(Some(UpdateInfo {
            version: tag,
            notes: short_notes(rel["body"].as_str().unwrap_or("")),
            page: rel["html_url"].as_str().unwrap_or("").to_string(),
            size,
        }))
    };
    run.await.map_err(es)
}

/// Downloads the new .exe, checks its SHA-256, swaps it in for the running one and restarts.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    install(app).await.map_err(es)
}

#[cfg(not(windows))]
async fn install(_app: AppHandle) -> Result<()> {
    Err(anyhow!(
        "Automatic updating is only available on Windows. Use “Open download page” instead."
    ))
}

#[cfg(windows)]
async fn install(app: AppHandle) -> Result<()> {
    use futures_util::StreamExt;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tauri::Emitter;
    use tokio::io::AsyncWriteExt;

    let repo = repo();
    if !valid_repo(&repo) {
        return Err(anyhow!("The update source is invalid"));
    }
    let rel = latest_release(&repo)
        .await?
        .ok_or_else(|| anyhow!("No release has been published yet"))?;
    let (name, url, size) =
        exe_asset(&rel).ok_or_else(|| anyhow!("The release doesn't contain an .exe file"))?;
    let prefix = format!("https://github.com/{repo}/releases/download/");
    if !url.starts_with(&prefix) {
        return Err(anyhow!("Unexpected download address"));
    }
    let sha_name = format!("{name}.sha256");
    let sha_url = rel["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(sha_name.as_str())))
        .and_then(|x| x["browser_download_url"].as_str())
        .map(String::from);
    let want_sha = match sha_url {
        Some(u) if u.starts_with(&prefix) => Some(
            get_text(&u)
                .await?
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_lowercase(),
        ),
        _ => None,
    };

    let cur = std::env::current_exe()?;
    let new_path = std::path::PathBuf::from(format!("{}.new", cur.display()));
    let resp = http().get(&url).send().await?.error_for_status()?;
    let total = resp.content_length().unwrap_or(size);
    let mut file = tokio::fs::File::create(&new_path).await?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    let mut last_pct: u64 = 101;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        let pct = if total > 0 { done * 100 / total } else { 0 };
        if pct != last_pct {
            last_pct = pct;
            let _ = app.emit("update-progress", json!({ "done": done, "total": total }));
        }
    }
    file.flush().await?;
    drop(file);

    if done < 1_000_000 {
        let _ = tokio::fs::remove_file(&new_path).await;
        return Err(anyhow!("The download was incomplete. Please try again."));
    }
    if let Some(w) = want_sha {
        if !w.is_empty() && hex::encode(hasher.finalize()) != w {
            let _ = tokio::fs::remove_file(&new_path).await;
            return Err(anyhow!("The downloaded update is damaged. Please try again."));
        }
    }

    // Windows lets a running .exe be renamed (not overwritten), so: current -> .old, new -> current.
    let old = std::path::PathBuf::from(format!("{}.old", cur.display()));
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&cur, &old)?;
    if let Err(e) = std::fs::rename(&new_path, &cur) {
        let _ = std::fs::rename(&old, &cur);
        return Err(anyhow!("Couldn't install the update: {e}"));
    }
    std::process::Command::new(&cur).spawn()?;
    app.exit(0);
    Ok(())
}

/// Removes leftovers of a previous update.
pub fn cleanup_old() {
    if let Ok(cur) = std::env::current_exe() {
        for ext in ["old", "new"] {
            let _ = std::fs::remove_file(format!("{}.{}", cur.display(), ext));
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub text: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Deserialize)]
struct NewsFile {
    #[serde(default)]
    items: Vec<NewsItem>,
}

/// Announcements for players: the `news.json` file at the root of your GitHub repo.
/// Editing that file on GitHub updates every launcher — no new build needed.
#[tauri::command]
pub async fn news_get() -> Result<Vec<NewsItem>, String> {
    let run = async {
        let repo = repo();
        if !valid_repo(&repo) {
            return Err(anyhow!("The update source is invalid"));
        }
        let f: NewsFile =
            get_json(&format!("https://raw.githubusercontent.com/{repo}/main/news.json")).await?;
        Ok::<Vec<NewsItem>, anyhow::Error>(
            f.items
                .into_iter()
                .take(3)
                .map(|mut n| {
                    if !n.url.starts_with("https://") {
                        n.url.clear();
                    }
                    n.title = n.title.chars().take(80).collect();
                    n.text = n.text.chars().take(300).collect();
                    n
                })
                .collect(),
        )
    };
    run.await.map_err(es)
}
