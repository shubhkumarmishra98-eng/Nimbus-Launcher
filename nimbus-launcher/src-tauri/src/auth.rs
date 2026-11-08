use crate::instances::load_settings;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DEVICE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPE: &str = "XboxLive.signin offline_access";

static MS_CANCEL: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn ms_login_cancel() {
    MS_CANCEL.store(true, Ordering::Relaxed);
}

/// True when this build (or the user's settings) has a Microsoft client ID.
#[tauri::command]
pub fn ms_available() -> bool {
    client_id().is_ok()
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Account {
    pub id: String,
    pub kind: String, // "microsoft" | "offline"
    pub username: String,
    pub uuid: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub expires_at: u64,
}
#[derive(Serialize, Deserialize, Default)]
struct Store {
    accounts: Vec<Account>,
    selected: Option<String>,
}
#[derive(Serialize)]
pub struct AccountView {
    pub id: String,
    pub kind: String,
    pub username: String,
    pub uuid: String,
    pub selected: bool,
}
pub struct Session {
    pub username: String,
    pub uuid: String,
    pub token: String,
    pub user_type: String,
}

fn store_path() -> PathBuf {
    root().join("accounts.json")
}
fn load() -> Store {
    read_json(&store_path()).unwrap_or_default()
}
fn save(s: &Store) -> Result<()> {
    write_json(&store_path(), s)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(store_path(), std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
fn views(s: &Store) -> Vec<AccountView> {
    s.accounts
        .iter()
        .map(|a| AccountView {
            id: a.id.clone(),
            kind: a.kind.clone(),
            username: a.username.clone(),
            uuid: a.uuid.clone(),
            selected: s.selected.as_deref() == Some(a.id.as_str()),
        })
        .collect()
}

pub fn get_account(id: &str) -> Result<Account> {
    load()
        .accounts
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| anyhow!("Account not found"))
}

pub fn offline_uuid(name: &str) -> String {
    let mut h = md5::compute(format!("OfflinePlayer:{name}")).0;
    h[6] = (h[6] & 0x0f) | 0x30;
    h[8] = (h[8] & 0x3f) | 0x80;
    hex::encode(h)
}

#[tauri::command]
pub fn accounts_list() -> Vec<AccountView> {
    views(&load())
}

#[tauri::command]
pub fn account_add_offline(username: String) -> Result<Vec<AccountView>, String> {
    let name = username.trim().to_string();
    if name.len() < 3
        || name.len() > 16
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("Username must be 3–16 characters: letters, numbers or underscore".into());
    }
    let mut s = load();
    let id = format!("off-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
    s.accounts.push(Account {
        id: id.clone(),
        kind: "offline".into(),
        username: name.clone(),
        uuid: offline_uuid(&name),
        ..Default::default()
    });
    if s.selected.is_none() {
        s.selected = Some(id);
    }
    save(&s).map_err(es)?;
    Ok(views(&s))
}

#[tauri::command]
pub fn account_select(id: String) -> Result<Vec<AccountView>, String> {
    let mut s = load();
    if s.accounts.iter().any(|a| a.id == id) {
        s.selected = Some(id);
        save(&s).map_err(es)?;
    }
    Ok(views(&s))
}

#[tauri::command]
pub fn account_remove(id: String) -> Result<Vec<AccountView>, String> {
    let mut s = load();
    s.accounts.retain(|a| a.id != id);
    if s.selected.as_deref() == Some(id.as_str()) {
        s.selected = s.accounts.first().map(|a| a.id.clone());
    }
    save(&s).map_err(es)?;
    let _ = std::fs::remove_file(skins_dir().join(format!("{id}.png")));
    let _ = std::fs::remove_file(skins_dir().join(format!("{id}_cape.png")));
    Ok(views(&s))
}

// ---------------- Microsoft sign-in (device code flow) ----------------
#[derive(Serialize)]
pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
    pub device_code: String,
    pub interval: u64,
    pub expires_in: u64,
}

fn client_id() -> Result<String> {
    // 1) a client ID typed into Settings wins, 2) otherwise the one embedded at build time
    let c = load_settings().ms_client_id.trim().to_string();
    if !c.is_empty() {
        return Ok(c);
    }
    if let Some(b) = option_env!("NIMBUS_MS_CLIENT_ID") {
        if !b.trim().is_empty() {
            return Ok(b.trim().to_string());
        }
    }
    Err(anyhow!(
        "Microsoft sign-in isn't configured in this build. Use an offline account, or add an Azure client ID in Settings (see the README)."
    ))
}

#[tauri::command]
pub async fn ms_login_start() -> Result<DeviceCode, String> {
    async fn run() -> Result<DeviceCode> {
        MS_CANCEL.store(false, Ordering::Relaxed);
        let cid = client_id()?;
        let r = http()
            .post(DEVICE_URL)
            .form(&[("client_id", cid.as_str()), ("scope", SCOPE)])
            .send()
            .await?;
        let ok = r.status().is_success();
        let v: Value = r.json().await?;
        if !ok {
            return Err(anyhow!(
                "{}",
                v["error_description"].as_str().unwrap_or("could not start sign-in")
            ));
        }
        Ok(DeviceCode {
            user_code: v["user_code"].as_str().unwrap_or("").to_string(),
            verification_uri: v["verification_uri"]
                .as_str()
                .unwrap_or("https://microsoft.com/link")
                .to_string(),
            device_code: v["device_code"].as_str().unwrap_or("").to_string(),
            interval: v["interval"].as_u64().unwrap_or(5),
            expires_in: v["expires_in"].as_u64().unwrap_or(900),
        })
    }
    run().await.map_err(es)
}

#[tauri::command]
pub async fn ms_login_poll(
    device_code: String,
    interval: u64,
    expires_in: u64,
) -> Result<Vec<AccountView>, String> {
    async fn run(device_code: String, mut interval: u64, expires_in: u64) -> Result<Vec<AccountView>> {
        let cid = client_id()?;
        let start = Instant::now();
        let tokens: Value = loop {
            if MS_CANCEL.load(Ordering::Relaxed) {
                return Err(anyhow!("Sign-in cancelled"));
            }
            if start.elapsed() > Duration::from_secs(expires_in) {
                return Err(anyhow!("Sign-in timed out. Please try again."));
            }
            tokio::time::sleep(Duration::from_secs(interval.max(1))).await;
            let r = http()
                .post(TOKEN_URL)
                .form(&[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", cid.as_str()),
                    ("device_code", device_code.as_str()),
                ])
                .send()
                .await?;
            let ok = r.status().is_success();
            let v: Value = r.json().await?;
            if ok {
                break v;
            }
            match v["error"].as_str() {
                Some("authorization_pending") => continue,
                Some("slow_down") => {
                    interval += 5;
                    continue;
                }
                Some(e) => {
                    return Err(anyhow!(
                        "{e}: {}",
                        v["error_description"].as_str().unwrap_or("")
                    ))
                }
                None => return Err(anyhow!("Unknown sign-in error")),
            }
        };
        let ms = tokens["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("no access token"))?;
        let refresh = tokens["refresh_token"].as_str().unwrap_or("").to_string();
        let mc = chain(ms).await?;
        let mut s = load();
        s.accounts.retain(|a| a.uuid != mc.uuid);
        let id = mc.uuid.clone();
        s.accounts.push(Account {
            id: id.clone(),
            kind: "microsoft".into(),
            username: mc.name,
            uuid: mc.uuid,
            access_token: mc.token,
            refresh_token: refresh,
            expires_at: now_secs() + mc.expires_in,
        });
        s.selected = Some(id);
        save(&s)?;
        Ok(views(&s))
    }
    run(device_code, interval, expires_in).await.map_err(es)
}

struct McLogin {
    token: String,
    expires_in: u64,
    uuid: String,
    name: String,
}

async fn post_json(url: &str, body: &Value) -> Result<Value> {
    let r = http()
        .post(url)
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await?;
    let st = r.status();
    let txt = r.text().await?;
    if !st.is_success() {
        let short: String = txt.chars().take(300).collect();
        return Err(anyhow!("{url} -> {st}: {short}"));
    }
    Ok(serde_json::from_str(&txt)?)
}

async fn chain(ms_access: &str) -> Result<McLogin> {
    let xbl = post_json(
        "https://user.auth.xboxlive.com/user/authenticate",
        &json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}")
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }),
    )
    .await?;
    let xbl_token = xbl["Token"]
        .as_str()
        .ok_or_else(|| anyhow!("no Xbox Live token"))?
        .to_string();
    let uhs = xbl["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .ok_or_else(|| anyhow!("no Xbox user hash"))?
        .to_string();

    let xsts = post_json(
        "https://xsts.auth.xboxlive.com/xsts/authorize",
        &json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }),
    )
    .await
    .map_err(|e| {
        let s = e.to_string();
        if s.contains("2148916233") {
            anyhow!("This Microsoft account has no Xbox profile yet. Sign in once at xbox.com to create one.")
        } else if s.contains("2148916238") {
            anyhow!("This is a child account. An adult must add it to a Microsoft family first.")
        } else {
            e
        }
    })?;
    let xsts_token = xsts["Token"]
        .as_str()
        .ok_or_else(|| anyhow!("no XSTS token"))?
        .to_string();

    let mc = post_json(
        "https://api.minecraftservices.com/authentication/login_with_xbox",
        &json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") }),
    )
    .await?;
    let token = mc["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("no Minecraft token"))?
        .to_string();
    let expires_in = mc["expires_in"].as_u64().unwrap_or(86400);

    let r = http()
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&token)
        .send()
        .await?;
    if r.status().as_u16() == 404 {
        return Err(anyhow!("This account doesn't own Minecraft: Java Edition."));
    }
    let p: Value = r.error_for_status()?.json().await?;
    Ok(McLogin {
        token,
        expires_in,
        uuid: p["id"].as_str().unwrap_or("").to_string(),
        name: p["name"].as_str().unwrap_or("Player").to_string(),
    })
}

