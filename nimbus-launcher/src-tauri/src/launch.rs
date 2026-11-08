use crate::instances::*;
use crate::util::*;
use crate::{auth, loaders, mc, mods, skins};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

type RunMap = Mutex<HashMap<String, oneshot::Sender<()>>>;
static RUNNING: OnceLock<RunMap> = OnceLock::new();
fn running() -> &'static RunMap {
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}
pub fn is_running(id: &str) -> bool {
    running().lock().unwrap().contains_key(id)
}

struct CancelGuard;
impl Drop for CancelGuard {
    fn drop(&mut self) {
        set_cancel(false);
    }
}

#[tauri::command]
pub fn instance_cancel() {
    set_cancel(true);
}

#[derive(Clone, Serialize)]
struct StateEvt {
    id: String,
    state: String,
    code: Option<i32>,
}
#[derive(Clone, Serialize)]
struct CrashEvt {
    id: String,
    code: Option<i32>,
    findings: Vec<crate::crash::Finding>,
}
#[derive(Clone, Serialize)]
struct LogEvt {
    id: String,
    lines: Vec<String>,
}

fn emit_state(app: &AppHandle, id: &str, state: &str, code: Option<i32>) {
    let _ = app.emit(
        "game-state",
        StateEvt {
            id: id.to_string(),
            state: state.to_string(),
            code,
        },
    );
}

// ---------------- prepare (download everything) ----------------
fn stamp_path(id: &str) -> PathBuf {
    instance_dir(id).join("ready.stamp")
}
fn stamp_value(inst: &Instance) -> String {
    format!(
        "{}|{}|{}|{}",
        inst.resolved_id, inst.mc_version, inst.loader, inst.loader_version
    )
}
fn ready_stamp_ok(inst: &Instance) -> bool {
    !inst.resolved_id.is_empty()
        && mc::jar_path(&inst.mc_version).exists()
        && std::fs::read_to_string(stamp_path(&inst.id))
            .map(|s| s == stamp_value(inst))
            .unwrap_or(false)
}

async fn prepare(
    app: &AppHandle,
    inst: &mut Instance,
    st: &Settings,
) -> Result<(mc::VersionJson, PathBuf)> {
    let task = inst.id.clone();
    let _guard = CancelGuard;
    set_cancel(false);

    // Fast path: everything was fully downloaded/verified on an earlier run, so skip all
    // library/asset checks (thousands of files) and go straight to launching.
    if !is_verify() && ready_stamp_ok(inst) {
        if let Ok(v) = mc::load_version(&inst.resolved_id) {
            let jp = if inst.java_path.trim().is_empty() {
                st.java_path.clone()
            } else {
                inst.java_path.clone()
            };
            if let Ok(java) = mc::ensure_java(app, &v, &task, &jp).await {
                return Ok((v, java));
            }
        }
    }
    progress(app, &task, "Preparing", 0, 0);
    mc::ensure_vanilla(&inst.mc_version).await?;
    let vid = if inst.loader == "vanilla" {
        inst.mc_version.clone()
    } else {
        loaders::ensure_loader(app, inst).await?
    };
    inst.resolved_id = vid.clone();
    let natives = instance_dir(&inst.id).join("natives");
    let v = mc::install_game(app, &vid, &task, st.concurrency, &natives, &game_dir(&inst.id)).await?;
    let jp = if inst.java_path.trim().is_empty() {
        st.java_path.clone()
    } else {
        inst.java_path.clone()
    };
    let java = mc::ensure_java(app, &v, &task, &jp).await?;
    if !inst.installed {
        if inst.perf_pack && inst.loader != "vanilla" {
            progress(app, &task, "Installing performance mods", 0, 0);
            let _ = mods::install_perf_pack(inst).await;
        }
        inst.installed = true;
    }
    save_instance(inst)?;
    let _ = std::fs::write(stamp_path(&inst.id), stamp_value(inst));
    Ok((v, java))
}

