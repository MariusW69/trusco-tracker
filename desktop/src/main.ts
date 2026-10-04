import { invoke } from "@tauri-apps/api/core";

type Account = {
  device_id: string;
  device_name: string;
  organization_id: string;
  organization_name: string;
  user_name: string;
};

type BlockView = {
  id: string;
  app_name: string;
  label: string;
  detail: string;
  started_at: string;
  ended_at: string;
  active_seconds: number;
  pages: number | null;
  matter_label: string | null;
  match_reason: string | null;
  status: "approved" | "discarded" | "synced" | "waiting" | "short";
};

type SettingsView = {
  server_url: string;
  idle_minutes: number;
  excluded_apps: string[];
  excluded_keywords: string[];
};

type UiState = {
  paired: boolean;
  account: Account | null;
  status: string;
  activity: { state: string };
  paused: boolean;
  today_seconds: number;
  blocks: BlockView[];
  last_sync: string | null;
  last_sync_error: string | null;
  accessibility: boolean;
  platform: string;
  version: string;
  settings: SettingsView;
  launch_at_login: boolean;
};

const app = document.querySelector<HTMLElement>("#app")!;
let state: UiState | null = null;
let view: "main" | "settings" = "main";
let busy = false;
let pairError = "";

const esc = (s: string | null | undefined) =>
  (s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

const duration = (secs: number) => {
  if (secs > 0 && secs < 60) return "<1m";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return h > 0 ? `${h}h ${String(m).padStart(2, "0")}m` : `${m}m`;
};

const clock = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

const ago = (iso: string | null) => {
  if (!iso) return "not yet";
  const secs = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  return clock(iso);
};

async function refresh() {
  if (busy) return;
  try {
    state = await invoke<UiState>("get_state");
    render();
  } catch (e) {
    console.error(e);
  }
}

async function act<T>(fn: () => Promise<T>) {
  busy = true;
  try {
    const next = await fn();
    if (next && typeof next === "object" && "paired" in next) state = next as unknown as UiState;
  } finally {
    busy = false;
  }
  await refresh();
}

function render() {
  if (!state) return;
  if (!state.paired) return renderPair(state);
  if (view === "settings") return renderSettings(state);
  renderMain(state);
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

async function renderPair(s: UiState) {
  if (app.dataset.view === "pair") return; // keep what the user is typing
  app.dataset.view = "pair";
  const name = await invoke<string>("default_device_name");
  app.innerHTML = `
    <div>
      <h1>Connect TrusCo Tracker</h1>
      <p class="muted" style="margin-top:6px">In TrusCo, open <b>Captured Time</b> and choose <b>Connect a device</b>. Enter the code shown there.</p>
    </div>
    ${s.last_sync_error ? `<div class="banner">${esc(s.last_sync_error)}</div>` : ""}
    <form class="card" id="pair-form">
      <div class="field">
        <label for="code">Connection code</label>
        <input id="code" class="code-input" autocomplete="off" spellcheck="false" maxlength="9" placeholder="XXXX-XXXX" required />
      </div>
      <div class="field">
        <label for="device">Name this computer</label>
        <input id="device" value="${esc(name)}" maxlength="80" />
      </div>
      <details class="field">
        <summary>Advanced</summary>
        <div style="margin-top:8px">
          <label for="server">TrusCo address</label>
          <input id="server" value="${esc(s.settings.server_url)}" />
        </div>
      </details>
      <p class="error" id="pair-error" style="margin-top:10px">${esc(pairError)}</p>
      <button class="primary" type="submit" style="width:100%;margin-top:6px">Connect</button>
    </form>
    <p class="small muted">TrusCo Tracker records which app and document you're working in, never what you type or what's on screen. Nothing is billed until you approve it in TrusCo.</p>
  `;
  const code = app.querySelector<HTMLInputElement>("#code")!;
  code.focus();
  code.addEventListener("input", () => {
    const raw = code.value.toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 8);
    code.value = raw.length > 4 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : raw;
  });
  app.querySelector<HTMLFormElement>("#pair-form")!.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const button = app.querySelector<HTMLButtonElement>("button[type=submit]")!;
    const errorEl = app.querySelector<HTMLElement>("#pair-error")!;
    button.disabled = true;
    button.textContent = "Connecting…";
    errorEl.textContent = "";
    try {
      state = await invoke<UiState>("pair", {
        code: code.value,
        deviceName: app.querySelector<HTMLInputElement>("#device")!.value,
        serverUrl: app.querySelector<HTMLInputElement>("#server")!.value,
      });
      pairError = "";
      app.dataset.view = "";
      render();
    } catch (e) {
      pairError = String(e);
      errorEl.textContent = pairError;
      button.disabled = false;
      button.textContent = "Connect";
    }
  });
}

