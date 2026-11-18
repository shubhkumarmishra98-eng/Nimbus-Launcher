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
fn skin_file(id: &str) -> PathBuf {
    skins_dir().join(format!("{id}.png"))
}
fn cape_file(id: &str) -> PathBuf {
    skins_dir().join(format!("{id}_cape.png"))
}

/// `Some("")` removes the file, `Some(path)` replaces it, `None` leaves it alone.
#[tauri::command]
pub fn offline_skin_set(
    account_id: String,
    skin_path: Option<String>,
    cape_path: Option<String>,
) -> Result<(), String> {
    let run = || -> Result<()> {
        safe_account_id(&account_id)?;
        if auth::get_account(&account_id)?.kind != "offline" {
            return Err(anyhow!("Local skins are for offline accounts"));
        }
        std::fs::create_dir_all(skins_dir())?;
        if let Some(p) = skin_path {
            if p.is_empty() {
                let _ = std::fs::remove_file(skin_file(&account_id));
            } else {
                let b = std::fs::read(&p)?;
                match png_dims(&b) {
                    Some((64, 64)) | Some((64, 32)) => {}
                    _ => return Err(anyhow!("Skin must be a 64×64 (or 64×32) PNG")),
                }
                std::fs::write(skin_file(&account_id), b)?;
            }
        }
        if let Some(p) = cape_path {
            if p.is_empty() {
                let _ = std::fs::remove_file(cape_file(&account_id));
            } else {
                let b = std::fs::read(&p)?;
                if png_dims(&b).is_none() {
                    return Err(anyhow!("Cape must be a PNG file"));
                }
                std::fs::write(cape_file(&account_id), b)?;
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
    safe_account_id(&account_id).map_err(es)?;
    Ok(SkinData {
        skin: data_url(&skin_file(&account_id)),
        cape: data_url(&cape_file(&account_id)),
    })
}

/// Offline mode has no skin server, so the launcher feeds the CustomSkinLoader mod
/// a local skin/cape (and installs that mod for the instance if needed).
pub async fn sync_offline_skin(inst: &Instance, account_id: &str, username: &str) -> Result<()> {
    safe_account_id(account_id)?;
    let skin = skin_file(account_id);
    let cape = cape_file(account_id);
    if (!skin.exists() && !cape.exists()) || inst.loader == "vanilla" {
        return Ok(());
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
    crate::mods::ensure_by_slug(inst, "customskinloader").await
}