#[tauri::command]
pub async fn instance_install(app: AppHandle, id: String) -> Result<Instance, String> {
    let run = async {
        let mut inst = load_instance(&id)?;
        let st = load_settings();
        prepare(&app, &mut inst, &st).await?;
        progress(&app, &id, "Ready", 1, 1);
        Ok::<Instance, anyhow::Error>(inst)
    };
    run.await.map_err(es)
}

// ---------------- arguments ----------------
fn expand(s: &str, vars: &HashMap<String, String>) -> String {
    let mut out = s.to_string();
    if out.contains("${") {
        for (k, v) in vars {
            out = out.replace(&format!("${{{k}}}"), v);
        }
    }
    out
}

fn flatten(
    args: &[mc::Arg],
    feats: &HashMap<&str, bool>,
    vars: &HashMap<String, String>,
) -> Vec<String> {
    let mut out = vec![];
    for a in args {
        match a {
            mc::Arg::Plain(s) => out.push(expand(s, vars)),
            mc::Arg::Ruled { rules, value } => {
                if mc::rules_allow(rules, feats) {
                    match value {
                        mc::StrOrVec::One(s) => out.push(expand(s, vars)),
                        mc::StrOrVec::Many(list) => {
                            out.extend(list.iter().map(|s| expand(s, vars)))
                        }
                    }
                }
            }
        }
    }
    out
}

fn build_args(
    v: &mc::VersionJson,
    inst: &Instance,
    sess: &auth::Session,
) -> Result<Vec<String>> {
    let plan = mc::lib_plan(v)?;
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut cp: Vec<String> = plan
        .classpath
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    cp.push(mc::jar_path(&v.jar_id).display().to_string());
    let classpath = cp.join(sep);
    let gdir = game_dir(&inst.id);
    let natives = instance_dir(&inst.id).join("natives");
    let assets_name = v
        .asset_index
        .as_ref()
        .map(|a| a.id.clone())
        .or_else(|| v.assets.clone())
        .unwrap_or_else(|| "legacy".to_string());

    let virt = assets().join("virtual").join(&assets_name);
    let game_assets = if virt.exists() {
        virt.display().to_string()
    } else {
        assets().display().to_string()
    };
    let has_server = !inst.server.trim().is_empty();
    let custom_res = inst.width > 0 && inst.height > 0;
    let mut vars: HashMap<String, String> = HashMap::new();
    for (k, val) in [
        ("auth_player_name", sess.username.clone()),
        ("version_name", v.id.clone()),
        ("game_directory", gdir.display().to_string()),
        ("assets_root", assets().display().to_string()),
        ("assets_index_name", assets_name),
        ("auth_uuid", sess.uuid.clone()),
        ("auth_access_token", sess.token.clone()),
        ("clientid", "0".to_string()),
        ("auth_xuid", "0".to_string()),
        ("user_type", sess.user_type.clone()),
        (
            "version_type",
            v.kind.clone().unwrap_or_else(|| "release".to_string()),
        ),
        ("user_properties", "{}".to_string()),
        ("natives_directory", natives.display().to_string()),
        ("launcher_name", "Nimbus".to_string()),
        ("launcher_version", "1.0".to_string()),
        ("classpath", classpath.clone()),
        ("classpath_separator", sep.to_string()),
        ("library_directory", libraries().display().to_string()),
        ("game_assets", game_assets.clone()),
        (
            "auth_session",
            format!("token:{}:{}", sess.token, sess.uuid),
        ),
        ("resolution_width", inst.width.to_string()),
        ("resolution_height", inst.height.to_string()),
        ("quickPlayMultiplayer", inst.server.trim().to_string()),
    ] {
        vars.insert(k.to_string(), val);
    }
    let mut feats: HashMap<&str, bool> = HashMap::new();
    feats.insert("is_demo_user", false);
    feats.insert("has_custom_resolution", custom_res);
    feats.insert("is_quick_play_multiplayer", has_server);

    // --- JVM ---
    let mem = inst.memory_mb.max(512);
    let mut jvm: Vec<String> = vec![
        format!("-Xmx{mem}M"),
        format!("-Xms{}M", mem.min(512)),
        "-XX:+UseG1GC".into(),
        "-XX:+ParallelRefProcEnabled".into(),
        "-XX:MaxGCPauseMillis=200".into(),
        "-XX:+UnlockExperimentalVMOptions".into(),
        "-XX:+DisableExplicitGC".into(),
        "-XX:G1NewSizePercent=30".into(),
        "-XX:G1MaxNewSizePercent=40".into(),
        "-XX:G1HeapRegionSize=8M".into(),
        "-XX:G1ReservePercent=20".into(),
        "-XX:G1HeapWastePercent=5".into(),
        "-XX:G1MixedGCCountTarget=4".into(),
        "-XX:InitiatingHeapOccupancyPercent=15".into(),
        "-XX:G1MixedGCLiveThresholdPercent=90".into(),
        "-XX:G1RSetUpdatingPauseTimePercent=5".into(),
        "-XX:SurvivorRatio=32".into(),
        "-XX:+PerfDisableSharedMem".into(),
        "-XX:MaxTenuringThreshold=1".into(),
        "-Dlog4j2.formatMsgNoLookups=true".into(),
        "-Dfile.encoding=UTF-8".into(),
    ];
    jvm.extend(inst.jvm_args.split_whitespace().map(String::from));
    let version_jvm = v
        .arguments
        .as_ref()
        .filter(|a| !a.jvm.is_empty())
        .map(|a| flatten(&a.jvm, &feats, &vars));
    match version_jvm {
        Some(list) => jvm.extend(list),
        None => {
            jvm.push(format!("-Djava.library.path={}", natives.display()));
            jvm.push("-cp".into());
            jvm.push(classpath);
        }
    }

    // --- game ---
    let mut game: Vec<String> = if let Some(a) = &v.arguments {
        flatten(&a.game, &feats, &vars)
    } else if let Some(m) = &v.minecraft_arguments {
        let mut g: Vec<String> = m.split_whitespace().map(|s| expand(s, &vars)).collect();
        if custom_res {
            g.extend([
                "--width".to_string(),
                inst.width.to_string(),
                "--height".to_string(),
                inst.height.to_string(),
            ]);
        }
        g
    } else {
        return Err(anyhow!("version has no launch arguments"));
    };

    if has_server && !game.iter().any(|a| a == "--quickPlayMultiplayer") {
        let s = inst.server.trim();
        let (host, port) = match s.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, p),
            _ => (s, "25565"),
        };
        game.extend([
            "--server".to_string(),
            host.to_string(),
            "--port".to_string(),
            port.to_string(),
        ]);
    }
    if inst.fullscreen {
        game.push("--fullscreen".to_string());
    }

    let main = v
        .main_class
        .clone()
        .ok_or_else(|| anyhow!("version has no main class"))?;
    let mut all = jvm;
    all.push(main);
    all.extend(game);
    Ok(all)
}

