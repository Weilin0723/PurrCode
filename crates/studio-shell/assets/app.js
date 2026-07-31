import { Terminal, measure } from "/term.js";

const state = {
  config: null, repository: null, sessions: [], selectedRun: null,
  messages: [], events: [], liveText: "", liveSource: null,
  streamRun: null, streamRefresh: null,
  terminals: [], selectedTerminal: null, terminalSocket: null, terminalSocketId: null,
  drawerOpen: false, drawerTab: "changes", emulator: null, replayedTerminal: null,
  models: [], providers: [], activeModel: null, activity: [], plan: [],
  planRevision: 0, awaitingPlanReview: false,
  taskMode: "build", permission: "ask"
};

const $ = (sel) => document.querySelector(sel);

async function request(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { "Content-Type": "application/json", ...(options.headers || {}) }, credentials: "same-origin" });
  const text = await response.text();
  let value = null;
  if (text) { try { value = JSON.parse(text); } catch { value = text; } }
  if (!response.ok) { throw new Error(typeof value === "string" ? value : value?.error || `HTTP ${response.status}`); }
  return value;
}

function escapeHtml(value) {
  const node = document.createElement("span"); node.textContent = value ?? ""; return node.innerHTML;
}

function toast(message) {
  const el = $("#toast"); el.textContent = message; el.classList.add("visible");
  clearTimeout(toast.timer); toast.timer = setTimeout(() => el.classList.remove("visible"), 3600);
}

// ── Session list ──
function renderSessionList() {
  const c = $("#session-list");
  if (!state.sessions.length) { c.innerHTML = '<div class="empty">No sessions yet.</div>'; return; }
  c.innerHTML = state.sessions.slice().reverse().map((s) => `
    <button class="session-item ${s.id === state.selectedRun ? "active" : ""}" data-session-id="${escapeHtml(s.id)}">
      <div class="session-title">${escapeHtml(s.objective || "Untitled")}</div>
      <div class="session-meta">${escapeHtml(s.repository || "—")} · ${escapeHtml(s.status_code || s.status || "—")}</div>
    </button>`).join("");
  document.querySelectorAll(".session-item").forEach((btn) => btn.addEventListener("click", () => openSession(btn.dataset.sessionId)));
}

// ── Conversation ──
function renderConversation() {
  const c = $("#conversation");
  $("#session-status").textContent = state.selectedRun ? (state.selectedSession?.status_code || state.selectedSession?.status || "—") : "—";
  if (!state.messages.length && !state.liveText) {
    c.innerHTML = '<div class="empty">No conversation yet. Submit an objective to start.</div>'; return;
  }
  const durable = state.messages.map((m) => `
    <article class="message">
      <div class="message-role"><span>${escapeHtml(m.role)}</span><span class="model">${escapeHtml(m.model || "")}</span></div>
      <p class="message-content">${escapeHtml(m.content)}</p>
    </article>`).join("");
  const live = state.liveText ? `
    <article class="message live"><div class="message-role"><span>assistant · streaming</span></div>
    <p class="message-content">${escapeHtml(state.liveText)}</p></article>` : "";
  c.innerHTML = durable + renderPlan() + live; c.scrollTop = c.scrollHeight;
}

/// The plan, in full.
///
/// In Plan mode the plan is the deliverable (PRD §11), and the run pauses
/// saying it is "ready for review". It was reaching the client only as the
/// first step, truncated into an activity summary — a session that announces
/// something to review and then shows nothing to review.
function renderPlan() {
  if (!state.plan.length) return "";
  const steps = state.plan
    .map((step) => `<li>${escapeHtml(planStepText(step))}</li>`)
    .join("");
  // Reviewing a plan has to lead somewhere, and to more than one place. The
  // button accepts the plan as written; the composer changes it. Offering only
  // the button made review a yes/no vote on a plan the reviewer could not edit,
  // and the only way to change one step was to start over and re-describe the
  // whole task.
  const action = state.awaitingPlanReview
    ? `<div class="plan-actions">
         <button id="build-plan" class="primary">Build this plan</button>
         <span class="plan-hint">Or say what to change below — the plan is rewritten and paused again. Nothing has been changed yet.</span>
       </div>`
    : "";
  const revision = state.planRevision > 1 ? ` · revision ${state.planRevision}` : "";
  return `<article class="message plan">
    <div class="message-role"><span>Plan</span><span class="model">${state.plan.length} steps${revision}</span></div>
    <ol class="plan-steps">${steps}</ol>
    ${action}
  </article>`;
}

