//! Tasks: the cached project tasks, the tray "Tasks" submenu (Detach at the top), the detached
//! always-on-top Tasks window, the window commands and the macOS widget snapshot.

use std::sync::Mutex;

use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};
use tauri::menu::{IsMenuItem, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Wry};

use tracker_core::api::{self, ApiError, Project, Task};
use tracker_core::tasks::{self as t, Urgency};

use super::{AppState, forget_device, show_window};

pub(crate) const FLOAT_LABEL: &str = "tasks-float";
const MENU_TASKS: usize = 12;

pub(crate) struct TasksMenu {
    submenu: Submenu<Wry>,
    last_shown: Mutex<String>,
}

// ---------------------------------------------------------------------------
// Fetching / saving
// ---------------------------------------------------------------------------

fn client_and_token(app: &AppHandle) -> Result<(api::Client, String), String> {
    let s = app.state::<AppState>();
    let s = s.lock();
    let token = s.token.clone().ok_or("Connect TrusCo Tracker to TrusCo first")?;
    Ok((api::Client::new(&s.settings.server_url), token))
}

fn api_error(app: &AppHandle, e: ApiError) -> String {
    if matches!(e, ApiError::Unauthorized) {
        forget_device(app, "This device was disconnected in TrusCo. Enter a new code to reconnect.");
    }
    match e {
        ApiError::Network(_) => "You're offline or TrusCo can't be reached. Try again in a moment.".into(),
        other => other.to_string(),
    }
}

/// Pulls tasks (and, when asked, projects) from TrusCo into the cache. Runs on the sync thread.
pub(crate) fn fetch(app: &AppHandle, with_projects: bool) -> Result<(), String> {
    let (client, token) = client_and_token(app)?;
    let tasks = client.tasks(&token).map_err(|e| api_error(app, e))?;
    let projects = if with_projects { Some(client.projects(&token).map_err(|e| api_error(app, e))?) } else { None };
    {
        let state = app.state::<AppState>();
        let mut s = state.lock();
        s.tasks.tasks = tasks;
        if let Some(p) = projects {
            s.tasks.projects = p;
        }
        s.tasks.synced_at = Some(Utc::now());
        s.tasks_error = None;
        let _ = state.store.save_tasks(&s.tasks);
    }
    changed(app);
    Ok(())
}

/// After the cache changes: tray menu, widget, and any open window.
pub(crate) fn changed(app: &AppHandle) {
    refresh_menu(app);
    write_widget(app);
    let _ = app.emit("tasks-changed", ());
}

fn upsert(app: &AppHandle, task: Task) {
    let state = app.state::<AppState>();
    {
        let mut s = state.lock();
        match s.tasks.tasks.iter_mut().find(|x| x.id == task.id) {
            Some(existing) => *existing = task,
            None => s.tasks.tasks.push(task),
        }
        let _ = state.store.save_tasks(&s.tasks);
    }
    changed(app);
}

// ---------------------------------------------------------------------------
// Views for the windows
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct TaskView {
    id: String,
    project_id: String,
    project: String,
    title: String,
    kind: String,
    priority: String,
    done: bool,
    urgency: Urgency,
    due_label: String,
    budget: Option<String>,
    slot: Option<String>,
    // Raw values for the edit form, in local time.
    due_date: Option<String>,
    due_time: Option<String>,
    slot_date: Option<String>,
    slot_time: Option<String>,
    estimated_hours: f64,
}

#[derive(Serialize)]
pub(crate) struct TasksState {
    paired: bool,
    tasks: Vec<TaskView>,
    projects: Vec<Project>,
    synced_at: Option<String>,
    error: Option<String>,
    float_open: bool,
}

fn view(task: &Task) -> TaskView {
    let now = Local::now();
    let (due_date, due_time) = match task.due_at {
        Some(at) => {
            let (d, tm) = t::utc_to_local_parts(at);
            (Some(d), Some(tm))
        }
        None => (task.due_date.map(|d| d.format("%Y-%m-%d").to_string()), None),
    };
    let (slot_date, slot_time) = task.scheduled_at.map(t::utc_to_local_parts).unzip();
    TaskView {
        id: task.id.clone(),
        project_id: task.project_id.clone(),
        project: t::project_label(task),
        title: task.title.clone(),
        kind: task.kind.clone(),
        priority: task.priority.clone(),
        done: task.status == "done",
        urgency: t::urgency(task, now),
        due_label: t::due_label(task, now),
        budget: t::budget_label(task),
        slot: t::slot_label(task, now),
        due_date,
        due_time,
        slot_date,
        slot_time,
        estimated_hours: task.estimated_hours,
    }
}