// ---------------- launch ----------------
fn spawn_reader<R>(app: AppHandle, id: String, r: R)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    // Lines are sent to the UI in batches (every ~120 ms) instead of one event per line.
    tokio::spawn(async move {
        let mut rd = BufReader::new(r);
        let mut buf: Vec<u8> = Vec::new();
        let mut batch: Vec<String> = Vec::new();
        let flush = |batch: &mut Vec<String>| {
            if !batch.is_empty() {
                let _ = app.emit(
                    "game-log",
                    LogEvt {
                        id: id.clone(),
                        lines: std::mem::take(batch),
                    },
                );
            }
        };
        loop {
            let res = tokio::time::timeout(
                std::time::Duration::from_millis(120),
                rd.read_until(b'\n', &mut buf),
            )
            .await;
            match res {
                Ok(Ok(0)) | Ok(Err(_)) => break,
                Ok(Ok(_)) => {
                    batch.push(String::from_utf8_lossy(&buf).trim_end().to_string());
                    buf.clear();
                    if batch.len() >= 200 {
                        flush(&mut batch);
                    }
                }
                Err(_) => flush(&mut batch),
            }
        }
        if !buf.is_empty() {
            batch.push(String::from_utf8_lossy(&buf).trim_end().to_string());
        }
        flush(&mut batch);
    });
}

