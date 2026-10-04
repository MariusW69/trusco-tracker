//! TrusCo Tracker desktop app: a tray/menu-bar icon, a tracker thread that samples
//! the foreground window every few seconds, a sync thread that sends finished work
//! blocks to TrusCo, and a small window for pairing, today's activity and settings.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_opener::OpenerExt as _;

use tracker_core::api::{self, ApiError, BlockPayload};
use tracker_core::platform::{self, Sampler};
use tracker_core::store::{Account, Store};
use tracker_core::{Activity, Block, Engine, Sample, Settings, WindowInfo, secrets};

const SAMPLE_EVERY: Duration = Duration::from_secs(5);
const SYNC_EVERY: Duration = Duration::from_secs(60);
const SAVE_EVERY_TICKS: u32 = 6;
const SYNC_BATCH: usize = 200;

struct Inner {
    engine: Engine,
    settings: Settings,
    account: Option<Account>,
    token: Option<String>,
    activity: Activity,
    current: Option<WindowInfo>,
    last_sync: Option<DateTime<Utc>>,
    last_sync_error: Option<String>,
}

struct AppState {
    inner: Mutex<Inner>,
    store: Store,
    sync_tx: Mutex<Sender<()>>,
}

impl AppState {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn request_sync(&self) {
        let _ = self.sync_tx.lock().unwrap_or_else(|e| e.into_inner()).send(());
    }
}

struct Tray {
    icon: TrayIcon<Wry>,
    status: MenuItem<Wry>,
    today: MenuItem<Wry>,
    pause_15: MenuItem<Wry>,
    pause_60: MenuItem<Wry>,
    pause_toggle: MenuItem<Wry>,
    last_shown: Mutex<String>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            get_state,
            default_device_name,
            pair,
            disconnect,
            pause,
            resume,
            save_settings,
            request_accessibility,
            sync_now,
            open_review,
            remove_block,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let store = Store::new(app.path().app_data_dir()?)?;
            let token = secrets::load_token();
            let paired = token.is_some();
            let (tx, rx) = mpsc::channel();
            app.manage(AppState {
                inner: Mutex::new(Inner {
                    engine: store.load_engine(),
                    settings: store.load_settings(),
                    account: if paired { store.load_account() } else { None },
                    token,
                    activity: Activity::NoWindow,
                    current: None,
                    last_sync: None,
                    last_sync_error: None,
                }),
                store,
                sync_tx: Mutex::new(tx),
            });
            build_tray(app.handle())?;
            spawn_tracker(app.handle().clone());
            spawn_sync(app.handle().clone(), rx);
            if !paired {
                show_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window only hides it; tracking carries on in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building TrusCo Tracker")
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                } else {
                    save_now(app);
                }
            }
        });
}

// ---------------------------------------------------------------------------
// Background threads
// ---------------------------------------------------------------------------

fn spawn_tracker(app: AppHandle) {
    thread::Builder::new()
        .name("tracker".into())
        .spawn(move || {
            let mut sampler = Sampler::default();
            let mut ticks: u32 = 0;
            loop {
                let now = Utc::now();
                let state = app.state::<AppState>();
                let (paired, paused) = {
                    let s = state.lock();
                    (s.token.is_some(), s.settings.is_paused(now))
                };
                // Nothing is read from the screen while paused or before the device is connected.
                let sample = (paired && !paused).then(|| {
                    let (idle_secs, window) = sampler.sample();
                    Sample { at: now, idle_secs, window }
                });
                {
                    let mut guard = state.lock();
                    let s = &mut *guard;
                    match sample {
                        Some(sample) => {
                            s.activity = s.engine.tick(&sample, &s.settings);
                            s.current = match s.activity {
                                Activity::Tracking { .. } => sample.window,
                                _ => None,
                            };
                        }
                        None => {
                            s.activity = if paused { Activity::Paused } else { Activity::NoWindow };
                            s.current = None;
                        }
                    }
                    ticks = ticks.wrapping_add(1);
                    if ticks % SAVE_EVERY_TICKS == 0 {
                        s.engine.prune(now);
                        let _ = state.store.save_engine(&s.engine);
                    }
                }
                refresh_tray(&app);
                thread::sleep(SAMPLE_EVERY);
            }
        })
        .expect("failed to start tracker thread");
}

fn spawn_sync(app: AppHandle, rx: Receiver<()>) {
    thread::Builder::new()
        .name("sync".into())
        .spawn(move || {
            refresh_account(&app);
            loop {
                while sync_once(&app) {}
                let _ = rx.recv_timeout(SYNC_EVERY);
                while rx.try_recv().is_ok() {}
            }
        })
        .expect("failed to start sync thread");
}