/// Point the composer at what sending will actually do.
///
/// The same box starts a session, continues one, and revises a plan. A person
/// about to press Send is entitled to know which.
function renderComposerIntent() {
  const composer = $("#composer");
  if (!state.selectedRun) {
    composer.placeholder = "Ask PurrCode to inspect, change, test, or explain…";
    $("#send").textContent = "Send";
    return;
  }
  const revising = state.awaitingPlanReview;
  composer.placeholder = revising
    ? "Say what to change about the plan — add, drop or reorder steps…"
    : "Reply, or add to what PurrCode is doing…";
  $("#send").textContent = revising ? "Revise plan" : "Send";
}

async function buildPlan() {
  if (!state.selectedRun) return;
  const button = $("#build-plan");
  if (button) { button.disabled = true; button.textContent = "Starting…"; }
  try {
    await request(`/api/v1/sessions/${state.selectedRun}/resume`, { method: "POST" });
    toast("Building from the plan.");
    await refreshSession();
  } catch (error) {
    toast(`Could not start: ${error.message}`);
    if (button) { button.disabled = false; button.textContent = "Build this plan"; }
  }
}

/// A plan step without its own leading number.
///
/// Models usually number their steps, and the list numbers them again, so a
/// step arrives reading "1. 1. Define core data models". Only a leading
/// enumerator is removed — a step that genuinely starts with a figure, like
/// "2024 exports must keep working", is left alone because the separator is
/// required.
function planStepText(step) {
  return String(step).replace(/^\s*\d{1,3}\s*[.)]\s+/, "").trim();
}

// ── Activity (compact) ──
//
// PRD §15.1 and §31.4: the daemon decides what a person reads. Title-casing raw
// event names here produced "Submodules Prepared" and "Model Request Started" —
// internal vocabulary leaking into the main surface, and a second, divergent
// reading of the same run from the one the Workbench shows.
const ACTIVITY_ICON = {
  done: '<span class="activity-check">✓</span>',
  running: '<span class="activity-running">●</span>',
  blocked: '<span class="activity-attention">!</span>',
  failed: '<span class="activity-failed">✗</span>',
  pending: '<span class="activity-pending">○</span>'
};

async function refreshActivity() {
  if (!state.selectedRun) {
    state.activity = []; state.plan = [];
    state.planRevision = 0; state.awaitingPlanReview = false;
    renderComposerIntent(); renderActivity(); return;
  }
  try {
    const [activity, summary] = await Promise.all([
      request(`/api/v1/sessions/${state.selectedRun}/activity`),
      request(`/api/v1/sessions/${state.selectedRun}/summary`)
    ]);
    state.activity = activity || [];
    state.plan = summary?.plan || [];
    state.planRevision = summary?.plan_revision || 0;
    // Whether a plan is open for revision is the daemon's answer, not a guess
    // from status and step count. Two clients guessing separately is how the
    // same session ends up described two ways.
    state.awaitingPlanReview = Boolean(summary?.awaiting_plan_review);
    renderComposerIntent();
    renderConversation();
  } catch {
    // Leave the last known activity in place: blanking it would claim the
    // session had done nothing, which is a different statement from "we could
    // not read it just now".
  }
  renderActivity();
}

function renderActivity() {
  const c = $("#activity-compact");
  if (!state.activity.length) { c.innerHTML = ""; return; }
  c.innerHTML = state.activity.slice(-8).map((item) => {
    const icon = ACTIVITY_ICON[item.status] || ACTIVITY_ICON.pending;
    const summary = item.summary ? `<span class="activity-summary">${escapeHtml(item.summary)}</span>` : "";
    // The status word rides along with the glyph so it never depends on colour
    // or on a symbol alone.
    return `<div class="activity-item">${icon} ${escapeHtml(item.label)} <span class="activity-state">${escapeHtml(item.status)}</span>${summary}</div>`;
  }).join("");
}

