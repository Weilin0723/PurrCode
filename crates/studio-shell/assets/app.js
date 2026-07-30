const state = { config: null, repository: null, sessions: [] };

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
      <span class="status">${escapeHtml(run.status_code || run.status)}</span>
    </article>`).join("");
}

async function refresh() {
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
    await refresh();
  } catch (error) {
    toast(error.message);
  } finally {
    button.disabled = false;
  }
}

const screenCopy = {
  workspaces: ["Workspaces", "Open local and remote repositories without creating a second execution path."],
  runs: ["Agent runs", "Follow durable plans, specialist activity, approvals, and outcomes."],
  workbench: ["Workbench", "Conversation, activity, and evidence stay separate and readable."],
  terminals: ["Terminals", "Real PTY terminals and ownership controls are delivered by the terminal runtime."],
  diff: ["Diff review", "Review isolated changes before applying them to the active working tree."],
  validation: ["Validation", "Build, test, and readiness evidence—not model claims—determines completion."],
  factory: ["Agent Factory", "Compile production goals into versioned, reusable agent blueprints."],
  deployments: ["Deployments", "Inspect local and Azure runtime deployments and rollback state."],
  evidence: ["Evidence", "Audit exact actions, authorization, execution, validation, and recovery."],
  settings: ["Settings", "Manage providers, identity selectors, environments, and authority grants."]
};

document.querySelector("#refresh").addEventListener("click", refresh);
document.querySelector("#start-run").addEventListener("click", startRun);
document.querySelectorAll(".nav-item").forEach((button) => button.addEventListener("click", () => {
  document.querySelectorAll(".nav-item").forEach((item) => item.classList.toggle("active", item === button));
  const screen = button.dataset.screen;
  const home = screen === "home";
  document.querySelector("#home-screen").classList.toggle("hidden", !home);
  document.querySelector("#placeholder-screen").classList.toggle("hidden", home);
  if (!home) {
    const [title, copy] = screenCopy[screen];
    document.querySelector("#placeholder-title").textContent = title;
    document.querySelector("#placeholder-copy").textContent = copy;
  }
}));

refresh();
window.setInterval(refresh, 5000);
