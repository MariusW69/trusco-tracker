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

const handlers: Record<string, (a: any) => any> = {
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
