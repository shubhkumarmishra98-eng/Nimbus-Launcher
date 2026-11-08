# Nimbus Launcher (v1)

A fast, lightweight Minecraft: Java Edition launcher built with **Rust + Tauri 2**.
No bundled Chromium, no Node build step — the UI is plain HTML/CSS/JS served straight from `ui/`.

## Features

| Area | What's in v1 |
|---|---|
| Performance | Rust core, HTTP/2 parallel downloads that resume after interruptions, SHA‑1 checks, cancel button, libraries/assets/Java shared across instances, size-based fast path so relaunches skip re-hashing, tuned G1GC flags, optional performance-mod pack, one-click **Repair** (full re-verify) |
| Game | Every Mojang version incl. pre‑1.7 (virtual assets), automatic Java runtime per version, isolated instances, join-server-on-launch, fullscreen, window size, launch wrapper (e.g. `gamemoderun`) |
| Loaders | Fabric, Quilt, Forge (**1.5–1.12 legacy installers and 1.13+**), NeoForge (1.20.1 and 1.20.2+) |
| Content | Browse/install **mods, resource packs, shaders, modpacks** from Modrinth; mods also from CurseForge; required dependencies auto-installed; enable/disable/remove/add `.jar`; update check + update all; installed mods show their real names/icons |
| Modpacks | Import Modrinth `.mrpack` and CurseForge `.zip`; install Modrinth modpacks in one click; export any instance as `.mrpack` |
| Files | Manage worlds, resource packs, shader packs, screenshots; add packs/worlds from `.zip`; world backups |
| Instances | Duplicate, export, repair, delete, search, **switch Minecraft version/loader**, server-list editor (`servers.dat`) with auto-join |
| Accounts | Microsoft sign-in (device code, auto-refresh) and offline accounts |
| Skins & capes | Interactive **3D preview**; premium: upload skin (classic/slim), equip/hide capes; offline: local skin + cape via CustomSkinLoader (auto-installed) |
| Diagnostics | Live console, **crash analyzer** (auto-runs after a crash), optional log sharing to mclo.gs |
| Launcher | Dark/light theme, optional update check against a GitHub repo, account file locked to the current user on Linux/macOS |

## How it stays fast and light

- **Native core, tiny UI:** Rust + the system webview (no bundled Chromium); release builds use LTO, stripped symbols, `panic=abort`.
- **Near-instant relaunch:** after the first full download Nimbus remembers the instance is complete and skips every library/asset check
  (thousands of files). *Repair* forces a full re-verify.
- **Parallel startup:** Microsoft token refresh and file preparation run at the same time; offline accounts need no network at all.
- **Fast first install:** HTTP/2, 24 parallel downloads (adjustable), libraries and assets fetched simultaneously, interrupted downloads resume,
  everything is shared between instances so nothing is downloaded twice.
- **Game tuning:** G1GC flags tuned for Minecraft, sensible memory defaults (half your RAM, capped), optional Sodium/Lithium/FerriteCore pack.
- **Light on the UI thread:** game log lines are sent in batches, console is capped, progress events are throttled.

## Shared mods and resource packs

All instances read mods and resource packs from two common folders (default: `NimbusLauncher/common/mods` and `.../common/resourcepacks`
in your Roaming data folder), so you add something once and every instance sees it. Under the hood each instance's `mods` and
`resourcepacks` folder is a link (a junction on Windows — no admin rights needed) to the common folder.

- **Settings → Shared folders:** change either folder, or reset it to the default. A custom folder's path is not displayed;
  the default folder has a folder button that opens it.
- **Per instance:** the Settings tab has "Use the shared mods and resource packs folders". Switch it off and the instance gets its own
  copy. Imported modpacks start with sharing off so they keep their own mod set.
- **Heads-up:** mods are tied to a Minecraft version and loader, so instances on different versions/loaders usually shouldn't share mods.

## Everything is point-and-click

