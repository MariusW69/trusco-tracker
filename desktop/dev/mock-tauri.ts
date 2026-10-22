// Dev-only stand-in for window.__TAURI_INTERNALS__ (used by @tauri-apps/api invoke).
// ?paired=1 shows the main view with sample data; ?ax=0 shows the Accessibility banner.
const params = new URLSearchParams(location.search);
const now = Date.now();
const iso = (minsAgo: number) => new Date(now - minsAgo * 60_000).toISOString();

const state: any = {
  paired: params.get("paired") === "1",
  account: { device_id: "d1", device_name: "Office MacBook Pro", organization_id: "o1", organization_name: "Example Attorneys Inc", user_name: "A. Attorney" },
  status: "Tracking: Jane Smit - Divorce Settlement.docx",
  activity: { state: "tracking", block_id: "b1" },
  paused: false,
  today_seconds: 4 * 3600 + 17 * 60,
  blocks: [
    { id: "b1", app_name: "Microsoft Word", label: "Jane Smit - Divorce Settlement.docx", detail: "/Users/attorney/Clients/Smit/Jane Smit - Divorce Settlement.docx", started_at: iso(185), ended_at: iso(1), active_seconds: 2 * 3600 + 51 * 60, pages: 14, matter_label: "M-1004 · Divorce Settlement", match_reason: 'Client name "jane smit" + "settlement"', status: "synced" },
    { id: "b2", app_name: "Microsoft Outlook", label: "RE: Settlement proposal – Smit", detail: "RE: Settlement proposal – Smit", started_at: iso(200), ended_at: iso(188), active_seconds: 11 * 60, pages: null, matter_label: "M-1004 · Divorce Settlement", match_reason: 'Client name "smit"', status: "synced" },
    { id: "b3", app_name: "PDF Expert", label: "Opposing heads of argument.pdf", detail: "/Users/attorney/Downloads/Opposing heads of argument.pdf", started_at: iso(260), ended_at: iso(205), active_seconds: 48 * 60, pages: null, matter_label: null, match_reason: null, status: "synced" },
    { id: "b4", app_name: "Google Chrome", label: "Inbox (3) - attorney@example.co.za - Gmail", detail: "Inbox (3) - attorney@example.co.za - Gmail", started_at: iso(270), ended_at: iso(262), active_seconds: 45, pages: null, matter_label: null, match_reason: null, status: "short" },
    { id: "b5", app_name: "Microsoft Word", label: "M-1003 Deed of Donation.docx", detail: "/Users/attorney/Clients/Botha/M-1003 Deed of Donation.docx", started_at: iso(330), ended_at: iso(275), active_seconds: 26 * 60, pages: 5, matter_label: "M-1003 · Estate & Succession", match_reason: "Matter number M-1003", status: "approved" },
  ],
  last_sync: iso(0.5),
  last_sync_error: null,
  accessibility: params.get("ax") !== "0",
  platform: "macos",
  version: "0.1.0",
  settings: { server_url: "https://trusco.app", idle_minutes: 5, excluded_apps: ["1Password", "Bitwarden", "Keychain Access", "Passwords", "TrusCo Tracker"], excluded_keywords: ["Incognito", "InPrivate", "Private Browsing"] },
  launch_at_login: true,
};

// Mirror of Block::never_track_rule: file name for documents, title without "(3)" counts otherwise.
for (const b of state.blocks) {
  b.never_track_app = b.app_name;
  const item = b.detail.startsWith("/") ? b.detail.split(/[\\/]/).pop() : b.label.replace(/\(\d+\)/g, "").replace(/\s+/g, " ").trim();
  b.never_track_item = item && item.length >= 3 ? item : null;
}