// ── Streaming ──
function connectRunStream(runId) {
  if (state.streamRun === runId && state.liveSource) return;
  if (state.liveSource) state.liveSource.close();
  state.streamRun = runId; state.liveText = "";
  const source = new EventSource(`/api/v1/sessions/${runId}/events/stream?after=${state.events.length}`);
  state.liveSource = source;
  source.addEventListener("content_delta", (msg) => {
    try { const e = JSON.parse(msg.data); state.liveText = (state.liveText + (e.delta || "")).slice(-262144); renderConversation(); }
    catch { /* stream warning */ }
  });
  source.addEventListener("phase", (msg) => {
    try { const e = JSON.parse(msg.data); if (["completed", "failed", "cancelled"].includes(e.phase)) scheduleStreamRefresh(); }
    catch { /* */ }
  });
  source.addEventListener("durable_audit", scheduleStreamRefresh);
  source.onerror = () => { /* reconnect; durable output remains */ };
}

function scheduleStreamRefresh() { clearTimeout(state.streamRefresh); state.streamRefresh = setTimeout(refreshSession, 120); }

// ── Data refresh ──
async function refreshAll() {
  try {
    state.config = await request("/studio/config");
    const [health, repository, sessions] = await Promise.all([
      request("/api/v1/health"),
      request("/api/v1/repository/inspect", { method: "POST", body: JSON.stringify({ repository: state.config.repository }) }),
      request("/api/v1/sessions")
    ]);
    state.repository = repository; state.sessions = sessions;
    $("#health-dot").classList.add("ready");
    $("#health-label").textContent = health.status === "ok" ? "Connected" : "Degraded";
    const repoName = repository.name || repository.root || "—";
    const branch = repository.branch || (repository.head || "").slice(0, 12) || "—";
    $("#repo-info").textContent = `${repoName}/${branch}`;
    $("#repository-path")?.replaceChildren();
    $("#version").textContent = `API ${state.config.daemon_api_version}`;
    $("#footer-info").textContent = `${repoName}/${branch}`;
    renderSessionList();
  } catch (error) {
    $("#health-dot").classList.remove("ready");
    $("#health-label").textContent = "Disconnected"; toast(error.message);
  }
}

async function refreshSession() {
  if (!state.selectedRun) return;
  try {
    const [session, messages, events] = await Promise.all([
      request(`/api/v1/sessions/${state.selectedRun}`),
      request(`/api/v1/sessions/${state.selectedRun}/messages`),
      request(`/api/v1/sessions/${state.selectedRun}/events`)
    ]);
    state.selectedSession = session; state.messages = messages; state.events = events;
    $("#session-objective").textContent = session.objective || "Untitled session";
    // The header follows the session, so opening one shows that session's model.
    renderModel();
    renderConversation(); await refreshActivity(); connectRunStream(state.selectedRun);
  } catch (error) { toast(error.message); }
}

async function openSession(runId) {
  state.selectedRun = runId; renderSessionList(); await refreshSession();
}

// ── Task and permission modes (PRD §11, §12) ──
//
// These are selectors, not labels. Studio previously displayed "Build" and
// "Ask" as static text, which read as a setting the user could not reach.
const PERMISSION_LABELS = { ask: "Ask", auto: "Auto", full_access: "Full Access" };
const AUTHORITY_MODES = { ask: "governed", auto: "elevated", full_access: "unrestricted" };
// Ask and Plan must not change files. The session payload carries that, so the
// daemon enforces it rather than inferring intent from the objective's wording.
const READ_ONLY_MODES = ["ask", "plan"];

function readModes() {
  state.taskMode = $("#composer-mode").value;
  state.permission = $("#composer-permission").value;
  $("#mode-info").textContent = `${$("#composer-mode").selectedOptions[0].text} · ${PERMISSION_LABELS[state.permission]}`;
}

function sessionPayload(objective) {
  return JSON.stringify({
    objective,
    repository: state.config.repository,
    plan_only: READ_ONLY_MODES.includes(state.taskMode),
    authority_mode: AUTHORITY_MODES[state.permission]
  });
}

async function startSession() {
  const objective = $("#composer").value.trim();
  if (!objective) { toast("Enter an objective first."); return; }
  try {
    const accepted = await request("/api/v1/sessions", { method: "POST", body: sessionPayload(objective) });
    $("#composer").value = ""; toast(`Session ${accepted.id.slice(0, 8)} started.`);
    await refreshAll(); await openSession(accepted.id);
  } catch (error) { toast(error.message); }
}