async fn launch_core(
    app: &AppHandle,
    inst: &mut Instance,
    st: &Settings,
    account_id: &str,
) -> Result<()> {
    // token refresh (network) and file preparation run at the same time
    let (sess, (v, java)) =
        tokio::try_join!(auth::fresh_session(account_id), prepare(app, inst, st))?;
    if sess.user_type == "legacy" {
        let _ = skins::sync_offline_skin(inst, account_id, &sess.username).await;
    }
    let args = build_args(&v, inst, &sess)?;
    let gdir = game_dir(&inst.id);
    std::fs::create_dir_all(&gdir)?;

    let wrapper: Vec<String> = inst.wrapper.split_whitespace().map(String::from).collect();
    let (program, mut full_args): (String, Vec<String>) = match wrapper.split_first() {
        Some((first, rest)) => (first.clone(), rest.to_vec()),
        None => (java.display().to_string(), vec![]),
    };
    if !wrapper.is_empty() {
        full_args.push(java.display().to_string());
    }
    full_args.extend(args.iter().cloned());
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&full_args)
        .current_dir(&gdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to start Java ({}): {e}", java.display()))?;

    let id = inst.id.clone();
    let (tx, rx) = oneshot::channel::<()>();
    running().lock().unwrap().insert(id.clone(), tx);
    inst.last_played = now_secs();
    save_instance(inst)?;

    if let Some(out) = child.stdout.take() {
        spawn_reader(app.clone(), id.clone(), out);
    }
    if let Some(err) = child.stderr.take() {
        spawn_reader(app.clone(), id.clone(), err);
    }
    emit_state(app, &id, "running", None);
    progress(app, &id, "Running", 1, 1);

    let app2 = app.clone();
    tokio::spawn(async move {
        let mut killed = false;
        let code: Option<i32> = tokio::select! {
            st = child.wait() => st.ok().and_then(|s| s.code()),
            _ = rx => {
                killed = true;
                let _ = child.kill().await;
                child.wait().await.ok().and_then(|s| s.code())
            }
        };
        running().lock().unwrap().remove(&id);
        emit_state(&app2, &id, "exited", code);
        if !killed && code != Some(0) {
            let id3 = id.clone();
            let findings =
                tokio::task::spawn_blocking(move || crate::crash::analyze_instance(&id3))
                    .await
                    .unwrap_or_default();
            let _ = app2.emit(
                "game-crash",
                CrashEvt {
                    id: id.clone(),
                    code,
                    findings,
                },
            );
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn instance_launch(app: AppHandle, id: String, account_id: String) -> Result<(), String> {
    if is_running(&id) {
        return Err("This instance is already running".into());
    }
    let run = async {
        let mut inst = load_instance(&id)?;
        let st = load_settings();
        emit_state(&app, &id, "starting", None);
        launch_core(&app, &mut inst, &st, &account_id).await
    };
    let res = run.await;
    if res.is_err() {
        emit_state(&app, &id, "idle", None);
    }
    res.map_err(es)
}

#[tauri::command]
pub fn instance_kill(id: String) -> Result<(), String> {
    if let Some(tx) = running().lock().unwrap().remove(&id) {
        let _ = tx.send(());
    }
    Ok(())
}

#[tauri::command]
pub fn instance_running() -> Vec<String> {
    running().lock().unwrap().keys().cloned().collect()
}

/// Re-verifies every known file hash (libraries, assets, Java) and re-downloads anything damaged.
#[tauri::command]
pub async fn instance_repair(app: AppHandle, id: String) -> Result<Instance, String> {
    if is_running(&id) {
        return Err("Stop the game before repairing".into());
    }
    let run = async {
        let mut inst = load_instance(&id)?;
        let st = load_settings();
        if let Ok(rd) = std::fs::read_dir(runtimes()) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path().join(".installed"));
            }
        }
        set_verify(true);
        let r = prepare(&app, &mut inst, &st).await;
        set_verify(false);
        r?;
        progress(&app, &id, "Ready", 1, 1);
        Ok::<Instance, anyhow::Error>(inst)
    };
    run.await.map_err(es)
}
