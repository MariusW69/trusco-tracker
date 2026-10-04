use chrono::{DateTime, Duration, Local, Utc};
use serde::{Deserialize, Serialize};

/// Samples further apart than this mean the machine slept or the process stalled;
/// the gap is not counted as work.
const MAX_TICK_GAP_SECS: f64 = 30.0;

/// Blocks kept locally after they are synced, for the "Today" view and re-sync.
const KEEP_DAYS: i64 = 14;

/// What is in front of the user right now.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct WindowInfo {
    pub app_name: String,
    pub app_bundle: Option<String>,
    pub title: String,
    pub document_path: Option<String>,
    pub document_pages: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub at: DateTime<Utc>,
    pub idle_secs: f64,
    pub window: Option<WindowInfo>,
}

/// One continuous stretch of work on the same document / window, possibly with
/// short interruptions (up to `merge_gap_secs`) that are not counted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub key: String,
    pub app_name: String,
    pub app_bundle: Option<String>,
    pub window_title: String,
    pub document_path: Option<String>,
    pub document_pages: Option<u32>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub active_ms: u64,
    /// Bumped on every change; the block needs syncing while `version > synced_version`.
    pub version: u64,
    pub synced_version: u64,
    /// Set once TrusCo reports the block approved or discarded; further work starts a new block.
    pub frozen: bool,
    pub matter_label: Option<String>,
    pub match_reason: Option<String>,
    pub server_status: Option<String>,
}

impl Block {
    pub fn active_seconds(&self) -> u64 {
        self.active_ms / 1000
    }

    pub fn is_dirty(&self) -> bool {
        self.version > self.synced_version
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server_url: String,
    pub device_name: Option<String>,
    /// No keyboard/mouse input for this long pauses tracking (except in meetings).
    pub idle_threshold_secs: u64,
    /// Returning to the same document within this gap continues the same block.
    pub merge_gap_secs: u64,
    /// Blocks shorter than this stay on the device.
    pub min_sync_secs: u64,
    pub excluded_apps: Vec<String>,
    pub excluded_keywords: Vec<String>,
    pub paused_until: Option<DateTime<Utc>>,
    pub paused_indefinitely: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: "https://trusco.app".into(),
            device_name: None,
            idle_threshold_secs: 300,
            merge_gap_secs: 900,
            min_sync_secs: 60,
            excluded_apps: [
                "1Password",
                "Bitwarden",
                "LastPass",
                "KeePassXC",
                "Keychain Access",
                "Passwords",
                "TrusCo Tracker",
                "loginwindow",
                "ScreenSaverEngine",
            ]
            .map(String::from)
            .to_vec(),
            excluded_keywords: ["Incognito", "InPrivate", "Private Browsing"].map(String::from).to_vec(),
            paused_until: None,
            paused_indefinitely: false,
        }
    }
}

impl Settings {
    pub fn is_paused(&self, now: DateTime<Utc>) -> bool {
        self.paused_indefinitely || self.paused_until.is_some_and(|t| now < t)
    }

    pub fn is_excluded(&self, w: &WindowInfo) -> bool {
        let app = w.app_name.to_lowercase();
        if self.excluded_apps.iter().any(|a| !a.trim().is_empty() && app == a.trim().to_lowercase()) {
            return true;
        }
        let title = w.title.to_lowercase();
        self.excluded_keywords
            .iter()
            .any(|k| !k.trim().is_empty() && title.contains(&k.trim().to_lowercase()))
    }
}

/// What the tracker did with the latest sample (shown in the tray).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Activity {
    Paused,
    Idle,
    Excluded,
    NoWindow,
    Tracking { block_id: String },
}

#[derive(Default, Serialize, Deserialize)]
pub struct Engine {
    pub blocks: Vec<Block>,
    #[serde(skip)]
    last_tick: Option<DateTime<Utc>>,
}

