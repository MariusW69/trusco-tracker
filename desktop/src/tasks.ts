// Tasks panel: used by the main window's Tasks tab and by the detached, always-on-top window.
import { invoke } from "@tauri-apps/api/core";

export type Urgency = "overdue" | "soon" | "today" | "upcoming" | "no_date" | "done";

export type TaskView = {
  id: string;
  project_id: string;
  project: string;
  title: string;
  kind: "todo" | "time";
  priority: "normal" | "high";
  done: boolean;
  urgency: Urgency;
  due_label: string;
  budget: string | null;
  slot: string | null;
  due_date: string | null;
  due_time: string | null;
  slot_date: string | null;
  slot_time: string | null;
  estimated_hours: number;
};

export type Project = { id: string; number: string; name: string; client: string | null };

export type TasksState = {
  paired: boolean;
  tasks: TaskView[];
  projects: Project[];
  synced_at: string | null;
  error: string | null;
  float_open: boolean;
};

type Draft = {
  id: string | null;
  project_id: string;
  title: string;
  kind: "todo" | "time";
  high_priority: boolean;
  due_date: string;
  due_time: string;
  estimated_hours: string;
  slot_date: string;
  slot_time: string;
};

const GROUPS: { title: string; match: (u: Urgency) => boolean }[] = [
  { title: "Overdue", match: (u) => u === "overdue" },
  { title: "Today", match: (u) => u === "soon" || u === "today" },
  { title: "Upcoming", match: (u) => u === "upcoming" },
  { title: "No due date", match: (u) => u === "no_date" },
  { title: "Done today", match: (u) => u === "done" },
];

const LAST_PROJECT_KEY = "trusco-tracker:last-project";