/// Sends one batch of changed blocks. Returns true when more are waiting.
fn sync_once(app: &AppHandle) -> bool {
    let state = app.state::<AppState>();
    let (token, server, batch) = {
        let s = state.lock();
        let Some(token) = s.token.clone() else { return false };
        (token, s.settings.server_url.clone(), s.engine.pending_sync(&s.settings, SYNC_BATCH))
    };
    if batch.is_empty() {
        state.lock().last_sync = Some(Utc::now());
        return false;
    }
    let payload: Vec<BlockPayload> = batch.iter().map(BlockPayload::from).collect();
    match api::Client::new(&server).sync(&token, &payload) {
        Ok(results) => {
            let mut s = state.lock();
            for sent in &batch {
                let r = results.iter().find(|r| r.external_id == sent.id);
                s.engine.mark_synced(
                    &sent.id,
                    sent.version,
                    r.and_then(|r| r.status.clone()),
                    r.and_then(|r| r.matter_label.clone()),
                    r.and_then(|r| r.match_reason.clone()),
                );
            }
            s.last_sync = Some(Utc::now());
            s.last_sync_error = None;
            let _ = state.store.save_engine(&s.engine);
            batch.len() == SYNC_BATCH
        }
        Err(ApiError::Unauthorized) => {
            forget_device(app, "This device was disconnected in TrusCo. Enter a new code to reconnect.");
            false
        }
        Err(e) => {
            state.lock().last_sync_error = Some(e.to_string());
            false
        }
    }
}

/// Picks up renamed devices / organizations; detects a revoked token at start-up.
fn refresh_account(app: &AppHandle) {
    let state = app.state::<AppState>();
    let (token, server) = {
        let s = state.lock();
        let Some(token) = s.token.clone() else { return };
        (token, s.settings.server_url.clone())
    };
    match api::Client::new(&server).me(&token) {
        Ok(account) => {
            let _ = state.store.save_account(Some(&account));
            state.lock().account = Some(account);
        }
        Err(ApiError::Unauthorized) => {
            forget_device(app, "This device was disconnected in TrusCo. Enter a new code to reconnect.")
        }
        Err(_) => {}
    }
}

/// Removes the token and everything captured on this device.
fn forget_device(app: &AppHandle, reason: &str) {
    secrets::clear_token();
    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        s.token = None;
        s.account = None;
        s.engine = Engine::default();
        s.current = None;
        s.last_sync_error = Some(reason.to_string());
        let _ = state.store.save_engine(&s.engine);
    }
    let _ = state.store.save_account(None);
    refresh_tray(app);
    show_window(app);
}

fn save_now(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        let s = state.lock();
        let _ = state.store.save_engine(&s.engine);
        let _ = state.store.save_settings(&s.settings);
    }
}

// ---------------------------------------------------------------------------
// Tray
// ---------------------------------------------------------------------------

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let today = MenuItem::with_id(app, "today", "Today: 0m", false, None::<&str>)?;
    let pause_15 = MenuItem::with_id(app, "pause15", "Pause for 15 minutes", true, None::<&str>)?;
    let pause_60 = MenuItem::with_id(app, "pause60", "Pause for 1 hour", true, None::<&str>)?;
    let pause_toggle = MenuItem::with_id(app, "pause", "Pause until I resume", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Show today's activity…", true, None::<&str>)?;
    let review = MenuItem::with_id(app, "review", "Review in TrusCo…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit TrusCo Tracker", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&status, &today, &sep1, &pause_15, &pause_60, &pause_toggle, &sep2, &open, &review, &sep3, &quit],
    )?;

    let icon = TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("TrusCo Tracker")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "pause15" => set_pause(app, Some(15)),
            "pause60" => set_pause(app, Some(60)),
            "pause" => {
                let paused = app.state::<AppState>().lock().settings.is_paused(Utc::now());
                if paused { clear_pause(app) } else { set_pause(app, None) }
            }
            "open" => show_window(app),
            "review" => open_review_page(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    app.manage(Tray { icon, status, today, pause_15, pause_60, pause_toggle, last_shown: Mutex::new(String::new()) });
    Ok(())
}

fn refresh_tray(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else { return };
    let state = app.state::<AppState>();
    let (status, today_secs, paused) = {
        let s = state.lock();
        (status_line(&s), s.engine.today_seconds(), s.settings.is_paused(Utc::now()))
    };
    let today = format!("Today: {}", fmt_duration(today_secs));
    let signature = format!("{status}|{today}|{paused}");
    {
        let mut last = tray.last_shown.lock().unwrap_or_else(|e| e.into_inner());
        if *last == signature {
            return;
        }
        *last = signature;
    }
    let _ = tray.status.set_text(&status);
    let _ = tray.today.set_text(&today);
    let _ = tray.pause_15.set_enabled(!paused);
    let _ = tray.pause_60.set_enabled(!paused);
    let _ = tray.pause_toggle.set_text(if paused { "Resume tracking" } else { "Pause until I resume" });
    let _ = tray.icon.set_tooltip(Some(format!("TrusCo Tracker — {status}")));
    #[cfg(target_os = "macos")]
    let _ = tray.icon.set_title(Some(if paused { "Paused".to_string() } else { fmt_duration(today_secs) }));
}