// ---------------------------------------------------------------------------
// Today
// ---------------------------------------------------------------------------

function statusTag(b: BlockView) {
  switch (b.status) {
    case "approved":
      return `<span class="tag ok" title="Approved into a time entry in TrusCo">Approved</span>`;
    case "discarded":
      return `<span class="tag none">Discarded</span>`;
    case "short":
      return `<span class="tag none" title="Under a minute: kept on this computer only">Not sent</span>`;
    case "waiting":
      return `<span class="tag none">Sending…</span>`;
    default:
      return "";
  }
}

function renderMain(s: UiState) {
  app.dataset.view = "main";
  const tracking = s.activity.state === "tracking";
  const pill = s.paused
    ? `<span class="pill off">Paused</span>`
    : tracking
      ? `<span class="pill">Tracking</span>`
      : `<span class="pill off">${s.activity.state === "idle" ? "Idle" : "Waiting"}</span>`;

  const items = s.blocks
    .map(
      (b) => `
      <div class="item">
        <div class="title" title="${esc(b.detail)}">${esc(b.label)}</div>
        <div class="dur">${duration(b.active_seconds)}</div>
        <div class="meta">
          <span>${clock(b.started_at)}–${clock(b.ended_at)}</span>
          <span>${esc(b.app_name)}</span>
          ${b.pages ? `<span>${b.pages} pp</span>` : ""}
          ${
            b.matter_label
              ? `<span class="tag" title="${esc(b.match_reason)}">${esc(b.matter_label)}</span>`
              : b.status === "synced"
                ? `<span class="tag none">No matter yet</span>`
                : ""
          }
          ${statusTag(b)}
          ${b.status === "short" || b.status === "waiting" ? `<button class="link small" data-remove="${esc(b.id)}">Remove</button>` : ""}
        </div>
      </div>`,
    )
    .join("");

  app.innerHTML = `
    <div class="row spread">
      <h1>TrusCo Tracker</h1>
      <button class="link" id="settings">Settings</button>
    </div>
    ${
      !s.accessibility
        ? `<div class="banner"><b>Allow Accessibility access</b> so TrusCo Tracker can see which document is open (it never reads the contents).
           <div style="margin-top:8px"><button id="grant">Open System Settings</button></div></div>`
        : ""
    }
    ${s.last_sync_error ? `<div class="banner">${esc(s.last_sync_error)}</div>` : ""}
    <section class="card">
      <div class="row spread">${pill}<span class="small muted">Today</span></div>
      <div class="row spread" style="margin-top:8px">
        <div class="grow now muted">${esc(s.status)}</div>
        <div class="total">${duration(s.today_seconds)}</div>
      </div>
      <div class="row controls" style="margin-top:12px">
        ${
          s.paused
            ? `<button class="primary" id="resume">Resume tracking</button>`
            : `<span class="small muted">Pause</span><button id="p15">15 min</button><button id="p60">1 hour</button><button id="pinf">Until I resume</button>`
        }
      </div>
      <button class="primary" id="review" style="width:100%;margin-top:10px">Review in TrusCo</button>
    </section>
    <section class="card">
      <h2>Today's activity</h2>
      <div class="list">${items || `<div class="empty">Nothing recorded yet today.</div>`}</div>
    </section>
    <footer>
      <div>${esc(s.account?.organization_name)}${s.account?.user_name ? ` · ${esc(s.account.user_name)}` : ""}</div>
      <div>Synced ${ago(s.last_sync)} · <button class="link small" id="sync">Sync now</button></div>
    </footer>
  `;

  const on = (id: string, fn: () => void) => app.querySelector(`#${id}`)?.addEventListener("click", fn);
  on("settings", () => {
    view = "settings";
    render();
  });
  on("grant", () => invoke("request_accessibility"));
  on("resume", () => act(() => invoke("resume")));
  on("p15", () => act(() => invoke("pause", { minutes: 15 })));
  on("p60", () => act(() => invoke("pause", { minutes: 60 })));
  on("pinf", () => act(() => invoke("pause", { minutes: null })));
  on("review", () => invoke("open_review"));
  on("sync", () => act(() => invoke("sync_now")));
  app.querySelectorAll<HTMLButtonElement>("[data-remove]").forEach((b) =>
    b.addEventListener("click", () => act(() => invoke("remove_block", { id: b.dataset.remove }))),
  );
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

function renderSettings(s: UiState) {
  if (app.dataset.view === "settings") return; // don't clobber edits
  app.dataset.view = "settings";
  app.innerHTML = `
    <div class="row spread">
      <h1>Settings</h1>
      <button class="link" id="back">Done</button>
    </div>
    <form class="card" id="settings-form">
      <div class="field">
        <label for="idle">Pause after no keyboard or mouse activity for (minutes)</label>
        <input id="idle" type="number" min="1" max="120" value="${s.settings.idle_minutes}" />
        <p class="hint">Video calls keep counting while you only listen.</p>
      </div>
      <div class="field">
        <label for="apps">Never track these apps (one per line)</label>
        <textarea id="apps" rows="5">${esc(s.settings.excluded_apps.join("\n"))}</textarea>
      </div>
      <div class="field">
        <label for="keywords">Never track windows whose title contains (one per line)</label>
        <textarea id="keywords" rows="3">${esc(s.settings.excluded_keywords.join("\n"))}</textarea>
        <p class="hint">For example your bank's name, or "Personal".</p>
      </div>
      <div class="field row">
        <input id="login" type="checkbox" style="width:auto" ${s.launch_at_login ? "checked" : ""} />
        <label for="login" style="margin:0;font-weight:400">Start TrusCo Tracker when I log in</label>
      </div>
      <div class="field">
        <label for="server">TrusCo address</label>
        <input id="server" value="${esc(s.settings.server_url)}" />
      </div>
      <p class="error" id="settings-error" style="margin-top:10px"></p>
      <button class="primary" type="submit" style="margin-top:6px">Save</button>
    </form>
    <section class="card">
      <h2>This computer</h2>
      <p>${esc(s.account?.device_name)} · connected to <b>${esc(s.account?.organization_name)}</b></p>
      <p class="small muted" style="margin-top:4px">Disconnecting removes the activity stored on this computer. Anything already sent stays in TrusCo.</p>
      <button class="danger" id="disconnect" style="margin-top:10px">Disconnect this computer</button>
    </section>
    <footer>TrusCo Tracker ${esc(s.version)} · ${esc(s.platform)}</footer>
  `;

  const lines = (id: string) => app.querySelector<HTMLTextAreaElement>(`#${id}`)!.value.split("\n");
  app.querySelector("#back")!.addEventListener("click", () => {
    view = "main";
    render();
  });
  app.querySelector<HTMLFormElement>("#settings-form")!.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    try {
      state = await invoke<UiState>("save_settings", {
        settings: {
          server_url: app.querySelector<HTMLInputElement>("#server")!.value,
          idle_minutes: Number(app.querySelector<HTMLInputElement>("#idle")!.value) || 5,
          excluded_apps: lines("apps"),
          excluded_keywords: lines("keywords"),
        },
        launchAtLogin: app.querySelector<HTMLInputElement>("#login")!.checked,
      });
      view = "main";
      render();
    } catch (e) {
      app.querySelector("#settings-error")!.textContent = String(e);
    }
  });
  app.querySelector("#disconnect")!.addEventListener("click", async () => {
    if (!confirm("Disconnect this computer from TrusCo? Activity stored on this computer will be removed.")) return;
    state = await invoke<UiState>("disconnect");
    view = "main";
    app.dataset.view = "";
    render();
  });
}

refresh();
setInterval(() => {
  if (document.visibilityState === "visible") refresh();
}, 3000);
document.addEventListener("visibilitychange", refresh);