// Sample tasks for the Tasks tab / detached window (labels are pre-computed by Rust in the real app).
const projects = [
  { id: "p1", number: "PRJ-1014", name: "Smit divorce", client: "Jane Smit" },
  { id: "p2", number: "PRJ-1013", name: "Botha estate", client: "Botha family" },
  { id: "p3", number: "PRJ-1012", name: "Website refresh", client: null },
];
const mk = (id: string, title: string, p: number, extra: any) => ({
  id, project_id: projects[p].id, project: `${projects[p].number} · ${projects[p].name}`, title, kind: "todo", priority: "normal",
  done: false, urgency: "upcoming", due_label: "No due date", budget: null, slot: null, due_date: null, due_time: null,
  slot_date: null, slot_time: null, estimated_hours: 0, ...extra,
});
const tasksState: any = {
  paired: true,
  projects,
  synced_at: iso(1),
  error: null,
  float_open: false,
  tasks: [
    mk("t1", "Call Jane Smit about the settlement offer", 0, { priority: "high", urgency: "overdue", due_label: "Overdue · today 09:00", due_date: "2026-10-22", due_time: "09:00" }),
    mk("t2", "File founding affidavit", 0, { urgency: "soon", due_label: "Today 14:00 · in 2h", due_date: "2026-10-22", due_time: "14:00" }),
    mk("t3", "Draft parenting plan", 0, { kind: "time", urgency: "upcoming", due_label: "Fri", due_date: "2026-10-23", budget: "2h 30m", slot: "Tomorrow 09:00–11:30", slot_date: "2026-10-23", slot_time: "09:00", estimated_hours: 2.5 }),
    mk("t4", "Send fee note to the Botha family", 1, { urgency: "upcoming", due_label: "Mon", due_date: "2026-10-26" }),
    mk("t5", "Update website copy", 2, { urgency: "no_date" }),
    mk("t6", "Book counsel for the hearing", 0, { done: true, urgency: "done", due_label: "Overdue · Tue" }),
  ],
};

const handlers: Record<string, (a: any) => any> = {
  get_tasks: () => tasksState,
  refresh_tasks: () => tasksState,
  set_task_done: (a) => {
    const t = tasksState.tasks.find((x: any) => x.id === a.id);
    t.done = a.done;
    t.urgency = a.done ? "done" : "upcoming";
    return tasksState;
  },
  save_task: (a) => {
    const f = a.form;
    const p = projects.find((x) => x.id === f.project_id)!;
    const fields = {
      project_id: p.id, project: `${p.number} · ${p.name}`, title: f.title, kind: f.kind, priority: f.high_priority ? "high" : "normal",
      due_date: f.due_date, due_time: f.due_time, due_label: f.due_date ? `${f.due_date}${f.due_time ? " " + f.due_time : ""}` : "No due date",
      urgency: f.due_date ? "upcoming" : "no_date", estimated_hours: f.estimated_hours ?? 0,
      budget: f.kind === "time" && f.estimated_hours ? `${f.estimated_hours}h` : null,
      slot_date: f.slot_date, slot_time: f.slot_time, slot: f.slot_date ? `${f.slot_date} ${f.slot_time ?? ""}` : null,
    };
    const existing = tasksState.tasks.find((x: any) => x.id === f.id);
    if (existing) Object.assign(existing, fields);
    else tasksState.tasks.push(mk(`t${Date.now()}`, f.title, 0, fields));
    return tasksState;
  },
  create_project: (a) => {
    const p = { id: `p${Date.now()}`, number: `PRJ-10${15 + projects.length}`, name: a.name, client: null };
    projects.unshift(p);
    return p;
  },
  detach_tasks: () => {
    window.open("/dev/preview.html?paired=1#float", "tasks-float", "width=300,height=440");
  },
  never_track: (a) => {
    const blk = state.blocks.find((b: any) => b.id === a.id);
    const rule = a.wholeApp ? { kind: "app", value: blk.app_name } : { kind: "keyword", value: blk.never_track_item };
    const hit = (b: any) =>
      rule.kind === "app" ? b.app_name === rule.value : `${b.label} ${b.detail}`.toLowerCase().includes(rule.value.toLowerCase());
    const removed = state.blocks.filter(hit);
    state.blocks = state.blocks.filter((b: any) => !hit(b));
    (rule.kind === "app" ? state.settings.excluded_apps : state.settings.excluded_keywords).push(rule.value);
    return { state, rule, removed: removed.length, already_sent: removed.filter((b: any) => b.status !== "short" && b.status !== "waiting").length };
  },
  get_state: () => state,
  default_device_name: () => "Office MacBook Pro",
  pair: (a) => {
    if (a.code.replace(/-/g, "") !== "K7QF3MXA") throw "That code is invalid or has expired";
    state.paired = true;
    return state;
  },
  pause: (a) => ((state.paused = true), (state.status = a.minutes ? `Paused until ${new Date(now + a.minutes * 60000).toTimeString().slice(0, 5)}` : "Paused"), state),
  resume: () => ((state.paused = false), (state.status = "Tracking: Jane Smit - Divorce Settlement.docx"), state),
  save_settings: (a) => ((state.settings = a.settings), (state.launch_at_login = a.launchAtLogin), state),
  disconnect: () => ((state.paired = false), state),
  remove_block: (a) => ((state.blocks = state.blocks.filter((b: any) => b.id !== a.id)), state),
};

(window as any).__TAURI_INTERNALS__ = {
  transformCallback: () => 0,
  invoke: async (cmd: string, args: any) => {
    const h = handlers[cmd];
    return h ? h(args ?? {}) : null;
  },
};
