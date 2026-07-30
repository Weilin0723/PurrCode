const state = {
  config: null,
  repository: null,
  sessions: [],
  selectedRun: null,
  selectedSession: null,
  messages: [],
  events: [],
  screen: "home",
  liveText: "",
  liveSource: null,
  streamRun: null,
  streamRefresh: null
};

async function request(path, options = {}) {
  const response = await fetch(path, {
    ...options,
    headers: { "Content-Type": "application/json", ...(options.headers || {}) },
    credentials: "same-origin"
  });
  const text = await response.text();
  let value = null;
  if (text) {
    try { value = JSON.parse(text); } catch { value = text; }
  }
  if (!response.ok) {
    const detail = typeof value === "string" ? value : value?.error || `HTTP ${response.status}`;
    throw new Error(detail);
  }
  return value;
}

function escapeHtml(value) {
  const node = document.createElement("span");
  node.textContent = value ?? "";
  return node.innerHTML;
}

function toast(message) {
  const element = document.querySelector("#toast");
  element.textContent = message;
  element.classList.add("visible");
  window.clearTimeout(toast.timer);
  toast.timer = window.setTimeout(() => element.classList.remove("visible"), 3600);
}

function setScreen(screen) {
  state.screen = screen;
  document.querySelectorAll(".nav-item").forEach((item) => item.classList.toggle("active", item.dataset.screen === screen));
  const mapped = ["home", "workbench", "diff", "validation"];
  for (const name of mapped) {
    document.querySelector(`#${name}-screen`).classList.toggle("hidden", name !== screen);
  }
  const placeholder = !mapped.includes(screen);
  document.querySelector("#placeholder-screen").classList.toggle("hidden", !placeholder);
  if (placeholder) {
    const [title, copy] = screenCopy[screen];
    document.querySelector("#placeholder-title").textContent = title;
    document.querySelector("#placeholder-copy").textContent = copy;
  }
}

function renderSessions() {
  const container = document.querySelector("#runs");
  document.querySelector("#run-count").textContent = `${state.sessions.length} run${state.sessions.length === 1 ? "" : "s"}`;
  if (!state.sessions.length) {
    container.innerHTML = '<div class="empty">No durable runs yet. Submit one objective to begin.</div>';
    return;
  }
  container.innerHTML = state.sessions.slice().reverse().map((run) => `
    <article class="run">
      <div><h3>${escapeHtml(run.objective || "Untitled run")}</h3><p>${escapeHtml(run.repository || "Repository pending")} · ${Number(run.event_count || 0)} events</p></div>
      <div class="run-actions"><span class="status">${escapeHtml(run.status_code || run.status)}</span><button class="open-run" data-run-id="${escapeHtml(run.id)}">Open</button></div>
    </article>`).join("");
  document.querySelectorAll(".open-run").forEach((button) => button.addEventListener("click", () => openWorkbench(button.dataset.runId)));
}

async function refreshDashboard() {
  const refreshButton = document.querySelector("#refresh");
  refreshButton.disabled = true;
  try {
    state.config = await request("/studio/config");
    const [health, repository, sessions] = await Promise.all([
      request("/api/v1/health"),
      request("/api/v1/repository/inspect", { method: "POST", body: JSON.stringify({ repository: state.config.repository }) }),
      request("/api/v1/sessions")
    ]);
    state.repository = repository;
    state.sessions = sessions;
    document.querySelector("#health-dot").classList.add("ready");
    document.querySelector("#health-label").textContent = health.status === "ok" ? "Daemon connected" : "Daemon degraded";
    document.querySelector("#workspace-title").textContent = repository.root;
    document.querySelector("#branch").textContent = repository.head || "No commit";
    document.querySelector("#dirty").textContent = repository.dirty ? "Working tree changed" : "Working tree clean";
    document.querySelector("#repository-path").textContent = repository.root;
    document.querySelector("#version").textContent = `Daemon API ${state.config.daemon_api_version} · Studio API ${state.config.studio_api_version}`;
    renderSessions();
  } catch (error) {
    document.querySelector("#health-dot").classList.remove("ready");
    document.querySelector("#health-label").textContent = "Connection interrupted";
    toast(error.message);
  } finally {
    refreshButton.disabled = false;
  }
}

async function startRun() {
  const button = document.querySelector("#start-run");
  const objective = document.querySelector("#objective").value.trim();
  if (!objective) { toast("Enter an engineering objective first."); return; }
  button.disabled = true;
  try {
    const accepted = await request("/api/v1/sessions", {
      method: "POST",
      body: JSON.stringify({ objective, repository: state.config.repository, plan_only: false })
    });
    document.querySelector("#objective").value = "";
    toast(`Run ${accepted.id.slice(0, 8)} accepted. Work continues if this window closes.`);
    await refreshDashboard();
    await openWorkbench(accepted.id);
  } catch (error) {
    toast(error.message);
    await refreshDashboard();
  } finally {
    button.disabled = false;
  }
}