fn status_line(s: &Inner) -> String {
    if s.token.is_none() {
        return "Not connected — open to connect".into();
    }
    if s.settings.paused_indefinitely {
        return "Paused".into();
    }
    if let Some(until) = s.settings.paused_until.filter(|t| *t > Utc::now()) {
        return format!("Paused until {}", until.with_timezone(&Local).format("%H:%M"));
    }
    match &s.activity {
        Activity::Tracking { .. } => {
            let what = s.current.as_ref().map(display_name).unwrap_or_default();
            format!("Tracking: {}", truncate(&what, 42))
        }
        Activity::Idle => "Idle — away from the computer".into(),
        Activity::Excluded => "Not tracking this app".into(),
        Activity::Paused => "Paused".into(),
        Activity::NoWindow => "Waiting for activity".into(),
    }
}

fn set_pause(app: &AppHandle, minutes: Option<i64>) {
    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        match minutes {
            Some(m) => {
                s.settings.paused_until = Some(Utc::now() + chrono::Duration::minutes(m));
                s.settings.paused_indefinitely = false;
            }
            None => s.settings.paused_indefinitely = true,
        }
        let _ = state.store.save_settings(&s.settings);
    }
    refresh_tray(app);
}

fn clear_pause(app: &AppHandle) {
    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        s.settings.paused_until = None;
        s.settings.paused_indefinitely = false;
        let _ = state.store.save_settings(&s.settings);
    }
    refresh_tray(app);
}

fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn open_review_page(app: &AppHandle) {
    let server = app.state::<AppState>().lock().settings.server_url.clone();
    let _ = app.opener().open_url(format!("{server}/captured-time"), None::<&str>);
}

// ---------------------------------------------------------------------------
// Commands for the window
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct BlockView {
    id: String,
    app_name: String,
    label: String,
    detail: String,
    started_at: String,
    ended_at: String,
    active_seconds: u64,
    pages: Option<u32>,
    matter_label: Option<String>,
    match_reason: Option<String>,
    /// approved | discarded | synced | waiting | short
    status: &'static str,
}

#[derive(Serialize, Deserialize)]
struct SettingsView {
    server_url: String,
    idle_minutes: u64,
    excluded_apps: Vec<String>,
    excluded_keywords: Vec<String>,
}

#[derive(Serialize)]
struct UiState {
    paired: bool,
    account: Option<Account>,
    status: String,
    activity: Activity,
    paused: bool,
    today_seconds: u64,
    blocks: Vec<BlockView>,
    last_sync: Option<String>,
    last_sync_error: Option<String>,
    accessibility: bool,
    platform: &'static str,
    version: &'static str,
    settings: SettingsView,
    launch_at_login: bool,
}

fn ui_state(app: &AppHandle) -> UiState {
    let state = app.state::<AppState>();
    let s = state.lock();
    let mut blocks: Vec<BlockView> = s.engine.today().into_iter().map(|b| block_view(b, &s.settings)).collect();
    blocks.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    UiState {
        paired: s.token.is_some(),
        account: s.account.clone(),
        status: status_line(&s),
        activity: s.activity.clone(),
        paused: s.settings.is_paused(Utc::now()),
        today_seconds: s.engine.today_seconds(),
        blocks,
        last_sync: s.last_sync.map(|t| t.to_rfc3339()),
        last_sync_error: s.last_sync_error.clone(),
        accessibility: platform::accessibility_granted(),
        platform: api::platform_name(),
        version: api::APP_VERSION,
        settings: SettingsView {
            server_url: s.settings.server_url.clone(),
            idle_minutes: s.settings.idle_threshold_secs / 60,
            excluded_apps: s.settings.excluded_apps.clone(),
            excluded_keywords: s.settings.excluded_keywords.clone(),
        },
        launch_at_login: app.autolaunch().is_enabled().unwrap_or(false),
    }
}

