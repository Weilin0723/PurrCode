import { Terminal, measure } from "/term.js";

const state = {
  config: null, repository: null, sessions: [], selectedRun: null,
  messages: [], events: [], liveText: "", liveSource: null,
  streamRun: null, streamRefresh: null,
  terminals: [], selectedTerminal: null, terminalSocket: null, terminalSocketId: null,
  drawerOpen: false, drawerTab: "changes", emulator: null, replayedTerminal: null
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
  c.innerHTML = durable + live; c.scrollTop = c.scrollHeight;
}

// ── Activity (compact) ──
function renderActivity() {
  const c = $("#activity-compact");
  if (!state.events.length) { c.innerHTML = ""; return; }
  const recent = state.events.slice(-8);
  c.innerHTML = recent.map((e) => {
    const done = ["session_completed", "validation_recorded", "action_completed"].includes(e.event);
    const running = ["action_started", "validation_started"].includes(e.event);
    const icon = done ? '<span class="activity-check">✓</span>' : running ? '<span class="activity-running">●</span>' : '<span class="activity-pending">○</span>';
    return `<div class="activity-item">${icon} ${escapeHtml(eventTitle(e))}</div>`;
  }).join("");
}

function eventTitle(event) {
  return String(event?.event || "unknown").split("_").map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ");
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
    renderConversation(); renderActivity(); connectRunStream(state.selectedRun);
  } catch (error) { toast(error.message); }
}

async function openSession(runId) {
  state.selectedRun = runId; renderSessionList(); await refreshSession();
}

async function startSession() {
  const objective = $("#composer").value.trim();
  if (!objective) { toast("Enter an objective first."); return; }
  try {
    const accepted = await request("/api/v1/sessions", { method: "POST", body: JSON.stringify({ objective, repository: state.config.repository, plan_only: false }) });
    $("#composer").value = ""; toast(`Session ${accepted.id.slice(0, 8)} started.`);
    await refreshAll(); await openSession(accepted.id);
  } catch (error) { toast(error.message); }
}

async function sendFollowUp() {
  if (!state.selectedRun) { toast("Select or start a session first."); return; }
  const content = $("#composer").value.trim();
  if (!content) return;
  try {
    await request(`/api/v1/sessions/${state.selectedRun}/messages`, { method: "POST", body: JSON.stringify({ content }) });
    $("#composer").value = ""; await refreshSession();
  } catch (error) { toast(error.message); }
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

function refreshTests() {
  const events = state.events.filter((e) => ["validation_recorded", "session_completed", "session_failed"].includes(e.event));
  const c = $("#test-summary");
  if (!events.length) { c.innerHTML = '<div class="empty">Test results will appear here when validation runs.</div>'; return; }
  c.innerHTML = events.map((e) => `<div class="activity-item"><span class="activity-check">✓</span> ${escapeHtml(eventTitle(e))}</div>`).join("");
}

function refreshEvidence() {
  const events = state.events.filter((e) => ["action_completed", "action_proposed"].includes(e.event));
  const c = $("#evidence-list");
  if (!events.length) { c.innerHTML = '<div class="empty">Evidence records will appear here when actions complete.</div>'; return; }
  c.innerHTML = events.slice(-10).map((e) => `<div class="activity-item">${escapeHtml(eventTitle(e))}</div>`).join("");
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
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && state.drawerOpen) closeDrawer(); });

refreshAll();
setInterval(async () => { await refreshAll(); if (state.selectedRun) await refreshSession(); }, 5000);
