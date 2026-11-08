use crate::instances::valid_id;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Serialize, Clone, Debug)]
pub struct Finding {
    pub title: String,
    pub advice: String,
}

fn tail_file(p: &Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len > max {
        let _ = f.seek(SeekFrom::Start(len - max));
    }
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

fn newest_crash_report(id: &str) -> Option<PathBuf> {
    let dir = game_dir(id).join("crash-reports");
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let m = e.metadata().ok()?;
        let t = m.modified().ok()?;
        if best.as_ref().map(|(bt, _)| t > *bt).unwrap_or(true) {
            best = Some((t, e.path()));
        }
    }
    let (t, p) = best?;
    // only reports from the last 15 minutes are relevant to the run that just ended
    if SystemTime::now()
        .duration_since(t)
        .map(|d| d < Duration::from_secs(900))
        .unwrap_or(false)
    {
        Some(p)
    } else {
        None
    }
}

/// latest.log (tail) plus a fresh crash report, if any.
pub fn collect(id: &str) -> String {
    let mut text = tail_file(&game_dir(id).join("logs").join("latest.log"), 1_000_000);
    if let Some(p) = newest_crash_report(id) {
        text.push('\n');
        text.push_str(&tail_file(&p, 200_000));
    }
    text
}

fn lines_with(text: &str, needles: &[&str], max: usize) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for l in text.lines() {
        let t = l.trim();
        if needles.iter().any(|n| t.contains(n)) && !out.iter().any(|o| o == t) {
            out.push(t.chars().take(220).collect());
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

fn push(f: &mut Vec<Finding>, t: &str, a: String) {
    f.push(Finding {
        title: t.to_string(),
        advice: a,
    });
}

pub fn analyze_text(text: &str) -> Vec<Finding> {
    let low = text.to_lowercase();
    let has = |s: &str| low.contains(&s.to_lowercase());
    let mut f: Vec<Finding> = vec![];

    if has("outofmemoryerror") || has("java heap space") {
        push(&mut f, "Out of memory", "Give the instance more RAM in Settings (4–8 GB is typical for modded), remove heavy mods, or lower render distance.".into());
    }
    if has("unsupportedclassversionerror") || has("compiled by a more recent version of the java runtime") {
        push(&mut f, "Wrong Java version", "A mod or the game needs a newer Java. Clear the Java path in the instance settings so Nimbus uses the matching managed runtime.".into());
    }
    if has("could not find or load main class") || has("classnotfoundexception: net.minecraft.client.main.main") {
        push(&mut f, "Damaged installation", "Game files look incomplete. Use “Repair” in the instance settings.".into());
    }
    if has("glfw error") || has("failed to create window") || has("pixel format not accelerated") || has("wgl: the driver does not appear to support opengl") {
        push(&mut f, "Graphics driver problem", "Update your graphics driver. On laptops make sure Java runs on the dedicated GPU.".into());
    }
    if has("duplicatemodsfoundexception") || has("found duplicate mods") || has("duplicate mod") {
        push(&mut f, "Duplicate mods", "Two copies of the same mod are installed. Remove one in the Mods tab.".into());
    }
    let dep = lines_with(
        text,
        &["requires", "Unmet dependency", "Missing or unsupported mandatory dependencies", "depends on"],
        6,
    );
    if has("incompatible mod set") || has("missing or unsupported mandatory dependencies") || has("unmet dependency") || has("mod resolution encountered") {
        push(&mut f, "Mod dependency problem", format!("A mod is missing a dependency or doesn't match your Minecraft/loader version.\n{}", dep.join("\n")));
    }
    if has("mixin apply failed") || has("mixinapplyerror") || has("invalidinjectionexception") || has("mixintransformererror") {
        let who = lines_with(text, &["Mixin apply for mod", "mixin config"], 3);
        push(&mut f, "Mod conflict (Mixin)", format!("Two mods clash or one is built for another version. Disable recently added mods one by one.\n{}", who.join("\n")));
    }
    if has("nosuchmethoderror") || has("nosuchfielderror") || has("noclassdeffounderror") {
        push(&mut f, "Mod version mismatch", "A mod was built for a different Minecraft or loader version. Update the mods (Mods tab → Check for updates) or remove the newest one.".into());
    }
    if has("unknownhostexception") || has("connection timed out") || has("failed to download") {
        push(&mut f, "Network problem", "Something couldn't be downloaded. Check your connection/VPN and try again.".into());
    }
    if f.is_empty() {
        push(&mut f, "No known cause found", "Use “Share log” in the Console tab to get a link you can send to the mod authors or a support community.".into());
    }
    f
}

pub fn analyze_instance(id: &str) -> Vec<Finding> {
    analyze_text(&collect(id))
}

#[tauri::command]
pub fn crash_analyze(instance_id: String) -> Result<Vec<Finding>, String> {
    if !valid_id(&instance_id) {
        return Err("invalid instance".into());
    }
    Ok(analyze_instance(&instance_id))
}

/// Uploads the latest log to mclo.gs (user-triggered, after a confirmation in the UI).
#[tauri::command]
pub async fn log_share(instance_id: String) -> Result<String, String> {
    let run = async {
        if !valid_id(&instance_id) {
            return Err(anyhow!("invalid instance"));
        }
        let text = collect(&instance_id);
        if text.trim().is_empty() {
            return Err(anyhow!("No log found yet — launch the game first"));
        }
        let r: Value = http()
            .post("https://api.mclo.gs/1/log")
            .form(&[("content", text.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if r["success"] == true {
            Ok(r["url"].as_str().unwrap_or("").to_string())
        } else {
            Err(anyhow!("{}", r["error"].as_str().unwrap_or("upload failed")))
        }
    };
    run.await.map_err(es)
}