function eventTitle(event) {
  return String(event?.event || "unknown_event").split("_").map((word) => word.charAt(0).toUpperCase() + word.slice(1)).join(" ");
}

function bounded(value, maximum = 110) {
  const rendered = typeof value === "string" ? value : JSON.stringify(value ?? "");
  return rendered.length > maximum ? `${rendered.slice(0, maximum - 1)}…` : rendered;
}

function eventSummary(event) {
  const data = event?.data || {};
  const candidates = [data.reason, data.model, data.role, data.label, data.evidence, data.strategy, data.activity, data.action_id];
  const selected = candidates.find((value) => value !== undefined && value !== null && String(value).length);
  if (selected !== undefined) return bounded(selected);
  const keys = Object.keys(data);
  return keys.length ? keys.slice(0, 3).join(" · ") : "Durable lifecycle boundary";
}

function renderConversation() {
  const container = document.querySelector("#conversation");
  document.querySelector("#message-count").textContent = String(state.messages.length);
  if (!state.messages.length && !state.liveText) {
    container.innerHTML = '<div class="empty">No conversation messages recorded.</div>';
    return;
  }
  const durable = state.messages.map((message) => `
    <article class="message">
      <div class="message-role"><span>${escapeHtml(message.role)}</span><span>${escapeHtml(message.model || "")}</span></div>
      <p class="message-content">${escapeHtml(message.content)}</p>
    </article>`).join("");
  const live = state.liveText ? `
    <article class="message live">
      <div class="message-role"><span>assistant · live</span><span>streaming</span></div>
      <p class="message-content">${escapeHtml(state.liveText)}</p>
    </article>` : "";
  container.innerHTML = durable + live;
  container.scrollTop = container.scrollHeight;
}

function scheduleStreamRefresh() {
  window.clearTimeout(state.streamRefresh);
  state.streamRefresh = window.setTimeout(refreshWorkbench, 120);
}

function connectRunStream(runId) {
  if (state.streamRun === runId && state.liveSource) return;
  if (state.liveSource) state.liveSource.close();
  state.streamRun = runId;
  state.liveText = "";
  const source = new EventSource(`/api/v1/sessions/${runId}/events/stream?after=${state.events.length}`);
  state.liveSource = source;
  source.addEventListener("open", () => { document.querySelector("#stream-status").textContent = "live"; });
  source.addEventListener("content_delta", (message) => {
    try {
      const event = JSON.parse(message.data);
      state.liveText = (state.liveText + (event.delta || "")).slice(-262144);
      renderConversation();
    } catch { document.querySelector("#stream-status").textContent = "stream warning"; }
  });
  source.addEventListener("phase", (message) => {
    try {
      const event = JSON.parse(message.data);
      document.querySelector("#stream-status").textContent = event.phase || "live";
      if (["completed", "failed", "cancelled"].includes(event.phase)) scheduleStreamRefresh();
    } catch { document.querySelector("#stream-status").textContent = "stream warning"; }
  });
  source.addEventListener("durable_audit", scheduleStreamRefresh);
  source.addEventListener("diagnostic", () => { document.querySelector("#stream-status").textContent = "reconnecting"; });
  source.onerror = () => { document.querySelector("#stream-status").textContent = "reconnecting"; };
}

function renderActivity() {
  const container = document.querySelector("#activity");
  document.querySelector("#event-count").textContent = String(state.events.length);
  if (!state.events.length) {
    container.innerHTML = '<div class="empty">No durable events recorded.</div>';
    return;
  }
  container.innerHTML = state.events.map((event, index) => `
    <button class="event-button" data-event-index="${index}">
      <span class="event-title">${escapeHtml(eventTitle(event))}</span>
      <span class="event-summary">${escapeHtml(eventSummary(event))}</span>
    </button>`).join("");
  document.querySelectorAll(".event-button").forEach((button) => button.addEventListener("click", () => inspectEvent(Number(button.dataset.eventIndex), button)));
}

function inspectEvent(index, button) {
  const event = state.events[index];
  if (!event) return;
  document.querySelectorAll(".event-button").forEach((item) => item.classList.toggle("active", item === button));
  document.querySelector("#inspector-title").textContent = eventTitle(event);
  document.querySelector("#inspector-summary").textContent = eventSummary(event);
  document.querySelector("#inspector").textContent = JSON.stringify(event.data || {}, null, 2);
}