pub(crate) fn tasks_state(app: &AppHandle) -> TasksState {
    let state = app.state::<AppState>();
    let s = state.lock();
    let mut tasks = s.tasks.tasks.clone();
    t::sort(&mut tasks, Local::now());
    TasksState {
        paired: s.token.is_some(),
        tasks: tasks.iter().map(view).collect(),
        projects: s.tasks.projects.clone(),
        synced_at: s.tasks.synced_at.map(|x| x.to_rfc3339()),
        error: s.tasks_error.clone(),
        float_open: app.get_webview_window(FLOAT_LABEL).is_some(),
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn get_tasks(app: AppHandle) -> TasksState {
    tasks_state(&app)
}

#[tauri::command]
pub(crate) async fn refresh_tasks(app: AppHandle) -> Result<TasksState, String> {
    let a = app.clone();
    tauri::async_runtime::spawn_blocking(move || fetch(&a, true)).await.map_err(|e| e.to_string())??;
    Ok(tasks_state(&app))
}

/// The task form: a new task (no id) or a full edit. Dates and times are local.
#[derive(Deserialize)]
pub(crate) struct TaskForm {
    id: Option<String>,
    project_id: String,
    title: String,
    kind: String,
    high_priority: bool,
    due_date: Option<String>,
    due_time: Option<String>,
    estimated_hours: Option<f64>,
    slot_date: Option<String>,
    slot_time: Option<String>,
}

#[tauri::command]
pub(crate) async fn save_task(app: AppHandle, form: TaskForm) -> Result<TasksState, String> {
    let empty = |v: &Option<String>| v.as_deref().map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    let (due_date, due_time) = (empty(&form.due_date), empty(&form.due_time));
    if due_time.is_some() && due_date.is_none() {
        return Err("Choose a due date for the due time".into());
    }
    let due_at = t::local_to_utc(due_date.as_deref(), due_time.as_deref());
    let time_task = form.kind == "time";
    let scheduled_at = if time_task { t::local_to_utc(empty(&form.slot_date).as_deref(), empty(&form.slot_time).as_deref()) } else { None };
    let mut body = serde_json::json!({
        "project_id": form.project_id,
        "title": form.title.trim(),
        "kind": if time_task { "time" } else { "todo" },
        "priority": if form.high_priority { "high" } else { "normal" },
        "due_at": due_at.map(|d| d.to_rfc3339()),
        "due_date": due_date,
        "scheduled_at": scheduled_at.map(|d| d.to_rfc3339()),
        "estimated_hours": if time_task { form.estimated_hours.unwrap_or(0.0) } else { 0.0 },
    });
    if let Some(id) = form.id.filter(|x| !x.is_empty()) {
        body["id"] = id.into();
    }
    let a = app.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        let (client, token) = client_and_token(&a)?;
        client.save_task(&token, &body).map_err(|e| api_error(&a, e))
    })
    .await
    .map_err(|e| e.to_string())??;
    upsert(&app, task);
    Ok(tasks_state(&app))
}

#[tauri::command]
pub(crate) async fn set_task_done(app: AppHandle, id: String, done: bool) -> Result<TasksState, String> {
    let a = app.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        let (client, token) = client_and_token(&a)?;
        let body = serde_json::json!({ "id": id, "status": if done { "done" } else { "todo" } });
        client.save_task(&token, &body).map_err(|e| api_error(&a, e))
    })
    .await
    .map_err(|e| e.to_string())??;
    upsert(&app, task);
    Ok(tasks_state(&app))
}

#[tauri::command]
pub(crate) async fn create_project(app: AppHandle, name: String) -> Result<Project, String> {
    let a = app.clone();
    let project = tauri::async_runtime::spawn_blocking(move || {
        let (client, token) = client_and_token(&a)?;
        client.create_project(&token, name.trim()).map_err(|e| api_error(&a, e))
    })
    .await
    .map_err(|e| e.to_string())??;
    {
        let state = app.state::<AppState>();
        let mut s = state.lock();
        s.tasks.projects.insert(0, project.clone());
        let _ = state.store.save_tasks(&s.tasks);
    }
    Ok(project)
}

#[tauri::command]
pub(crate) fn detach_tasks(app: AppHandle) {
    open_float(&app);
}