/// Send what is in the composer to the selected session.
///
/// While a plan is under review this is how it gets changed: the daemon reads
/// the message as feedback, rewrites the plan and pauses again. It used to be
/// refused outright with "use Build this plan to continue it", which left the
/// reviewer no way to disagree with a plan except to abandon the session.
async function sendFollowUp() {
  if (!state.selectedRun) { toast("Select or start a session first."); return; }
  const content = $("#composer").value.trim();
  if (!content) return;
  const revising = state.awaitingPlanReview;
  try {
    await request(`/api/v1/sessions/${state.selectedRun}/messages`, { method: "POST", body: JSON.stringify({ content }) });
    $("#composer").value = "";
    if (revising) toast("Revising the plan — it will pause for review again.");
    await refreshSession();
  } catch (error) { toast(error.message); }
}

// ── Models, providers, settings ──
//
// PRD §10.1: the active model is visible at all times, and §36 fails the
// release if the primary UI cannot select one. The header, the composer meta
// and the settings modal all read `state.activeModel`, so they cannot disagree.
async function refreshModels() {
  try {
    const [models, providers] = await Promise.all([
      request("/api/v1/models"),
      request("/api/v1/providers")
    ]);
    state.models = models || [];
    state.providers = providers || [];
    state.activeModel = state.models.find((m) => m.default) || state.models[0] || null;
    renderModel();
  } catch {
    // A model list we could not read is reported as unknown, never as absent:
    // "no models" and "could not ask" are different problems.
    $("#model-info").textContent = "model unavailable";
    $("#composer-model").textContent = "unavailable";
  }
}

function renderModel() {
  // A selected session has its own model, which can differ from the repository
  // default. Showing the default while a session runs on something else is a
  // header that reports the wrong fact.
  const sessionModel = state.selectedSession?.selected_model;
  const entry = sessionModel
    ? state.models.find((m) => m.id === sessionModel)
    : state.activeModel;
  const label = entry ? entry.model : sessionModel ? modelName(sessionModel) : "no model";
  const provider = entry ? entry.provider : "";
  $("#model-info").textContent = label;
  $("#composer-model").textContent = label;
  $("#active-model").textContent = state.activeModel ? state.activeModel.model : "no model";
  $("#active-model-provider").textContent = state.activeModel?.provider || "no provider configured";
}

/// The model part of a provider-qualified id, matching the Workbench header.
function modelName(id) {
  const cut = id.indexOf("/");
  return cut < 0 ? id : id.slice(cut + 1);
}

function openSettings() {
  $("#settings-modal").classList.remove("hidden");
  renderModel();
  renderModelChoices();
  $("#provider-list").innerHTML = state.providers.length
    ? state.providers.map((p) => `<div class="provider-row">${escapeHtml(p.name)}</div>`).join("")
    : '<div class="empty">No providers configured.</div>';
  // The modal reflects live state rather than its own copy, so it can never
  // show a setting the composer has already moved past.
  $("#settings-permission").value = state.permission;
  $("#settings-default-mode").value = state.taskMode;
  $("#permission-note").textContent = PERMISSION_NOTES[state.permission];
  $("#settings-repository").textContent = state.repository?.name || "—";
  $("#settings-branch").textContent = state.repository?.branch || "—";
  $("#settings-terminal-count").textContent = String(state.terminals.length);
  $("#settings-daemon-api").textContent = state.config?.daemon_api_version ?? "—";
  $("#settings-studio-api").textContent = state.config?.studio_api_version ?? "—";
}

const PERMISSION_NOTES = {
  ask: "PurrCode asks before writes, commands, dependency installs and network access.",
  auto: "Repository reads, writes and recognised build/test commands run automatically. Destructive or unexpected effects still ask.",
  full_access: "PurrCode may use every permission this process, workspace and configured identity already hold. It grants no new ones — not root, not cloud, not filesystem access the process lacks."
};

function closeSettings() { $("#settings-modal").classList.add("hidden"); }

/// Theme choice is remembered locally; it is presentation only and never
/// reaches the daemon.
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme === "system" ? "" : theme;
  try { localStorage.setItem("purrcode-theme", theme); } catch { /* private mode */ }
}

function renderModelChoices() {
  const list = $("#model-choices");
  if (!state.models.length) {
    list.innerHTML = '<div class="empty">No models are configured. Add a provider first.</div>';
    return;
  }
  list.innerHTML = state.models.map((m) => `
    <button class="model-choice${m.id === state.activeModel?.id ? " active" : ""}" data-model="${escapeHtml(m.id)}">
      <span>${escapeHtml(m.model)}</span>
      <span class="model-meta">${escapeHtml(m.provider)} · ${m.local ? "local" : "remote"}</span>
    </button>`).join("");
  list.querySelectorAll(".model-choice").forEach((button) =>
    button.addEventListener("click", () => selectModel(button.dataset.model)));
}

