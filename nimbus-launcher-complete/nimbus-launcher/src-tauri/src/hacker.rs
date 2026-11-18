//! "Hacker Fabric" instances: Fabric + the LiquidBounce client (a "hacked client" mod).
//!
//! Nothing is bundled: the launcher downloads Fabric API (and, if available, Fabric Language Kotlin) from
//! Modrinth and the LiquidBounce jar from the project's GitHub releases at install time.
//! LiquidBounce is GPL-3.0 software by CCBlueX.
use crate::instances::{load_settings, Instance};
use crate::mods;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::path::Path;

const REPO: &str = "CCBlueX/LiquidBounce";

fn jar_name_from_url(url: &str) -> Result<String> {
    let last = url
        .split('?')
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or("");
    let name: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._-+".contains(*c))
        .collect();
    if name.to_lowercase().ends_with(".jar") {
        Ok(name)
    } else {
        Err(anyhow!("The download link must point to a .jar file"))
    }
}

/// Does `hay` contain the version `tok` as a whole version (not as part of e.g. "26.31" or "1.26.3")?
fn token_in(hay: &str, tok: &str) -> bool {
    if tok.is_empty() {
        return false;
    }
    let b = hay.as_bytes();
    let mut start = 0;
    while let Some(i) = hay[start..].find(tok) {
        let s = start + i;
        let e = s + tok.len();
        let before_ok = s == 0 || !(b[s - 1].is_ascii_digit() || b[s - 1] == b'.');
        let after_ok = e >= b.len()
            || !(b[e].is_ascii_digit() || (b[e] == b'.' && e + 1 < b.len() && b[e + 1].is_ascii_digit()));
        if before_ok && after_ok {
            return true;
        }
        start = e;
    }
    false
}

/// Finds the newest LiquidBounce release jar made for Minecraft `mc` on GitHub.
async fn from_github(mc: &str) -> Result<(String, String)> {
    let rels: Vec<Value> =
        get_json(&format!("https://api.github.com/repos/{REPO}/releases?per_page=30")).await?;
    let mut seen: Vec<String> = vec![];
    let mut by_body: Option<(String, String)> = None;
    for r in rels
        .iter()
        .filter(|r| r["draft"] != true && r["prerelease"] != true)
    {
        let body = r["body"].as_str().unwrap_or("");
        for a in r["assets"].as_array().into_iter().flatten() {
            let name = a["name"].as_str().unwrap_or("");
            let l = name.to_lowercase();
            if !l.ends_with(".jar") || l.contains("sources") || l.contains("javadoc") || l.contains("-dev") {
                continue;
            }
            let Some(url) = a["browser_download_url"].as_str() else {
                continue;
            };
            if seen.len() < 12 {
                seen.push(name.to_string());
            }
            if token_in(name, mc) {
                return Ok((url.to_string(), name.to_string()));
            }
            if by_body.is_none() && token_in(body, mc) {
                by_body = Some((url.to_string(), name.to_string()));
            }
        }
    }
    by_body.ok_or_else(|| {
        anyhow!(
            "no LiquidBounce release on GitHub is labelled for Minecraft {mc} (jar files seen: {})",
            if seen.is_empty() { "none".to_string() } else { seen.join(", ") }
        )
    })
}

/// (download url, file name): a link typed into Settings wins, otherwise GitHub releases.
async fn resolve(mc: &str) -> Result<(String, String)> {
    let manual = load_settings().lb_url.trim().to_string();
    if !manual.is_empty() {
        if !manual.starts_with("https://") {
            return Err(anyhow!("The LiquidBounce download link must start with https://"));
        }
        let name = jar_name_from_url(&manual)?;
        return Ok((manual, name));
    }
    from_github(mc).await.map_err(|e| {
        anyhow!(
            "Couldn't find LiquidBounce for Minecraft {mc} automatically: {e:#}. You can paste a direct download link to the .jar in Settings → LiquidBounce download link."
        )
    })
}

fn remove_old(dir: &Path, keep: &str) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            let l = n.to_lowercase();
            if l.starts_with("liquidbounce") && n != keep && (l.ends_with(".jar") || l.ends_with(".jar.disabled")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Installs the libraries and the client jar into the instance's mods folder.
pub async fn install(inst: &Instance) -> Result<()> {
    mods::install_modrinth(inst, "fabric-api")
        .await
        .map_err(|e| anyhow!("Couldn't install Fabric API: {e:#}"))?;
    // newer LiquidBounce builds may bundle Kotlin; this is best-effort
    let _ = mods::install_modrinth(inst, "fabric-language-kotlin").await;
    let (url, name) = resolve(&inst.mc_version).await?;
    let dir = game_dir(&inst.id).join("mods");
    tokio::fs::create_dir_all(&dir).await?;
    download_file(&url, &dir.join(&name), None, None).await?;
    remove_old(&dir, &name);
    Ok(())
}