async function refreshWorkbench() {
  if (!state.selectedRun) {
    toast("Select a run from Home first.");
    return;
  }
  const button = document.querySelector("#refresh-workbench");
  button.disabled = true;
  try {
    const [session, messages, events] = await Promise.all([
      request(`/api/v1/sessions/${state.selectedRun}`),
      request(`/api/v1/sessions/${state.selectedRun}/messages`),
      request(`/api/v1/sessions/${state.selectedRun}/events`)
    ]);
    state.selectedSession = session;
    state.messages = messages;
    state.events = events;
    document.querySelector("#workbench-objective").textContent = session.objective || "Untitled run";
    document.querySelector("#workbench-status").textContent = session.status_code || session.status;
    renderConversation();
    renderActivity();
    connectRunStream(state.selectedRun);
  } catch (error) {
    toast(error.message);
  } finally {
    button.disabled = false;
  }
}

async function openWorkbench(runId) {
  state.selectedRun = runId;
  setScreen("workbench");
  await refreshWorkbench();
}

async function sendFollowUp() {
  if (!state.selectedRun) { toast("Select a run first."); return; }
  const field = document.querySelector("#follow-up");
  const content = field.value.trim();
  if (!content) { toast("Enter a follow-up message first."); return; }
  const button = document.querySelector("#send-follow-up");
  button.disabled = true;
  try {
    await request(`/api/v1/sessions/${state.selectedRun}/messages`, { method: "POST", body: JSON.stringify({ content }) });
    field.value = "";
    await refreshWorkbench();
  } catch (error) {
    toast(error.message);
  } finally {
    button.disabled = false;
  }
}

async function refreshDiff() {
  if (!state.selectedRun) { toast("Select a run from Home first."); return; }
  try {
    const diff = await request(`/api/v1/sessions/${state.selectedRun}/diff`);
    const files = diff.changed_files || [];
    document.querySelector("#changed-files").innerHTML = files.length
      ? files.map((path) => `<span class="changed-file">${escapeHtml(path)}</span>`).join("")
      : '<span class="empty">No changed files.</span>';
    document.querySelector("#diff-content").textContent = diff.patch || "No patch recorded.";
  } catch (error) {
    document.querySelector("#changed-files").innerHTML = "";
    document.querySelector("#diff-content").textContent = `Diff unavailable: ${error.message}`;
  }
}

function validationEvents() {
  return state.events.filter((event) => [
    "validation_recorded",
    "outcome_judgment_recorded",
    "outcome_review_required",
    "outcome_review_approved",
    "session_completed",
    "session_failed"
  ].includes(event.event));
}

async function refreshValidation() {
  if (!state.selectedRun) { toast("Select a run from Home first."); return; }
  if (!state.events.length) await refreshWorkbench();
  const events = validationEvents();
  const container = document.querySelector("#validation-events");
  if (!events.length) {
    container.innerHTML = '<div class="empty">Validation has not produced durable evidence yet.</div>';
    return;
  }
  container.innerHTML = events.map((event) => `
    <article class="validation-item"><div><h3>${escapeHtml(eventTitle(event))}</h3><p>${escapeHtml(eventSummary(event))}</p></div><span class="status">recorded</span></article>`).join("");
}

const screenCopy = {
  workspaces: ["Workspaces", "Open local and remote repositories without creating a second execution path."],
  runs: ["Agent runs", "Follow durable plans, specialist activity, approvals, and outcomes."],
  terminals: ["Terminals", "Real PTY terminals and ownership controls are delivered by the terminal runtime."],
  factory: ["Agent Factory", "Compile production goals into versioned, reusable agent blueprints."],
  deployments: ["Deployments", "Inspect local and Azure runtime deployments and rollback state."],
  evidence: ["Evidence", "Audit exact actions, authorization, execution, validation, and recovery."],
  settings: ["Settings", "Manage providers, identity selectors, environments, and authority grants."]
};

document.querySelector("#refresh").addEventListener("click", refreshDashboard);
document.querySelector("#start-run").addEventListener("click", startRun);
document.querySelector("#refresh-workbench").addEventListener("click", refreshWorkbench);
document.querySelector("#send-follow-up").addEventListener("click", sendFollowUp);
document.querySelector("#refresh-diff").addEventListener("click", refreshDiff);
document.querySelector("#refresh-validation").addEventListener("click", refreshValidation);
document.querySelectorAll(".nav-item").forEach((button) => button.addEventListener("click", async () => {
  const screen = button.dataset.screen;
  setScreen(screen);
  if (screen === "workbench" && state.selectedRun) await refreshWorkbench();
  if (screen === "diff") await refreshDiff();
  if (screen === "validation") await refreshValidation();
}));

refreshDashboard();
window.setInterval(async () => {
  await refreshDashboard();
  if (state.screen === "workbench" && state.selectedRun) await refreshWorkbench();
}, 5000);