/// A ready-to-use session. Refreshes Microsoft tokens when they are about to expire.
pub async fn fresh_session(id: &str) -> Result<Session> {
    let acc = get_account(id)?;
    if acc.kind == "offline" {
        return Ok(Session {
            username: acc.username,
            uuid: acc.uuid,
            token: "0".into(),
            user_type: "legacy".into(),
        });
    }
    if acc.expires_at > now_secs() + 120 && !acc.access_token.is_empty() {
        return Ok(Session {
            username: acc.username,
            uuid: acc.uuid,
            token: acc.access_token,
            user_type: "msa".into(),
        });
    }
    let cid = client_id()?;
    let r = http()
        .post(TOKEN_URL)
        .form(&[
            ("client_id", cid.as_str()),
            ("grant_type", "refresh_token"),
            ("refresh_token", acc.refresh_token.as_str()),
            ("scope", SCOPE),
        ])
        .send()
        .await?;
    let ok = r.status().is_success();
    let v: Value = r.json().await?;
    if !ok {
        return Err(anyhow!(
            "Your Microsoft session expired. Please sign in again ({}).",
            v["error"].as_str().unwrap_or("error")
        ));
    }
    let ms = v["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("no access token"))?;
    let new_refresh = v["refresh_token"]
        .as_str()
        .unwrap_or(acc.refresh_token.as_str())
        .to_string();
    let mc = chain(ms).await?;
    let mut st = load();
    if let Some(a) = st.accounts.iter_mut().find(|a| a.id == id) {
        a.access_token = mc.token.clone();
        a.refresh_token = new_refresh;
        a.expires_at = now_secs() + mc.expires_in;
        a.username = mc.name.clone();
        a.uuid = mc.uuid.clone();
    }
    save(&st)?;
    Ok(Session {
        username: mc.name,
        uuid: mc.uuid,
        token: mc.token,
        user_type: "msa".into(),
    })
}