async function selectModel(id) {
  // Two scopes, reported separately. Announcing one success for both is how a
  // user ends up believing the open session switched when it did not — the
  // symptom being a header that shows the new model while the run keeps using
  // the old one.
  let repositoryOk = false;
  try {
    await request("/api/v1/models/roles", { method: "POST", body: JSON.stringify({ role: "coding_worker", model: id }) });
    repositoryOk = true;
  } catch (error) {
    toast(`Repository default unchanged: ${error.message}`);
  }
  let sessionResult = null;
  if (state.selectedRun) {
    try {
      await request(`/api/v1/sessions/${state.selectedRun}/model`, { method: "POST", body: JSON.stringify({ model: id }) });
      sessionResult = "ok";
    } catch (error) {
      sessionResult = error.message;
    }
  }
  await refreshModels();
  await refreshSession();
  renderModelChoices();
  if (sessionResult === "ok") toast(`${id} for this session and new work`);
  else if (sessionResult) toast(`This session kept its model: ${sessionResult}`);
  else if (repositoryOk) toast(`${id} for new work in this repository`);
}

// ── Context drawer ──
function openDrawer(tab = "changes") {
  state.drawerOpen = true; state.drawerTab = tab;
  $("#context-drawer").classList.remove("hidden");
  document.querySelectorAll(".drawer-tab").forEach((t) => t.classList.toggle("active", t.dataset.tab === tab));
  document.querySelectorAll(".drawer-panel").forEach((p) => p.classList.toggle("hidden", p.id !== `drawer-${tab}`));
  if (tab === "changes") refreshDiff();
  if (tab === "terminal") refreshTerminals();
  if (tab === "tests") refreshTests();
  if (tab === "evidence") refreshEvidence();
}

function closeDrawer() { state.drawerOpen = false; $("#context-drawer").classList.add("hidden"); }

async function refreshDiff() {
  if (!state.selectedRun) return;
  try {
    const diff = await request(`/api/v1/sessions/${state.selectedRun}/diff`);
    const files = diff.changed_files || [];
    $("#changed-files").innerHTML = files.length ? files.map((p) => `<span class="changed-file">${escapeHtml(p)}</span>`).join("") : '<span class="empty">No changed files.</span>';
    $("#diff-content").textContent = diff.patch || "No patch recorded.";
  } catch (error) { $("#diff-content").textContent = `Diff unavailable: ${error.message}`; }
}

async function refreshTests() {
  const c = $("#test-summary");
  if (!state.selectedRun) { c.innerHTML = '<div class="empty">Test results will appear here when validation runs.</div>'; return; }
  try {
    const summary = await request(`/api/v1/sessions/${state.selectedRun}/validation`);
    const stages = summary?.stages || [];
    if (!stages.length) { c.innerHTML = '<div class="empty">No validation has run.</div>'; return; }
    // Unavailable, skipped, cancelled and timed-out are never drawn as a pass
    // (PRD §21.3): the glyph and the word both come from the outcome.
    c.innerHTML = stages.map((s) => {
      const passed = s.outcome === "passed";
      const icon = passed ? ACTIVITY_ICON.done : ACTIVITY_ICON.failed;
      return `<div class="activity-item">${icon} ${escapeHtml(s.stage)} <span class="activity-state">${escapeHtml(s.outcome.replace(/_/g, " "))}</span></div>`;
    }).join("");
  } catch (error) { c.innerHTML = `<div class="empty">Validation unavailable: ${escapeHtml(error.message)}</div>`; }
}

function refreshEvidence() {
  const c = $("#evidence-list");
  const items = state.activity.filter((item) => item.detail_available);
  if (!items.length) { c.innerHTML = '<div class="empty">Evidence records will appear here when actions complete.</div>'; return; }
  c.innerHTML = items.slice(-10).map((item) =>
    `<div class="activity-item">${escapeHtml(item.label)}${item.summary ? `<span class="activity-summary">${escapeHtml(item.summary)}</span>` : ""}</div>`).join("");
}

