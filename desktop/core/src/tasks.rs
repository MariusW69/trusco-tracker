//! Project tasks: urgency, labels and ordering shared by the tray menu, the windows and the
//! macOS widget, plus the snapshot file the widget reads from the shared App Group folder.

use chrono::{DateTime, Duration, Local, NaiveDate, SecondsFormat, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::api::{Project, Task};

/// Timed tasks due within this many hours are "due soon".
pub const DUE_SOON_HOURS: i64 = 3;

/// Shared with the widget extension; team-ID prefixed so no provisioning profile is needed.
pub const APP_GROUP: &str = "H7567N8H2N.app.trusco.tracker";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Overdue,
    Soon,
    Today,
    Upcoming,
    NoDate,
    Done,
}

/// When a task is due: its due time, or the end of its due day for date-only tasks.
pub fn due(task: &Task) -> Option<DateTime<Local>> {
    if let Some(at) = task.due_at {
        return Some(at.with_timezone(&Local));
    }
    task.due_date.and_then(end_of_day)
}

fn end_of_day(d: NaiveDate) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&d.and_hms_opt(23, 59, 59)?).earliest()
}

pub fn urgency(task: &Task, now: DateTime<Local>) -> Urgency {
    if task.status == "done" {
        return Urgency::Done;
    }
    let Some(due) = due(task) else { return Urgency::NoDate };
    if due < now {
        Urgency::Overdue
    } else if task.due_at.is_some() && due - now <= Duration::hours(DUE_SOON_HOURS) {
        Urgency::Soon
    } else if due.date_naive() == now.date_naive() {
        Urgency::Today
    } else {
        Urgency::Upcoming
    }
}

/// "Overdue · Mon 15:00", "Today 15:00 · in 45m", "Tomorrow", "Thu 09:00", "23 Oct".
pub fn due_label(task: &Task, now: DateTime<Local>) -> String {
    let Some(due) = due(task) else { return "No due date".into() };
    let timed = task.due_at.is_some();
    let time = if timed { due.format(" %H:%M").to_string() } else { String::new() };
    let day = day_label(due.date_naive(), now.date_naive());
    match urgency(task, now) {
        Urgency::Overdue => format!("Overdue · {}{time}", day.to_lowercase_first()),
        Urgency::Soon => format!("{day}{time} · in {}", short_duration(due - now)),
        _ => format!("{day}{time}"),
    }
}

fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    let days = (day - today).num_days();
    match days {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        2..=6 | -6..=-2 => day.format("%a").to_string(),
        _ => day.format("%-d %b").to_string(),
    }
}

trait LowerFirst {
    fn to_lowercase_first(&self) -> String;
}

impl LowerFirst for String {
    /// "Today" -> "today" but "Mon" stays "Mon".
    fn to_lowercase_first(&self) -> String {
        match self.as_str() {
            "Today" | "Tomorrow" | "Yesterday" => self.to_lowercase(),
            _ => self.clone(),
        }
    }
}

