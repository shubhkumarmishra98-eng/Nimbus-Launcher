use crate::auth;
use crate::instances::Instance;
use crate::util::*;
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

const API: &str = "https://api.minecraftservices.com/minecraft/profile";

fn png_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
    Some((w, h))
}

fn safe_account_id(id: &str) -> Result<()> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(anyhow!("invalid account id"));
    }
    Ok(())
}

async fn ms_token(account_id: &str) -> Result<String> {
    let acc = auth::get_account(account_id)?;
    if acc.kind != "microsoft" {
        return Err(anyhow!("This needs a Microsoft account"));
    }
    Ok(auth::fresh_session(account_id).await?.token)
}

// ---------------- premium accounts ----------------
#[tauri::command]
pub async fn skin_profile(account_id: String) -> Result<Value, String> {
    let run = async {
        let t = ms_token(&account_id).await?;
        let r = http()
            .get(API)
            .bearer_auth(t)
            .send()
            .await?
            .error_for_status()?;
        Ok::<Value, anyhow::Error>(r.json().await?)
    };
    run.await.map_err(es)
}

#[tauri::command]
pub async fn skin_upload(account_id: String, path: String, variant: String) -> Result<Value, String> {
    let run = async {
        let variant = if variant == "slim" { "slim" } else { "classic" };
        let bytes = std::fs::read(&path)?;
        match png_dims(&bytes) {
            Some((64, 64)) => {}
            _ => return Err(anyhow!("Skin must be a 64×64 PNG")),
        }
        let t = ms_token(&account_id).await?;
        let form = reqwest::multipart::Form::new()
            .text("variant", variant.to_string())
            .part(
                "file",
                reqwest::multipart::Part::bytes(bytes)
                    .file_name("skin.png")
                    .mime_str("image/png")?,
            );
        let r = http()
            .post(format!("{API}/skins"))
            .bearer_auth(t)
            .multipart(form)
            .send()
            .await?;
        if !r.status().is_success() {
            let txt = r.text().await.unwrap_or_default();
            return Err(anyhow!("Upload failed: {txt}"));
        }
        Ok::<Value, anyhow::Error>(r.json().await?)
    };
    run.await.map_err(es)
}

#[tauri::command]
pub async fn cape_set(account_id: String, cape_id: Option<String>) -> Result<Value, String> {
    let run = async {
        let t = ms_token(&account_id).await?;
        let url = format!("{API}/capes/active");
        let r = match cape_id {
            Some(id) => {
                http()
                    .put(&url)
                    .bearer_auth(&t)
                    .json(&json!({ "capeId": id }))
                    .send()
                    .await?
            }
            None => http().delete(&url).bearer_auth(&t).send().await?,
        };
        if !r.status().is_success() {
            let txt = r.text().await.unwrap_or_default();
            return Err(anyhow!("Cape change failed: {txt}"));
        }
        Ok::<Value, anyhow::Error>(r.json().await.unwrap_or(Value::Null))
    };
    run.await.map_err(es)
}

/// Fetches a skin/cape texture (only from Mojang's texture host) as a data URL.
#[tauri::command]
pub async fn skin_image(url: String) -> Result<String, String> {
    let run = async {
        let u = url.replacen("http://", "https://", 1);
        if !u.starts_with("https://textures.minecraft.net/") {
            return Err(anyhow!("untrusted skin host"));
        }
        let b = http().get(&u).send().await?.error_for_status()?.bytes().await?;
        Ok::<String, anyhow::Error>(format!("data:image/png;base64,{}", B64.encode(&b)))
    };
    run.await.map_err(es)
}