// ── Terminals ──
//
// Output is a real emulated screen, not a stripped log: the socket delivers
// only bytes produced since the last frame and the emulator applies them.
function terminalOwnerLabel(owner) { if (!owner) return "—"; return owner.kind === "agent" ? `agent · ${owner.data?.role || "worker"}` : owner.kind; }

function ensureEmulator() {
  if (state.emulator) return state.emulator;
  const host = $("#terminal-output");
  const size = measure(host);
  state.emulator = new Terminal(host, {
    rows: size.rows,
    cols: size.cols,
    onInput: (bytes) => sendTerminalBytes(bytes),
    onResize: (rows, cols) => resizeTerminal(rows, cols)
  });
  window.addEventListener("resize", () => {
    if (!state.emulator || $("#terminal-output").hidden) return;
    const next = measure($("#terminal-output"));
    state.emulator.resize(next.rows, next.cols);
  });
  return state.emulator;
}

function renderSelectedTerminal() {
  const term = state.selectedTerminal;
  $("#terminal-title").textContent = term ? `Terminal ${term.terminal_id.slice(0, 8)}` : "No terminal";
  $("#terminal-owner").textContent = term ? `${term.alive ? "running" : "exited"} · ${terminalOwnerLabel(term.owner)}` : "—";
  const output = $("#terminal-output");
  output.hidden = !term;
  $("#terminal-empty").hidden = Boolean(term);
  $("#terminal-owner-actions").hidden = !term?.alive;
  if (!term) return;
  const emulator = ensureEmulator();
  // Replay is applied once per terminal; live bytes arrive over the socket.
  if (state.replayedTerminal !== term.terminal_id) {
    emulator.reset();
    emulator.write(decodeBytes(term.transcript_tail));
    state.replayedTerminal = term.terminal_id;
  }
}

function decodeBytes(bytes) {
  try { return new TextDecoder().decode(new Uint8Array(bytes || [])); }
  catch { return ""; }
}

function connectTerminalSocket(id) {
  if (state.terminalSocketId === id && state.terminalSocket?.readyState < 2) return;
  if (state.terminalSocket) state.terminalSocket.close();
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const socket = new WebSocket(`${proto}//${location.host}/studio/terminals/${id}/stream`);
  state.terminalSocket = socket; state.terminalSocketId = id;
  socket.onmessage = (msg) => {
    let frame;
    try { frame = JSON.parse(msg.data); } catch { return; }
    const emulator = ensureEmulator();
    // `truncated` means the ring buffer dropped output we never saw. Say so
    // rather than splicing unrelated bytes into the screen.
    if (frame.chunk?.truncated) emulator.write("\r\n[output truncated — earlier bytes were discarded]\r\n");
    if (frame.chunk?.bytes?.length) emulator.write(decodeBytes(frame.chunk.bytes));
    if (state.selectedTerminal) {
      state.selectedTerminal.alive = frame.alive ?? state.selectedTerminal.alive;
      state.selectedTerminal.generation = frame.generation ?? state.selectedTerminal.generation;
      if (frame.owner) state.selectedTerminal.owner = frame.owner;
      $("#terminal-owner").textContent = `${state.selectedTerminal.alive ? "running" : "exited"} · ${terminalOwnerLabel(state.selectedTerminal.owner)}`;
    }
  };
  // Reconnect rather than silently going dead: the process keeps running.
  socket.onclose = () => { if (state.terminalSocketId === id) setTimeout(() => connectTerminalSocket(id), 1000); };
  socket.onerror = () => toast("Terminal reconnecting; output remains available.");
}

function sendTerminalBytes(bytes) {
  const term = state.selectedTerminal;
  if (!term) return;
  const payload = JSON.stringify({ generation: term.generation, input: bytes });
  if (state.terminalSocket?.readyState === WebSocket.OPEN && state.terminalSocketId === term.terminal_id) state.terminalSocket.send(payload);
  else request(`/api/v1/terminals/${term.terminal_id}/input`, { method: "POST", body: payload }).catch((e) => toast(e.message));
}

async function resizeTerminal(rows, cols) {
  const term = state.selectedTerminal;
  if (!term) return;
  try { await request(`/api/v1/terminals/${term.terminal_id}/resize`, { method: "POST", body: JSON.stringify({ size: { rows, cols } }) }); }
  catch { /* a refused resize must not break the live stream */ }
}