/// "45m", "2h", "2h 30m".
pub fn short_duration(d: Duration) -> String {
    let mins = d.num_minutes().max(0);
    let (h, m) = (mins / 60, mins % 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Time budget of an "allocate time" task, e.g. "2h 30m".
pub fn budget_label(task: &Task) -> Option<String> {
    (task.kind == "time" && task.estimated_hours > 0.0).then(|| short_duration(budget(task)))
}

fn budget(task: &Task) -> Duration {
    Duration::minutes((task.estimated_hours * 60.0).round() as i64)
}

/// The planned slot of an "allocate time" task, e.g. "Thu 09:00–11:30".
pub fn slot_label(task: &Task, now: DateTime<Local>) -> Option<String> {
    let start = task.scheduled_at?.with_timezone(&Local);
    let day = day_label(start.date_naive(), now.date_naive());
    let mut label = format!("{day} {}", start.format("%H:%M"));
    if task.estimated_hours > 0.0 {
        label.push_str(&(start + budget(task)).format("–%H:%M").to_string());
    }
    Some(label)
}

/// Overdue first, then by due time, high priority before normal, done last.
pub fn sort(tasks: &mut [Task], now: DateTime<Local>) {
    tasks.sort_by(|a, b| {
        urgency(a, now)
            .cmp(&urgency(b, now))
            .then_with(|| due(a).cmp(&due(b)))
            .then_with(|| (b.priority == "high").cmp(&(a.priority == "high")))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
}

pub fn project_label(task: &Task) -> String {
    format!("{} · {}", task.project_number, task.project_name)
}

/// Line in the tray "Tasks" menu, e.g. "🔴 ❗ Call client — Overdue · Mon 15:00".
pub fn menu_label(task: &Task, now: DateTime<Local>) -> String {
    let dot = match urgency(task, now) {
        Urgency::Overdue => "🔴",
        Urgency::Soon | Urgency::Today => "🟠",
        Urgency::Done => "✅",
        _ => "⚪️",
    };
    let flag = if task.priority == "high" { "❗ " } else { "" };
    let title: String = if task.title.chars().count() > 40 {
        format!("{}…", task.title.chars().take(39).collect::<String>())
    } else {
        task.title.clone()
    };
    format!("{dot} {flag}{title} — {}", due_label(task, now))
}

// ---------------------------------------------------------------------------
// Form values (local date "2026-10-22" + time "15:00") <-> stored instants
// ---------------------------------------------------------------------------

/// Local date + optional time -> UTC instant (None without a time, or for an invalid value).
pub fn local_to_utc(date: Option<&str>, time: Option<&str>) -> Option<DateTime<Utc>> {
    let d = NaiveDate::parse_from_str(date?.trim(), "%Y-%m-%d").ok()?;
    let t = chrono::NaiveTime::parse_from_str(time?.trim(), "%H:%M").ok()?;
    Local.from_local_datetime(&d.and_time(t)).earliest().map(|l| l.with_timezone(&Utc))
}

/// Instant -> local ("2026-10-22", "15:00") for the edit form.
pub fn utc_to_local_parts(t: DateTime<Utc>) -> (String, String) {
    let l = t.with_timezone(&Local);
    (l.format("%Y-%m-%d").to_string(), l.format("%H:%M").to_string())
}

// ---------------------------------------------------------------------------
// Local cache (so tasks show offline and at start-up)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskCache {
    pub tasks: Vec<Task>,
    pub projects: Vec<Project>,
    pub synced_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Widget snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct WidgetTask {
    pub id: String,
    pub title: String,
    pub project: String,
    /// Effective due moment (end of day for date-only tasks), RFC 3339 without fractions.
    pub due: Option<String>,
    pub due_has_time: bool,
    pub kind: String,
    pub priority: String,
    pub hours: f64,
    pub slot: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WidgetSnapshot {
    pub generated_at: String,
    pub paired: bool,
    pub tasks: Vec<WidgetTask>,
}

fn rfc3339(t: DateTime<Utc>) -> String {
    // Swift's ISO-8601 decoder rejects fractional seconds.
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Open tasks in display order (the widget works out urgency itself as time passes).
pub fn widget_snapshot(tasks: &[Task], paired: bool, now: DateTime<Local>) -> WidgetSnapshot {
    let mut open: Vec<Task> = tasks.iter().filter(|t| t.status != "done").cloned().collect();
    sort(&mut open, now);
    WidgetSnapshot {
        generated_at: rfc3339(now.with_timezone(&Utc)),
        paired,
        tasks: open
            .iter()
            .take(20)
            .map(|t| WidgetTask {
                id: t.id.clone(),
                title: t.title.clone(),
                project: t.project_name.clone(),
                due: due(t).map(|d| rfc3339(d.with_timezone(&Utc))),
                due_has_time: t.due_at.is_some(),
                kind: t.kind.clone(),
                priority: t.priority.clone(),
                hours: t.estimated_hours,
                slot: t.scheduled_at.map(rfc3339),
            })
            .collect(),
    }
}

/// ~/Library/Group Containers/<group>/ on macOS; widgets don't exist elsewhere.
pub fn widget_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Group Containers").join(APP_GROUP))
}

pub fn write_widget_snapshot(dir: &Path, snapshot: &WidgetSnapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("tasks.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(snapshot)?)?;
    std::fs::rename(tmp, dir.join("tasks.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap() // a Thursday
    }

    fn task(title: &str) -> Task {
        Task {
            id: title.into(),
            project_id: "p".into(),
            project_number: "PRJ-1014".into(),
            project_name: "Website".into(),
            title: title.into(),
            kind: "todo".into(),
            priority: "normal".into(),
            status: "todo".into(),
            due_at: None,
            due_date: None,
            scheduled_at: None,
            estimated_hours: 0.0,
            completed_at: None,
        }
    }

    fn at(t: &str, hours_from_now: i64) -> Task {
        Task { due_at: Some((now() + Duration::hours(hours_from_now)).with_timezone(&Utc)), ..task(t) }
    }

    fn on(t: &str, days_from_today: i64) -> Task {
        Task { due_date: Some(now().date_naive() + Duration::days(days_from_today)), ..task(t) }
    }

    #[test]
    fn urgency_from_due_time_and_date() {
        let n = now();
        assert_eq!(urgency(&at("late", -1), n), Urgency::Overdue);
        assert_eq!(urgency(&at("soon", 2), n), Urgency::Soon);
        assert_eq!(urgency(&at("tonight", 5), n), Urgency::Today);
        assert_eq!(urgency(&at("tomorrow", 24), n), Urgency::Upcoming);
        assert_eq!(urgency(&on("date today", 0), n), Urgency::Today, "date-only tasks are due at the end of the day");
        assert_eq!(urgency(&on("date yesterday", -1), n), Urgency::Overdue);
        assert_eq!(urgency(&task("someday"), n), Urgency::NoDate);
        assert_eq!(urgency(&Task { status: "done".into(), ..at("late", -1) }, n), Urgency::Done);
    }

    #[test]
    fn labels() {
        let n = now();
        assert_eq!(due_label(&at("a", -3), n), "Overdue · today 09:00");
        assert_eq!(due_label(&on("b", -3), n), "Overdue · Mon");
        assert_eq!(due_label(&at("c", 2), n), "Today 14:00 · in 2h");
        assert_eq!(due_label(&at("d", 6), n), "Today 18:00");
        assert_eq!(due_label(&at("e", 21), n), "Tomorrow 09:00");
        assert_eq!(due_label(&on("f", 3), n), "Sun");
        assert_eq!(due_label(&on("g", 10), n), "1 Nov");
        assert_eq!(due_label(&task("h"), n), "No due date");

        let slot = Task {
            kind: "time".into(),
            estimated_hours: 2.5,
            scheduled_at: Some((n + Duration::hours(21)).with_timezone(&Utc)),
            ..task("draft")
        };
        assert_eq!(budget_label(&slot).as_deref(), Some("2h 30m"));
        assert_eq!(slot_label(&slot, n).as_deref(), Some("Tomorrow 09:00–11:30"));
        assert_eq!(budget_label(&task("todo")), None);
        assert_eq!(short_duration(Duration::minutes(45)), "45m");
    }

    #[test]
    fn ordering_and_menu_label() {
        let n = now();
        let mut list = vec![
            task("someday"),
            on("next week", 6),
            Task { priority: "high".into(), ..at("urgent later", 24) },
            at("same time normal", 24),
            at("late", -1),
            Task { status: "done".into(), ..at("finished", -5) },
            at("soon", 1),
        ];
        sort(&mut list, n);
        let order: Vec<&str> = list.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(order, ["late", "soon", "urgent later", "same time normal", "next week", "someday", "finished"]);
        assert_eq!(menu_label(&list[2], n), "⚪️ ❗ urgent later — Tomorrow 12:00");
        assert_eq!(menu_label(&list[0], n), "🔴 late — Overdue · today 11:00");
    }

    #[test]
    fn form_values_round_trip() {
        let utc = local_to_utc(Some("2026-10-22"), Some("15:00")).unwrap();
        assert_eq!(utc_to_local_parts(utc), ("2026-10-22".to_string(), "15:00".to_string()));
        assert_eq!(local_to_utc(Some("2026-10-22"), None), None);
        assert_eq!(local_to_utc(Some("22/10/2026"), Some("15:00")), None);
        assert_eq!(local_to_utc(None, Some("15:00")), None);
    }

    #[test]
    fn widget_snapshot_is_swift_friendly() {
        let n = now();
        let tasks = vec![
            Task { due_at: Some(Utc.with_ymd_and_hms(2026, 10, 22, 13, 0, 0).unwrap() + Duration::milliseconds(250)), ..task("timed") },
            on("dated", 1),
            Task { status: "done".into(), ..task("done") },
        ];
        let snap = widget_snapshot(&tasks, true, n);
        assert_eq!(snap.tasks.len(), 2, "done tasks are left out");
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains(r#""due":"2026-10-22T13:00:00Z""#), "no fractional seconds: {json}");
        assert!(snap.tasks[1].due.is_some() && !snap.tasks[1].due_has_time);

        let dir = std::env::temp_dir().join(format!("tt-widget-{}", std::process::id()));
        write_widget_snapshot(&dir, &snap).unwrap();
        assert!(dir.join("tasks.json").exists());
        std::fs::remove_dir_all(dir).ok();
    }
}