impl Engine {
    pub fn tick(&mut self, s: &Sample, cfg: &Settings) -> Activity {
        let delta = match self.last_tick {
            Some(prev) => (s.at - prev).num_milliseconds().max(0) as f64 / 1000.0,
            None => 0.0,
        };
        self.last_tick = Some(s.at);
        let delta = if delta > MAX_TICK_GAP_SECS { 0.0 } else { delta };

        if cfg.is_paused(s.at) {
            return Activity::Paused;
        }
        let Some(w) = &s.window else {
            return Activity::NoWindow;
        };
        if cfg.is_excluded(w) {
            return Activity::Excluded;
        }
        if s.idle_secs >= cfg.idle_threshold_secs as f64 && !is_meeting(w) {
            return Activity::Idle;
        }

        let key = block_key(w);
        let day = s.at.with_timezone(&Local).date_naive();
        let gap = Duration::seconds(cfg.merge_gap_secs as i64);
        let existing = self.blocks.iter().rposition(|b| {
            !b.frozen
                && b.key == key
                && s.at - b.ended_at <= gap
                && b.started_at.with_timezone(&Local).date_naive() == day
        });
        let idx = match existing {
            Some(i) => i,
            None => {
                self.blocks.push(Block {
                    id: uuid::Uuid::new_v4().to_string(),
                    key,
                    app_name: w.app_name.clone(),
                    app_bundle: w.app_bundle.clone(),
                    window_title: w.title.clone(),
                    document_path: None,
                    document_pages: None,
                    started_at: s.at,
                    ended_at: s.at,
                    active_ms: 0,
                    version: 0,
                    synced_version: 0,
                    frozen: false,
                    matter_label: None,
                    match_reason: None,
                    server_status: None,
                });
                self.blocks.len() - 1
            }
        };

        let b = &mut self.blocks[idx];
        b.active_ms += (delta * 1000.0) as u64;
        b.ended_at = s.at;
        if !w.title.trim().is_empty() {
            b.window_title = w.title.clone();
        }
        if w.document_path.is_some() {
            b.document_path = w.document_path.clone();
        }
        if w.document_pages.is_some() {
            b.document_pages = w.document_pages;
        }
        b.version += 1;
        Activity::Tracking { block_id: b.id.clone() }
    }

    /// Blocks worth sending: changed since the last sync and long enough to matter.
    pub fn pending_sync(&self, cfg: &Settings, limit: usize) -> Vec<Block> {
        self.blocks
            .iter()
            .filter(|b| !b.frozen && b.is_dirty() && b.active_seconds() >= cfg.min_sync_secs)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Record the server's answer for a block that was sent at `sent_version`.
    pub fn mark_synced(
        &mut self,
        id: &str,
        sent_version: u64,
        status: Option<String>,
        matter_label: Option<String>,
        match_reason: Option<String>,
    ) {
        if let Some(b) = self.blocks.iter_mut().find(|b| b.id == id) {
            b.synced_version = b.synced_version.max(sent_version);
            if matches!(status.as_deref(), Some("approved") | Some("discarded")) {
                b.frozen = true;
            }
            b.server_status = status;
            b.matter_label = matter_label;
            b.match_reason = match_reason;
        }
    }

    /// Drop old blocks that no longer need syncing.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now - Duration::days(KEEP_DAYS);
        self.blocks.retain(|b| b.ended_at >= cutoff || (b.is_dirty() && !b.frozen));
    }

    pub fn today(&self) -> Vec<&Block> {
        let today = Local::now().date_naive();
        self.blocks
            .iter()
            .filter(|b| b.started_at.with_timezone(&Local).date_naive() == today)
            .collect()
    }

    pub fn today_seconds(&self) -> u64 {
        self.today().iter().map(|b| b.active_seconds()).sum()
    }
}

/// Calls and meetings legitimately have no keyboard/mouse input.
pub fn is_meeting(w: &WindowInfo) -> bool {
    let app = w.app_name.to_lowercase();
    let title = w.title.to_lowercase();
    app.starts_with("zoom")
        || app.contains("webex")
        || app == "facetime"
        || (app.contains("teams") && (title.contains("meeting") || title.contains("call")))
        || title.contains("google meet")
        || title.starts_with("meet - ")
}

/// Same app + same document (or same window title when there is no document) = same block.
pub fn block_key(w: &WindowInfo) -> String {
    let what = match &w.document_path {
        Some(p) if !p.trim().is_empty() => p.trim().to_lowercase(),
        _ => normalize_title(&w.title),
    };
    format!("{}|{}", w.app_name.to_lowercase(), what)
}

