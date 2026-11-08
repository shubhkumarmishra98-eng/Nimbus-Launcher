use crate::instances::{load_settings, Instance};
use crate::mc;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::AppHandle;

fn mc_parts(v: &str) -> Vec<u32> {
    v.split('.')
        .map(|p| {
            p.split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or("0")
                .parse()
                .unwrap_or(0)
        })
        .collect()
}

pub async fn versions_for(loader: &str, mcv: &str) -> Result<Vec<String>> {
    match loader {
        "fabric" | "quilt" => {
            let url = if loader == "fabric" {
                format!("https://meta.fabricmc.net/v2/versions/loader/{mcv}")
            } else {
                format!("https://meta.quiltmc.org/v3/versions/loader/{mcv}")
            };
            let v: Vec<Value> = get_json(&url).await?;
            Ok(v.iter()
                .filter_map(|x| x["loader"]["version"].as_str().map(String::from))
                .collect())
        }
        "forge" => {
            let v: Value = get_json(
                "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json",
            )
            .await?;
            let mut out: Vec<String> = vec![];
            for k in ["recommended", "latest"] {
                if let Some(s) = v["promos"][format!("{mcv}-{k}")].as_str() {
                    if !out.iter().any(|x| x == s) {
                        out.push(s.to_string());
                    }
                }
            }
            Ok(out)
        }
        "neoforge" => {
            let p = mc_parts(mcv);
            if mcv == "1.20.1" {
                // the first NeoForge builds were published as net.neoforged:forge
                let v: Value = get_json(
                    "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/forge",
                )
                .await?;
                let mut list: Vec<String> = v["versions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str())
                    .filter(|s| s.starts_with("1.20.1-"))
                    .map(String::from)
                    .collect();
                list.reverse();
                return Ok(list);
            }
            if p.len() < 2 || p[1] < 20 || (p[1] == 20 && p.get(2).copied().unwrap_or(0) < 2) {
                return Ok(vec![]);
            }
            let prefix = format!("{}.{}.", p[1], p.get(2).copied().unwrap_or(0));
            let v: Value = get_json(
                "https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge",
            )
            .await?;
            let mut list: Vec<String> = v["versions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x.as_str())
                .filter(|s| s.starts_with(&prefix))
                .map(String::from)
                .collect();
            list.reverse();
            list.sort_by_key(|s| s.contains("beta"));
            Ok(list)
        }
        _ => Ok(vec![]),
    }
}

#[tauri::command]
pub async fn loader_versions(loader: String, mc_version: String) -> Result<Vec<String>, String> {
    versions_for(&loader, &mc_version).await.map_err(es)
}

async fn install_profile(url: &str) -> Result<String> {
    let txt = get_text(url).await?;
    let v: Value = serde_json::from_str(&txt)?;
    let id = v["id"]
        .as_str()
        .ok_or_else(|| anyhow!("loader profile has no id"))?
        .to_string();
    let dir = versions().join(&id);
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join(format!("{id}.json")), txt).await?;
    Ok(id)
}

fn version_exists(id: &str) -> bool {
    versions().join(id).join(format!("{id}.json")).exists()
}

pub async fn ensure_loader(app: &AppHandle, inst: &mut Instance) -> Result<String> {
    if inst.loader_version.is_empty() {
        let v = versions_for(&inst.loader, &inst.mc_version).await?;
        inst.loader_version = v.into_iter().next().ok_or_else(|| {
            anyhow!(
                "No {} build is available for Minecraft {}",
                inst.loader,
                inst.mc_version
            )
        })?;
    }
    if !inst.resolved_id.is_empty() && version_exists(&inst.resolved_id) {
        return Ok(inst.resolved_id.clone());
    }
    let lv = inst.loader_version.clone();
    let mcv = inst.mc_version.clone();
    let task = inst.id.clone();
    match inst.loader.as_str() {
        "fabric" => {
            let id = format!("fabric-loader-{lv}-{mcv}");
            if version_exists(&id) {
                return Ok(id);
            }
            install_profile(&format!(
                "https://meta.fabricmc.net/v2/versions/loader/{mcv}/{lv}/profile/json"
            ))
            .await
        }
        "quilt" => {
            let id = format!("quilt-loader-{lv}-{mcv}");
            if version_exists(&id) {
                return Ok(id);
            }
            install_profile(&format!(
                "https://meta.quiltmc.org/v3/versions/loader/{mcv}/{lv}/profile/json"
            ))
            .await
        }
        "forge" => {
            if mc_parts(&mcv).get(1).copied().unwrap_or(0) < 13 {
                return install_legacy_forge(app, &task, &mcv, &lv).await;
            }
            let mut id = format!("{mcv}-forge-{lv}");
            if !version_exists(&id) {
                let url = format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{mcv}-{lv}/forge-{mcv}-{lv}-installer.jar");
                id = run_installer(app, &task, &mcv, &id, &lv, &url, "Forge").await?;
            }
            Ok(id)
        }
        "neoforge" => {
            let mut id = format!("neoforge-{lv}");
            if !version_exists(&id) {
                let url = if mcv == "1.20.1" {
                    format!("https://maven.neoforged.net/releases/net/neoforged/forge/{lv}/forge-{lv}-installer.jar")
                } else {
                    format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{lv}/neoforge-{lv}-installer.jar")
                };
                id = run_installer(app, &task, &mcv, &id, &lv, &url, "NeoForge").await?;
            }
            Ok(id)
        }
        other => Err(anyhow!("unknown loader {other}")),
    }
}

fn version_dirs() -> std::collections::HashSet<String> {
    std::fs::read_dir(versions())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Runs a Forge/NeoForge installer and returns the id of the version it created.
async fn run_installer(
    app: &AppHandle,
    task: &str,
    mcv: &str,
    expect_id: &str,
    hint: &str,
    url: &str,
    label: &str,
) -> Result<String> {
    progress(app, task, &format!("Downloading {label} installer"), 0, 0);
    let file = url.rsplit('/').next().unwrap_or("installer.jar");
    let jar = cache_dir().join("installers").join(file);
    download_file(url, &jar, None, None).await?;

    let vanilla = mc::load_version(mcv)?;
    let java = mc::ensure_java(app, &vanilla, task, &load_settings().java_path).await?;

    progress(
        app,
        task,
        &format!("Running {label} installer (this can take a minute)"),
        0,
        0,
    );
    let shared = shared();
    let lp = shared.join("launcher_profiles.json");
    if !lp.exists() {
        std::fs::write(&lp, r#"{"profiles":{},"settings":{},"version":3}"#)?;
    }
    let before = version_dirs();
    run_java_installer(&java, &jar, &shared).await?;
    let created: Vec<String> = version_dirs()
        .into_iter()
        .filter(|d| !before.contains(d))
        .collect();
    created
        .iter()
        .find(|d| d.contains(hint))
        .cloned()
        .or_else(|| {
            if version_exists(expect_id) {
                Some(expect_id.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("{label} installer finished but no new version was created"))
}

async fn run_java_installer(java: &Path, jar: &Path, dir: &Path) -> Result<()> {
    let mut cmd = tokio::process::Command::new(java);
    cmd.arg("-jar")
        .arg(jar)
        .arg("--installClient")
        .arg(dir)
        .current_dir(dir);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = cmd.output().await?;
    if !out.status.success() {
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let tail: Vec<&str> = text.lines().rev().take(12).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(anyhow!("Installer failed:\n{}", tail.join("\n")));
    }
    Ok(())
}

/// Forge for Minecraft 1.5–1.12: the installer jar carries the version profile and the Forge jar itself.
async fn install_legacy_forge(app: &AppHandle, task: &str, mcv: &str, lv: &str) -> Result<String> {
    progress(app, task, "Downloading Forge installer", 0, 0);
    let mut cands = vec![format!("{mcv}-{lv}"), format!("{mcv}-{lv}-{mcv}")];
    if mcv == "1.7.10" {
        cands.reverse();
    }
    let mut jar: Option<PathBuf> = None;
    let mut last: Option<anyhow::Error> = None;
    for c in cands {
        let url = format!(
            "https://maven.minecraftforge.net/net/minecraftforge/forge/{c}/forge-{c}-installer.jar"
        );
        let dest = cache_dir()
            .join("installers")
            .join(format!("forge-{c}-installer.jar"));
        match download_file(&url, &dest, None, None).await {
            Ok(()) => {
                jar = Some(dest);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let jar = jar.ok_or_else(|| last.unwrap_or_else(|| anyhow!("Forge installer not found")))?;
    let mcv = mcv.to_string();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let f = std::fs::File::open(&jar)?;
        let mut z = zip::ZipArchive::new(f)?;
        let prof: Value = {
            let mut e = z.by_name("install_profile.json")?;
            let mut s = String::new();
            e.read_to_string(&mut s)?;
            let v: Value = serde_json::from_str(&s)?;
            v
        };
        let mut vi = prof
            .get("versionInfo")
            .ok_or_else(|| anyhow!("Unsupported Forge installer layout"))?
            .clone();
        vi["inheritsFrom"] = Value::String(mcv);
        let id = vi["id"]
            .as_str()
            .ok_or_else(|| anyhow!("Forge profile has no id"))?
            .to_string();
        let dir = versions().join(&id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(format!("{id}.json")), serde_json::to_vec_pretty(&vi)?)?;
        if let (Some(path), Some(file)) = (
            prof["install"]["path"].as_str(),
            prof["install"]["filePath"].as_str(),
        ) {
            let mut e = z.by_name(file)?;
            let dest = libraries().join(maven_path(path));
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut o = std::fs::File::create(dest)?;
            std::io::copy(&mut e, &mut o)?;
        }
        Ok(id)
    })
    .await?
}