async function refreshTerminals() {
  try {
    const result = await request("/api/v1/terminals"); state.terminals = result.terminals || [];
    if (state.selectedTerminal) {
      const current = state.terminals.find((t) => t.terminal_id === state.selectedTerminal.terminal_id);
      if (current) { const detail = await request(`/api/v1/terminals/${current.terminal_id}`); state.selectedTerminal = detail.terminal; }
    }
    renderSelectedTerminal();
  } catch (error) { toast(error.message); }
}

async function startTerminal() {
  try {
    let wsId = sessionStorage.getItem("purrcode-wsid"); if (!wsId) { wsId = crypto.randomUUID(); sessionStorage.setItem("purrcode-wsid", wsId); }
    const result = await request("/api/v1/terminals", { method: "POST", body: JSON.stringify({ workspace_id: wsId, action: { working_directory: state.config.repository, environment: {}, arguments: [], initial_size: { rows: 30, cols: 100 }, owner: { kind: "human" } } }) });
    state.selectedTerminal = result.terminal;
    await request(`/api/v1/terminals/${result.terminal.terminal_id}/attach`, { method: "POST", body: JSON.stringify({ replay_bytes: 262144 }) });
    await refreshTerminals(); connectTerminalSocket(result.terminal.terminal_id); $("#terminal-output").focus();
  } catch (error) { toast(error.message); }
}

async function setTerminalOwner(owner) {
  if (!state.selectedTerminal) return;
  try { const r = await request(`/api/v1/terminals/${state.selectedTerminal.terminal_id}/owner`, { method: "POST", body: JSON.stringify({ owner }) }); state.selectedTerminal = r.terminal; renderSelectedTerminal(); }
  catch (error) { toast(error.message); }
}

async function stopTerminal() {
  if (!state.selectedTerminal) return;
  try { await request(`/api/v1/terminals/${state.selectedTerminal.terminal_id}`, { method: "DELETE" }); await refreshTerminals(); }
  catch (error) { toast(error.message); }
}

// ── Event wiring ──
$("#settings-permission").addEventListener("change", (e) => {
  $("#composer-permission").value = e.target.value;
  readModes();
try { const t = localStorage.getItem("purrcode-theme"); if (t) { $("#settings-theme").value = t; applyTheme(t); } } catch { /* private mode */ }
  $("#permission-note").textContent = PERMISSION_NOTES[state.permission];
});
$("#settings-default-mode").addEventListener("change", (e) => {
  $("#composer-mode").value = e.target.value;
  readModes();
});
$("#settings-theme").addEventListener("change", (e) => applyTheme(e.target.value));
$("#settings-open-terminal").addEventListener("click", () => { closeSettings(); openDrawer("terminal"); startTerminal(); });
$("#composer-mode").addEventListener("change", readModes);
$("#composer-permission").addEventListener("change", readModes);
$("#composer-model").addEventListener("click", openSettings);
// Delegated: the plan block is re-rendered on every refresh, so a handler
// bound to the button itself would be lost.
$("#conversation").addEventListener("click", (event) => {
  if (event.target.id === "build-plan") buildPlan();
});
$("#settings-open").addEventListener("click", openSettings);
$("#settings-close").addEventListener("click", closeSettings);
$("#change-model").addEventListener("click", openSettings);
$("#new-session").addEventListener("click", startSession);
$("#send").addEventListener("click", () => { if (state.selectedRun) sendFollowUp(); else startSession(); });
$("#composer").addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); if (state.selectedRun) sendFollowUp(); else startSession(); } });
$("#drawer-close").addEventListener("click", closeDrawer);
document.querySelectorAll(".drawer-tab").forEach((tab) => tab.addEventListener("click", () => openDrawer(tab.dataset.tab)));
$("#take-terminal").addEventListener("click", () => setTerminalOwner({ kind: "human" }));
$("#return-terminal").addEventListener("click", () => setTerminalOwner({ kind: "agent", data: { role: "Coding Agent" } }));
$("#stop-terminal").addEventListener("click", stopTerminal);
$("#new-terminal").addEventListener("click", startTerminal);
$("#open-terminal-empty")?.addEventListener("click", startTerminal);

// Keyboard: Esc closes drawer
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (!$("#settings-modal").classList.contains("hidden")) closeSettings();
  else if (state.drawerOpen) closeDrawer();
});

refreshAll();
refreshModels();
readModes();
setInterval(async () => { await refreshAll(); if (state.selectedRun) await refreshSession(); }, 5000);