First run asks you to **sign in with Microsoft or pick a local username**, then offers a one-click "Create & play". Resource packs, shaders,
worlds, mods and modpacks can be added with buttons **or by dragging files onto the window**; resource packs can be switched on/off in the launcher
(no need to touch `options.txt`).

## Build & run

1. Install **Rust** (https://rustup.rs) and the Tauri prerequisites for your OS
   (https://v2.tauri.app/start/prerequisites/ — on Windows: MSVC build tools + WebView2, on Linux: `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `librsvg2-dev`, `libssl-dev`, `patchelf`).
2. Install the Tauri CLI: `cargo install tauri-cli --version "^2" --locked`
3. From this folder:
   - Dev: `cargo tauri dev`
   - Release installer/binary: `cargo tauri build` (output in `src-tauri/target/release/bundle/`)
4. (Optional, for macOS `.icns` and all icon sizes) `cargo tauri icon src-tauri/icons/icon.png`

A GitHub Actions workflow (`.github/workflows/build.yml`) builds Windows, macOS and Linux bundles for you.

## Get a portable .exe without installing anything

1. Make a free GitHub account and create a new empty repository.
2. Upload the contents of this folder (drag & drop in the browser; make sure the `.github` folder is included).
3. Open the **Actions** tab → **windows-exe** → **Run workflow**.
4. After a few minutes download the **Nimbus-Launcher-portable** artifact: a single `nimbus-launcher.exe` you can double-click
   (Windows 10/11 already include the WebView2 component it needs).

## Microsoft sign-in setup (you do this once; players never see it)

Microsoft requires every launcher to have its own Azure application. Register one, then **embed its ID in your release builds**
so players just click “Sign in with Microsoft”, open the link and type a short code:

1. https://portal.azure.com → *App registrations* → *New registration*.
   Supported account types: **Personal Microsoft accounts only** (or "…and organizational").
2. *Authentication* → enable **Allow public client flows**.
3. Copy the **Application (client) ID**.
4. Build with it: `NIMBUS_MS_CLIENT_ID=<id> cargo tauri build` (in GitHub Actions, add it as the `NIMBUS_MS_CLIENT_ID` secret).
   For local testing you can instead paste it into *Settings → Microsoft client ID*.
5. Mojang requires new Azure apps to be approved before the Minecraft services accept them
   (see the "Microsoft authentication" page on the Minecraft Wiki for the current review form).
   Until it is approved you may get a 403 at the last step — offline accounts work in the meantime.

CurseForge browsing needs a free API key from https://console.curseforge.com → paste it in Settings.

## Where data lives

`%APPDATA%\NimbusLauncher` (Windows), `~/Library/Application Support/NimbusLauncher` (macOS), `~/.local/share/NimbusLauncher` (Linux):
`shared/` (libraries, assets, versions), `runtimes/` (Java), `instances/<id>/minecraft/` (each game folder), `accounts.json`, `settings.json`.

## Known limitations

- **This source was written without being compiled in the authoring environment** (no Rust toolchain was available), and the
  third-party flows (Microsoft/Xbox login, CurseForge, legacy Forge installers, mclo.gs) are written from API documentation, not tested live.
  Expect a few compiler errors/warnings on the first `cargo tauri dev` and some edge-case fixes after real-world use.
- Account tokens are stored in a JSON file in the data folder (restricted to your user on Linux/macOS, but **not** in the OS keychain yet).
- CurseForge: pack import and mod install only; shader/resource-pack/modpack browsing is Modrinth-only. Mods whose authors block third-party downloads must be added manually.
- Apple Silicon Macs run Minecraft 1.18 and older through an x64 Java (needs Rosetta); 1.19+ run natively.
- Very old Forge versions with unusual installer layouts aren't covered.
- Offline-mode skins need a modded instance (the CustomSkinLoader mod does the work).
- No built-in auto-updater (only the release check); no Discord presence.
- Respect the Minecraft EULA/brand guidelines: don't use Mojang/Minecraft logos in the app, and buy the game if you play online.