fn block_view(b: &Block, cfg: &Settings) -> BlockView {
    let status = match b.server_status.as_deref() {
        Some("approved") => "approved",
        Some("discarded") => "discarded",
        _ if b.active_seconds() < cfg.min_sync_secs => "short",
        _ if b.is_dirty() => "waiting",
        _ => "synced",
    };
    let info = WindowInfo {
        app_name: b.app_name.clone(),
        title: b.window_title.clone(),
        document_path: b.document_path.clone(),
        ..Default::default()
    };
    BlockView {
        id: b.id.clone(),
        app_name: b.app_name.clone(),
        label: display_name(&info),
        detail: b.document_path.clone().unwrap_or_else(|| b.window_title.clone()),
        started_at: b.started_at.to_rfc3339(),
        ended_at: b.ended_at.to_rfc3339(),
        active_seconds: b.active_seconds(),
        pages: b.document_pages,
        matter_label: b.matter_label.clone(),
        match_reason: b.match_reason.clone(),
        status,
    }
}

#[tauri::command]
fn get_state(app: AppHandle) -> UiState {
    ui_state(&app)
}

#[tauri::command]
fn default_device_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("/usr/sbin/scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    #[cfg(windows)]
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        return name;
    }
    "My computer".into()
}

#[tauri::command]
async fn pair(app: AppHandle, code: String, device_name: String, server_url: String) -> Result<UiState, String> {
    let server = normalize_server(&server_url)?;
    let name = match device_name.trim() {
        "" => default_device_name(),
        n => n.chars().take(80).collect(),
    };
    let (token, account) = {
        let (server, name) = (server.clone(), name.clone());
        tauri::async_runtime::spawn_blocking(move || api::Client::new(&server).pair(&code, &name))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?
    };
    secrets::save_token(&token).map_err(|e| format!("Could not save the device key in the keychain: {e}"))?;

    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        s.token = Some(token);
        s.account = Some(account.clone());
        s.settings.server_url = server;
        s.settings.device_name = Some(name);
        s.last_sync_error = None;
        let _ = state.store.save_account(Some(&account));
        let _ = state.store.save_settings(&s.settings);
    }
    let _ = app.autolaunch().enable();
    state.request_sync();
    refresh_tray(&app);
    Ok(ui_state(&app))
}

#[tauri::command]
fn disconnect(app: AppHandle) -> UiState {
    forget_device(&app, "Disconnected. Captured activity on this computer was removed.");
    ui_state(&app)
}

#[tauri::command]
fn pause(app: AppHandle, minutes: Option<i64>) -> UiState {
    set_pause(&app, minutes);
    ui_state(&app)
}

#[tauri::command]
fn resume(app: AppHandle) -> UiState {
    clear_pause(&app);
    ui_state(&app)
}

#[tauri::command]
fn save_settings(app: AppHandle, settings: SettingsView, launch_at_login: bool) -> Result<UiState, String> {
    let server = normalize_server(&settings.server_url)?;
    let clean = |v: Vec<String>| -> Vec<String> {
        v.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    };
    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        s.settings.server_url = server;
        s.settings.idle_threshold_secs = settings.idle_minutes.clamp(1, 120) * 60;
        s.settings.excluded_apps = clean(settings.excluded_apps);
        s.settings.excluded_keywords = clean(settings.excluded_keywords);
        state.store.save_settings(&s.settings).map_err(|e| e.to_string())?;
    }
    let autolaunch = app.autolaunch();
    let _ = if launch_at_login { autolaunch.enable() } else { autolaunch.disable() };
    Ok(ui_state(&app))
}

#[tauri::command]
fn request_accessibility() {
    platform::request_accessibility();
}

#[tauri::command]
fn sync_now(app: AppHandle) {
    app.state::<AppState>().request_sync();
}

#[tauri::command]
fn open_review(app: AppHandle) {
    open_review_page(&app);
}

/// Lets the user drop something private before it is sent (already-sent items are discarded in TrusCo).
#[tauri::command]
fn remove_block(app: AppHandle, id: String) -> UiState {
    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        s.engine.blocks.retain(|b| b.id != id);
        let _ = state.store.save_engine(&s.engine);
    }
    ui_state(&app)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn normalize_server(url: &str) -> Result<String, String> {
    let url = url.trim().trim_end_matches('/');
    let local = url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1");
    if url.starts_with("https://") || local {
        Ok(url.to_string())
    } else {
        Err("The TrusCo address must start with https://".into())
    }
}

/// "Settlement.docx" for documents, the window title otherwise, the app name as a last resort.
fn display_name(w: &WindowInfo) -> String {
    if let Some(path) = w.document_path.as_deref().filter(|p| !p.is_empty()) {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if !w.title.trim().is_empty() {
        return w.title.trim().to_string();
    }
    w.app_name.clone()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

fn fmt_duration(secs: u64) -> String {
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    if h > 0 { format!("{h}h {m:02}m") } else { format!("{m}m") }
}