// ---------------- offline accounts ----------------
/// (skin file, cape file) for an offline account. Files are keyed by the player name, so a skin
/// survives removing and re-adding the account. Older builds keyed them by the random account id.
fn paths(account_id: &str) -> Result<(PathBuf, PathBuf)> {
    safe_account_id(account_id)?;
    let acc = auth::get_account(account_id)?;
    if acc.kind != "offline" {
        return Err(anyhow!("Local skins are for offline accounts"));
    }
    let key = acc.username.to_lowercase();
    let skin = skins_dir().join(format!("skin-{key}.png"));
    let cape = skins_dir().join(format!("cape-{key}.png"));
    let old_skin = skins_dir().join(format!("{account_id}.png"));
    let old_cape = skins_dir().join(format!("{account_id}_cape.png"));
    if !skin.exists() && old_skin.exists() {
        let _ = std::fs::rename(&old_skin, &skin);
    }
    if !cape.exists() && old_cape.exists() {
        let _ = std::fs::rename(&old_cape, &cape);
    }
    Ok((skin, cape))
}

/// `Some("")` removes the file, `Some(path)` replaces it, `None` leaves it alone.
#[tauri::command]
pub fn offline_skin_set(
    account_id: String,
    skin_path: Option<String>,
    cape_path: Option<String>,
) -> Result<(), String> {
    let run = || -> Result<()> {
        let (skin_file, cape_file) = paths(&account_id)?;
        std::fs::create_dir_all(skins_dir())?;
        if let Some(p) = skin_path {
            if p.is_empty() {
                let _ = std::fs::remove_file(&skin_file);
            } else {
                let b = std::fs::read(&p)?;
                match png_dims(&b) {
                    Some((64, 64)) | Some((64, 32)) => {}
                    _ => return Err(anyhow!("Skin must be a 64×64 (or 64×32) PNG")),
                }
                std::fs::write(&skin_file, b)?;
            }
        }
        if let Some(p) = cape_path {
            if p.is_empty() {
                let _ = std::fs::remove_file(&cape_file);
            } else {
                let b = std::fs::read(&p)?;
                if png_dims(&b).is_none() {
                    return Err(anyhow!("Cape must be a PNG file"));
                }
                std::fs::write(&cape_file, b)?;
            }
        }
        Ok(())
    };
    run().map_err(es)
}

#[derive(Serialize)]
pub struct SkinData {
    pub skin: Option<String>,
    pub cape: Option<String>,
}

fn data_url(p: &PathBuf) -> Option<String> {
    std::fs::read(p)
        .ok()
        .map(|b| format!("data:image/png;base64,{}", B64.encode(b)))
}

#[tauri::command]
pub fn offline_skin_get(account_id: String) -> Result<SkinData, String> {
    let (skin, cape) = paths(&account_id).map_err(es)?;
    Ok(SkinData {
        skin: data_url(&skin),
        cape: data_url(&cape),
    })
}

/// Offline mode has no skin server, so the launcher feeds the CustomSkinLoader mod a local skin/cape
/// (and installs that mod for the instance if needed). Returns a message when the skin can't show here.
pub async fn sync_offline_skin(
    inst: &Instance,
    account_id: &str,
    username: &str,
) -> Result<Option<String>> {
    let (skin, cape) = paths(account_id)?;
    if !skin.exists() && !cape.exists() {
        return Ok(None);
    }
    if inst.loader == "vanilla" {
        return Ok(Some(
            "Your local skin/cape only shows on instances with a mod loader (Fabric, Quilt, Forge or NeoForge). This instance is vanilla.".to_string(),
        ));
    }
    let base = game_dir(&inst.id).join("CustomSkinLoader").join("LocalSkin");
    if skin.exists() {
        std::fs::create_dir_all(base.join("skins"))?;
        std::fs::copy(&skin, base.join("skins").join(format!("{username}.png")))?;
    }
    if cape.exists() {
        std::fs::create_dir_all(base.join("capes"))?;
        std::fs::copy(&cape, base.join("capes").join(format!("{username}.png")))?;
    }
    crate::mods::ensure_by_slug(inst, "customskinloader").await?;
    Ok(None)
}