const esc = (s: string | null | undefined) =>
  (s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

function lastProject(): string {
  try {
    return localStorage.getItem(LAST_PROJECT_KEY) ?? "";
  } catch {
    return "";
  }
}

function rememberProject(id: string) {
  try {
    localStorage.setItem(LAST_PROJECT_KEY, id);
  } catch {
    /* storage unavailable: just don't remember */
  }
}

export class TasksPanel {
  private state: TasksState | null = null;
  private draft: Draft | null = null;
  private newProject = false;
  private highlight: string | null = null;
  private message: string | null = null;
  private saving = false;
  private lastHtml = "";

  constructor(
    private root: HTMLElement,
    private mode: "main" | "float",
  ) {}

  /** Overdue + due-soon count, for the tab badge. */
  attentionCount(): number {
    return this.state?.tasks.filter((t) => t.urgency === "overdue" || t.urgency === "soon").length ?? 0;
  }

  async load(fromServer = false) {
    try {
      this.state = await invoke<TasksState>(fromServer ? "refresh_tasks" : "get_tasks");
      if (fromServer) this.message = null;
    } catch (e) {
      this.message = String(e);
      if (!this.state) this.state = await invoke<TasksState>("get_tasks");
    }
    this.render();
  }

  /** "tasks", "tasks-new" or "task:<id>" from the tray menu. */
  navigate(target: string) {
    if (target === "tasks-new") this.openForm(null);
    else if (target.startsWith("task:")) {
      const t = this.state?.tasks.find((x) => x.id === target.slice(5));
      this.highlight = target.slice(5);
      if (t) this.openForm(t);
    }
    this.render();
  }

  render() {
    if (!this.state) return;
    // Never rebuild while the form is open: it would wipe what the user is typing.
    if (this.draft && this.root.querySelector("form.task-form")) return;
    const html = this.html(this.state);
    if (html === this.lastHtml) return;
    this.lastHtml = html;
    this.root.innerHTML = html;
    this.bind();
    if (this.highlight) this.root.querySelector(`[data-id="${CSS.escape(this.highlight)}"]`)?.scrollIntoView({ block: "center" });
  }

  private openForm(t: TaskView | null) {
    this.newProject = false;
    this.message = null;
    this.draft = t
      ? {
          id: t.id,
          project_id: t.project_id,
          title: t.title,
          kind: t.kind,
          high_priority: t.priority === "high",
          due_date: t.due_date ?? "",
          due_time: t.due_time ?? "",
          estimated_hours: t.estimated_hours ? String(t.estimated_hours) : "",
          slot_date: t.slot_date ?? "",
          slot_time: t.slot_time ?? "",
        }
      : {
          id: null,
          project_id: this.state?.projects.some((p) => p.id === lastProject()) ? lastProject() : (this.state?.projects[0]?.id ?? ""),
          title: "",
          kind: "todo",
          high_priority: false,
          due_date: today(),
          due_time: "",
          estimated_hours: "",
          slot_date: "",
          slot_time: "",
        };
    this.lastHtml = "";
  }

  private closeForm() {
    this.draft = null;
    this.newProject = false;
    this.lastHtml = "";
    this.render();
  }

  // -------------------------------------------------------------------------
  // HTML
  // -------------------------------------------------------------------------

  private html(s: TasksState): string {
    const float = this.mode === "float";
    if (!s.paired) {
      return `<div class="card empty">Connect TrusCo Tracker to TrusCo to see your tasks.</div>`;
    }
    const top = float
      ? `<div class="row spread task-top">
           <strong>Tasks</strong>
           <span class="row">
             <button class="icon" data-act="new" title="New task" aria-label="New task">＋</button>
             <button class="icon" data-act="open-main" title="Open in TrusCo Tracker" aria-label="Open in TrusCo Tracker">↗</button>
           </span>
         </div>`
      : `<div class="row task-top">
           <button data-act="detach" aria-label="Detach" title="Show your tasks in a small window that stays on top">⧉ Detach</button>
           <span class="grow"></span>
           <button class="primary" data-act="new">＋ New task</button>
         </div>`;
    const form = this.draft ? this.formHtml(s) : "";
    const groups = GROUPS.map((g) => {
      const items = s.tasks.filter((t) => g.match(t.urgency));
      if (!items.length) return "";
      return `<section class="${float ? "" : "card"} task-group">
          <h2 class="group-${g.title === "Overdue" ? "overdue" : "normal"}">${g.title} <span class="muted">${items.length}</span></h2>
          <div class="list">${items.map((t) => this.rowHtml(t)).join("")}</div>
        </section>`;
    }).join("");
    const empty = s.tasks.length
      ? ""
      : `<div class="${float ? "" : "card "}empty">No tasks yet. ${float ? "Click ＋ to add one." : "Click <b>New task</b> to add one to a TrusCo project."}</div>`;
    const status = this.message || s.error;
    return `
      ${top}
      ${status ? `<div class="banner">${esc(status)}</div>` : ""}
      ${form}
      ${groups}${empty}
      ${float ? "" : `<footer>Tasks are saved to the TrusCo project · <button class="link small" data-act="refresh">Refresh</button></footer>`}`;
  }

  private rowHtml(t: TaskView): string {
    const float = this.mode === "float";
    const due = `<span class="due u-${t.urgency}">${esc(t.due_label)}</span>`;
    // The narrow detached window puts the due date on the second line so titles stay readable.
    const meta = float ? [due, esc(t.project)] : [esc(t.project)];
    if (t.kind === "time") meta.push(`⏱ ${esc(t.budget ?? "time")}${t.slot && !float ? ` · ${esc(t.slot)}` : ""}`);
    return `
      <div class="task ${t.done ? "done" : ""} ${this.highlight === t.id ? "hl" : ""}" data-id="${esc(t.id)}">
        <input type="checkbox" class="check" data-done="${esc(t.id)}" ${t.done ? "checked" : ""} aria-label="Mark “${esc(t.title)}” done" />
        <span class="dot u-${t.urgency}" aria-hidden="true"></span>
        <button class="task-title" data-edit="${esc(t.id)}" title="${esc(t.title)}">${t.priority === "high" ? `<span class="flag" title="High priority">!</span> ` : ""}${esc(t.title)}</button>
        ${float ? "" : due}
        <div class="task-meta">${meta.join(" · ")}</div>
      </div>`;
  }

  private formHtml(s: TasksState): string {
    const d = this.draft!;
    const projectOptions = s.projects
      .map((p) => `<option value="${esc(p.id)}" ${p.id === d.project_id ? "selected" : ""}>${esc(p.number)} · ${esc(p.name)}</option>`)
      .join("");
    const time = d.kind === "time";
    return `
      <form class="card task-form" novalidate>
        <h2>${d.id ? "Edit task" : "New task"}</h2>
        <div class="field">
          <label for="tf-project">Project</label>
          ${
            this.newProject
              ? `<div class="row"><input id="tf-newproject" placeholder="New project name" maxlength="120" />
                   <button type="button" data-act="create-project">Create</button>
                   <button type="button" class="link small" data-act="cancel-project">Cancel</button></div>`
              : `<div class="row"><select id="tf-project" class="grow">${projectOptions || `<option value="">No projects yet</option>`}</select>
                   <button type="button" class="link small" data-act="new-project">＋ New project</button></div>`
          }
        </div>
        <div class="field">
          <label for="tf-title">Task</label>
          <input id="tf-title" maxlength="300" placeholder="What needs doing?" value="${esc(d.title)}" />
        </div>
        <div class="field" role="radiogroup" aria-label="Type">
          <label>Type</label>
          <div class="segmented">
            <label><input type="radio" name="tf-kind" value="todo" ${time ? "" : "checked"} /> To Do</label>
            <label><input type="radio" name="tf-kind" value="time" ${time ? "checked" : ""} /> Allocate time</label>
          </div>
        </div>
        <div class="field">
          <label for="tf-due-date">Due</label>
          <div class="row">
            <input id="tf-due-date" type="date" value="${esc(d.due_date)}" />
            <input id="tf-due-time" type="time" value="${esc(d.due_time)}" aria-label="Due time (optional)" />
          </div>
          <p class="hint">Leave the time empty for "any time that day".</p>
        </div>
        ${
          time
            ? `<div class="field">
                 <label for="tf-hours">Time budget (hours)</label>
                 <input id="tf-hours" type="number" min="0.25" max="1000" step="0.25" placeholder="e.g. 2.5" value="${esc(d.estimated_hours)}" />
               </div>
               <div class="field">
                 <label for="tf-slot-date">Time slot (optional)</label>
                 <div class="row">
                   <input id="tf-slot-date" type="date" value="${esc(d.slot_date)}" />
                   <input id="tf-slot-time" type="time" value="${esc(d.slot_time)}" aria-label="Slot start time" />
                 </div>
               </div>`
            : ""
        }
        <div class="field row">
          <input id="tf-high" type="checkbox" style="width:auto" ${d.high_priority ? "checked" : ""} />
          <label for="tf-high" style="margin:0;font-weight:400">High priority</label>
        </div>
        <div class="row" style="margin-top:12px">
          <button type="submit" class="primary" ${this.saving ? "disabled" : ""}>${this.saving ? "Saving…" : "Save"}</button>
          <button type="button" data-act="cancel">Cancel</button>
        </div>
      </form>`;
  }

  // -------------------------------------------------------------------------
  // Events
  // -------------------------------------------------------------------------

  private bind() {
    const r = this.root;
    const act = (name: string, fn: () => void) =>
      r.querySelectorAll<HTMLElement>(`[data-act="${name}"]`).forEach((b) => b.addEventListener("click", fn));

    act("detach", () => invoke("detach_tasks"));
    act("open-main", () => invoke("open_tasks_in_main", { target: "tasks" }));
    act("refresh", () => this.load(true));
    act("new", () => {
      this.openForm(null);
      this.render();
      r.querySelector<HTMLInputElement>("#tf-title")?.focus();
    });
    act("cancel", () => this.closeForm());
    act("new-project", () => {
      this.capture();
      this.newProject = true;
      this.redrawForm();
      r.querySelector<HTMLInputElement>("#tf-newproject")?.focus();
    });
    act("cancel-project", () => {
      this.capture();
      this.newProject = false;
      this.redrawForm();
    });
    act("create-project", () => this.createProject());

    r.querySelectorAll<HTMLInputElement>("[data-done]").forEach((c) =>
      c.addEventListener("change", () => this.setDone(c.dataset.done!, c.checked)),
    );
    r.querySelectorAll<HTMLButtonElement>("[data-edit]").forEach((b) =>
      b.addEventListener("click", () => {
        const t = this.state?.tasks.find((x) => x.id === b.dataset.edit);
        if (!t) return;
        this.highlight = t.id;
        this.openForm(t);
        this.render();
        r.querySelector<HTMLInputElement>("#tf-title")?.focus();
      }),
    );
    r.querySelectorAll<HTMLInputElement>('input[name="tf-kind"]').forEach((i) =>
      i.addEventListener("change", () => {
        this.capture();
        this.redrawForm();
      }),
    );
    const form = r.querySelector<HTMLFormElement>("form.task-form");
    form?.addEventListener("submit", (ev) => {
      ev.preventDefault();
      this.save();
    });
    form?.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") this.closeForm();
    });
  }

  /** Copies the form's current values into the draft. */
  private capture() {
    const d = this.draft;
    if (!d) return;
    const v = (id: string) => this.root.querySelector<HTMLInputElement | HTMLSelectElement>(`#${id}`)?.value;
    d.project_id = v("tf-project") ?? d.project_id;
    d.title = v("tf-title") ?? d.title;
    d.kind = (this.root.querySelector<HTMLInputElement>('input[name="tf-kind"]:checked')?.value as Draft["kind"]) ?? d.kind;
    d.due_date = v("tf-due-date") ?? d.due_date;
    d.due_time = v("tf-due-time") ?? d.due_time;
    d.estimated_hours = v("tf-hours") ?? d.estimated_hours;
    d.slot_date = v("tf-slot-date") ?? d.slot_date;
    d.slot_time = v("tf-slot-time") ?? d.slot_time;
    d.high_priority = this.root.querySelector<HTMLInputElement>("#tf-high")?.checked ?? d.high_priority;
  }

  private redrawForm() {
    const form = this.root.querySelector("form.task-form");
    if (!form || !this.state) return;
    form.outerHTML = this.formHtml(this.state);
    this.bind();
  }

  private async createProject() {
    const input = this.root.querySelector<HTMLInputElement>("#tf-newproject");
    const name = input?.value.trim() ?? "";
    if (name.length < 2) {
      this.flash("Give the project a name of at least 2 characters.");
      return;
    }
    try {
      const p = await invoke<Project>("create_project", { name });
      this.state?.projects.unshift(p);
      if (this.draft) this.draft.project_id = p.id;
      this.newProject = false;
      this.redrawForm();
    } catch (e) {
      this.flash(String(e));
    }
  }

  private async save() {
    this.capture();
    const d = this.draft;
    if (!d) return;
    if (!d.project_id) return this.flash("Choose a project, or create one.");
    if (!d.title.trim()) return this.flash("Describe the task.");
    if (d.due_time && !d.due_date) return this.flash("Choose a due date for the due time.");
    const hours = d.kind === "time" ? Number(d.estimated_hours || 0) : 0;
    if (d.kind === "time" && !(hours > 0)) return this.flash("Set a time budget, e.g. 1.5 hours.");
    this.saving = true;
    this.redrawForm();
    try {
      this.state = await invoke<TasksState>("save_task", {
        form: {
          id: d.id,
          project_id: d.project_id,
          title: d.title,
          kind: d.kind,
          high_priority: d.high_priority,
          due_date: d.due_date || null,
          due_time: d.due_time || null,
          estimated_hours: hours || null,
          slot_date: d.slot_date || null,
          slot_time: d.slot_time || null,
        },
      });
      rememberProject(d.project_id);
      this.saving = false;
      this.message = null;
      this.closeForm();
    } catch (e) {
      this.saving = false;
      this.flash(String(e));
    }
  }

  private async setDone(id: string, done: boolean) {
    try {
      this.state = await invoke<TasksState>("set_task_done", { id, done });
      this.message = null;
    } catch (e) {
      this.message = String(e);
    }
    this.lastHtml = "";
    this.render();
  }

  /** Shows a message without losing what's in the form. */
  private flash(text: string) {
    this.message = text;
    const form = this.root.querySelector("form.task-form");
    let banner = this.root.querySelector<HTMLElement>(".form-error");
    if (form && !banner) {
      banner = document.createElement("div");
      banner.className = "banner form-error";
      form.prepend(banner);
    }
    if (banner) banner.textContent = text;
    else this.render();
  }
}