fn normalize_title(title: &str) -> String {
    let mut t = title.trim().to_lowercase();
    // "(3) Inbox - Gmail" -> "inbox - gmail"
    if t.starts_with('(') {
        if let Some(end) = t.find(") ") {
            if t[1..end].chars().all(|c| c.is_ascii_digit()) {
                t = t[end + 2..].to_string();
            }
        }
    }
    for suffix in [" — edited", " - edited", " [compatibility mode]", " [read-only]"] {
        if let Some(stripped) = t.strip_suffix(suffix) {
            t = stripped.to_string();
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(title: &str, path: Option<&str>) -> WindowInfo {
        WindowInfo {
            app_name: "Microsoft Word".into(),
            app_bundle: Some("com.microsoft.Word".into()),
            title: title.into(),
            document_path: path.map(String::from),
            document_pages: None,
        }
    }

    fn at(base: DateTime<Utc>, secs: i64) -> DateTime<Utc> {
        base + Duration::seconds(secs)
    }

    fn base() -> DateTime<Utc> {
        // Mid-morning local time so the day never rolls over during a test.
        let today = Local::now().date_naive().and_hms_opt(9, 0, 0).unwrap();
        today.and_local_timezone(Local).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn accumulates_time_on_same_document() {
        let cfg = Settings::default();
        let mut e = Engine::default();
        let w = word("Settlement.docx", Some("/Clients/Smit/Settlement.docx"));
        for i in 0..=12 {
            e.tick(&Sample { at: at(base(), i * 5), idle_secs: 1.0, window: Some(w.clone()) }, &cfg);
        }
        assert_eq!(e.blocks.len(), 1);
        assert_eq!(e.blocks[0].active_seconds(), 60);
    }

    #[test]
    fn short_interruption_resumes_same_block_without_counting_it() {
        let cfg = Settings::default();
        let mut e = Engine::default();
        let doc = word("Settlement.docx", Some("/Clients/Smit/Settlement.docx"));
        let mail = WindowInfo { app_name: "Microsoft Outlook".into(), title: "Inbox".into(), ..Default::default() };
        let mut t = 0;
        for w in [&doc, &doc, &doc, &mail, &mail, &doc, &doc] {
            e.tick(&Sample { at: at(base(), t), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
            t += 5;
        }
        assert_eq!(e.blocks.len(), 2);
        let d = e.blocks.iter().find(|b| b.app_name == "Microsoft Word").unwrap();
        assert_eq!(d.active_seconds(), 20); // 3 ticks + 2 ticks after returning, minus the first (no delta)
        let m = e.blocks.iter().find(|b| b.app_name == "Microsoft Outlook").unwrap();
        assert_eq!(m.active_seconds(), 10);
    }

    #[test]
    fn idle_and_sleep_are_not_counted() {
        let cfg = Settings::default();
        let mut e = Engine::default();
        let w = word("Settlement.docx", None);
        e.tick(&Sample { at: at(base(), 0), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
        e.tick(&Sample { at: at(base(), 5), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
        assert_eq!(
            e.tick(&Sample { at: at(base(), 10), idle_secs: 400.0, window: Some(w.clone()) }, &cfg),
            Activity::Idle
        );
        // laptop lid closed for an hour
        e.tick(&Sample { at: at(base(), 3610), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
        assert_eq!(e.blocks[0].active_seconds(), 5);
    }

    #[test]
    fn meetings_ignore_idle() {
        let cfg = Settings::default();
        let mut e = Engine::default();
        let zoom = WindowInfo { app_name: "zoom.us".into(), title: "Zoom Meeting".into(), ..Default::default() };
        e.tick(&Sample { at: at(base(), 0), idle_secs: 900.0, window: Some(zoom.clone()) }, &cfg);
        e.tick(&Sample { at: at(base(), 5), idle_secs: 905.0, window: Some(zoom) }, &cfg);
        assert_eq!(e.blocks[0].active_seconds(), 5);
    }

    #[test]
    fn excluded_apps_and_pause_record_nothing() {
        let mut cfg = Settings::default();
        let mut e = Engine::default();
        let pw = WindowInfo { app_name: "1Password".into(), title: "Vault".into(), ..Default::default() };
        assert_eq!(e.tick(&Sample { at: base(), idle_secs: 0.0, window: Some(pw) }, &cfg), Activity::Excluded);
        cfg.paused_until = Some(at(base(), 600));
        let w = word("Settlement.docx", None);
        assert_eq!(e.tick(&Sample { at: at(base(), 5), idle_secs: 0.0, window: Some(w) }, &cfg), Activity::Paused);
        assert!(e.blocks.is_empty());
    }

    #[test]
    fn long_gap_starts_new_block() {
        let cfg = Settings::default();
        let mut e = Engine::default();
        let w = word("Settlement.docx", Some("/x/Settlement.docx"));
        e.tick(&Sample { at: at(base(), 0), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
        e.tick(&Sample { at: at(base(), 3600), idle_secs: 0.0, window: Some(w) }, &cfg);
        assert_eq!(e.blocks.len(), 2);
    }

    #[test]
    fn sync_bookkeeping() {
        let cfg = Settings::default();
        let mut e = Engine::default();
        let w = word("Settlement.docx", Some("/x/Settlement.docx"));
        for i in 0..=20 {
            e.tick(&Sample { at: at(base(), i * 5), idle_secs: 0.0, window: Some(w.clone()) }, &cfg);
        }
        let pending = e.pending_sync(&cfg, 100);
        assert_eq!(pending.len(), 1);
        let (id, v) = (pending[0].id.clone(), pending[0].version);
        e.mark_synced(&id, v, Some("pending".into()), Some("M-1004 · Divorce".into()), None);
        assert!(e.pending_sync(&cfg, 100).is_empty());
        e.mark_synced(&id, v, Some("approved".into()), None, None);
        e.tick(&Sample { at: at(base(), 110), idle_secs: 0.0, window: Some(w) }, &cfg);
        assert_eq!(e.blocks.len(), 2, "work after approval starts a fresh block");
    }

    #[test]
    fn title_normalisation() {
        assert_eq!(normalize_title("(3) Inbox - Gmail"), "inbox - gmail");
        assert_eq!(normalize_title("Memo — Edited"), "memo");
        assert_eq!(normalize_title("(Draft) Memo"), "(draft) memo");
    }
}