#[tauri::command]
pub(crate) fn open_tasks_in_main(app: AppHandle, target: Option<String>) {
    open_main(&app, target.as_deref().unwrap_or("tasks"));
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Shows the main window on the Tasks tab ("tasks", "tasks-new" or "task:<id>").
pub(crate) fn open_main(app: &AppHandle, target: &str) {
    show_window(app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.emit("navigate", target);
    }
}

/// The detached Tasks list: small, always on top, on every desktop Space.
pub(crate) fn open_float(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(FLOAT_LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, FLOAT_LABEL, WebviewUrl::App("index.html#float".into()))
        .title("Tasks")
        .inner_size(300.0, 420.0)
        .min_inner_size(240.0, 180.0)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(true)
        .build();
    if built.is_ok() {
        set_float_open(app, true);
        let _ = app.emit("tasks-changed", ());
    }
}

pub(crate) fn set_float_open(app: &AppHandle, open: bool) {
    let state = app.state::<AppState>();
    let mut s = state.lock();
    if s.settings.tasks_float_open != open {
        s.settings.tasks_float_open = open;
        let _ = state.store.save_settings(&s.settings);
    }
}

// ---------------------------------------------------------------------------
// Tray "Tasks" submenu
// ---------------------------------------------------------------------------

pub(crate) fn build_submenu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let submenu = Submenu::with_id(app, "tasks", "Tasks", true)?;
    app.manage(TasksMenu { submenu: submenu.clone(), last_shown: Mutex::new(String::new()) });
    refresh_menu(app);
    Ok(submenu)
}

/// Rebuilds the submenu when its text would change (labels such as "in 45m" age every minute).
pub(crate) fn refresh_menu(app: &AppHandle) {
    let Some(menu) = app.try_state::<TasksMenu>() else { return };
    let now = Local::now();
    let (paired, mut open) = {
        let state = app.state::<AppState>();
        let s = state.lock();
        (s.token.is_some(), s.tasks.tasks.iter().filter(|x| x.status != "done").cloned().collect::<Vec<_>>())
    };
    t::sort(&mut open, now);
    let lines: Vec<(String, String)> = open.iter().take(MENU_TASKS).map(|x| (x.id.clone(), t::menu_label(x, now))).collect();
    let signature = format!("{paired}|{}|{}", open.len(), lines.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"));
    {
        let mut last = menu.last_shown.lock().unwrap_or_else(|e| e.into_inner());
        if *last == signature {
            return;
        }
        *last = signature;
    }

    let sub = &menu.submenu;
    while let Ok(Some(_)) = sub.remove_at(0) {}
    let mut items: Vec<Box<dyn IsMenuItem<Wry>>> = Vec::new();
    let item = |id: &str, text: &str, enabled: bool| MenuItem::with_id(app, id, text, enabled, None::<&str>);
    if let Ok(i) = item("tasks:detach", "Detach — keep Tasks on top", paired) {
        items.push(Box::new(i));
    }
    if let Ok(s) = PredefinedMenuItem::separator(app) {
        items.push(Box::new(s));
    }
    if !paired {
        if let Ok(i) = item("tasks:none", "Connect TrusCo Tracker to see tasks", false) {
            items.push(Box::new(i));
        }
    } else if lines.is_empty() {
        if let Ok(i) = item("tasks:none", "No open tasks", false) {
            items.push(Box::new(i));
        }
    } else {
        for (id, label) in &lines {
            if let Ok(i) = item(&format!("task:{id}"), label, true) {
                items.push(Box::new(i));
            }
        }
        if open.len() > MENU_TASKS {
            if let Ok(i) = item("tasks:open", &format!("{} more…", open.len() - MENU_TASKS), true) {
                items.push(Box::new(i));
            }
        }
    }
    if let Ok(s) = PredefinedMenuItem::separator(app) {
        items.push(Box::new(s));
    }
    if let Ok(i) = item("tasks:new", "New task…", paired) {
        items.push(Box::new(i));
    }
    if let Ok(i) = item("tasks:open", "Show all tasks…", paired) {
        items.push(Box::new(i));
    }
    for i in &items {
        let _ = sub.append(i.as_ref());
    }
}

/// Handles clicks on the submenu. Returns false for ids that aren't ours.
pub(crate) fn on_menu(app: &AppHandle, id: &str) -> bool {
    match id {
        "tasks:detach" => open_float(app),
        "tasks:new" => open_main(app, "tasks-new"),
        "tasks:open" => open_main(app, "tasks"),
        _ if id.starts_with("task:") => open_main(app, id),
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------
// macOS widget
// ---------------------------------------------------------------------------

pub(crate) fn write_widget(app: &AppHandle) {
    let Some(dir) = t::widget_dir() else { return };
    let snapshot = {
        let state = app.state::<AppState>();
        let s = state.lock();
        t::widget_snapshot(&s.tasks.tasks, s.token.is_some(), Local::now())
    };
    if t::write_widget_snapshot(&dir, &snapshot).is_ok() {
        crate::widget::reload();
    }
}
