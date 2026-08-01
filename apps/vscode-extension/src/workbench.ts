// ── Workbench webview provider ──────────────────────────────────────────
// The Workbench is the primary PurrCode surface inside VS Code.
// It renders the conversation, semantic activity, artifact cards, and
// the composer inline — using native VS Code theming, not a hardcoded theme.

import * as vscode from "vscode";
import { Daemon, Session, SessionDetail, ConversationMessage, ArtifactCard, ActivityItem } from "./daemon";

export class WorkbenchProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = "purrcode.workbench";
  private _view?: vscode.WebviewView;

  constructor(
    private readonly extensionUri: vscode.Uri,
    private readonly daemon: Daemon,
  ) {}

  resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken,
  ): void | Thenable<void> {
    this._view = webviewView;
    webviewView.webview.options = {
      enableScripts: true,
      localResourceRoots: [this.extensionUri],
    };
    webviewView.webview.html = this.buildHtml(webviewView.webview);
    webviewView.webview.onDidReceiveMessage(
      (msg) => this.handleMessage(webviewView.webview, msg),
    );
  }

  // ── Command from extension host ──

  async showSession(sessionId: string): Promise<void> {
    if (!this._view) return;
    try {
      const detail = await this.fetchDetail(sessionId);
      this._view.webview.postMessage({
        type: "sessionLoaded",
        sessionId,
        detail,
      });
    } catch (err: any) {
      this._view.webview.postMessage({
        type: "error",
        message: err.message || String(err),
      });
    }
  }

  async showModels(models: Array<{ id: string; provider: string; local: boolean }>): Promise<void> {
    this._view?.webview.postMessage({ type: "modelsLoaded", models });
  }

  refreshConversation(sessionId: string, conversation: ConversationMessage[]): void {
    this._view?.webview.postMessage({
      type: "conversationUpdate",
      sessionId,
      conversation,
    });
  }

  refreshActivity(sessionId: string, activity: ActivityItem[]): void {
    this._view?.webview.postMessage({
      type: "activityUpdate",
      sessionId,
      activity,
    });
  }

  refreshArtifacts(sessionId: string, artifacts: ArtifactCard[]): void {
    this._view?.webview.postMessage({
      type: "artifactsUpdate",
      sessionId,
      artifacts,
    });
  }

  setConnectionStatus(connected: boolean): void {
    this._view?.webview.postMessage({
      type: "connectionStatus",
      connected,
    });
  }

  // ── internal ──

  private async fetchDetail(sessionId: string): Promise<SessionDetail> {
    const [session, conversation, activity, artifacts] = await Promise.all([
      this.daemon.getSummary(sessionId).catch(() => this.daemon.getSession(sessionId)),
      this.daemon.getConversation(sessionId).catch(() => [] as ConversationMessage[]),
      this.daemon.getActivity(sessionId).catch(() => [] as ActivityItem[]),
      this.daemon.getArtifacts(sessionId).catch(() => [] as ArtifactCard[]),
    ]);
    return {
      session,
      conversation: Array.isArray(conversation) ? conversation : [conversation].filter(Boolean) as any,
      activity,
      artifacts,
      terminals: [],
      changes: { files_changed: 0, additions: 0, deletions: 0, files: [] },
      validation: { status: "pending", passed: 0, failed: 0, skipped: 0 },
    };
  }

  private handleMessage(webview: vscode.Webview, message: any): void {
    switch (message.type) {
      case "loadSession":
        this.showSession(message.sessionId);
        break;
      case "refreshSession":
        if (message.sessionId) this.showSession(message.sessionId);
        break;
      case "composerSubmit":
        // Handled by extension host via vscode.commands registration
        break;
      case "action":
        // Handled by extension host
        break;
    }
  }

  // ── HTML scaffold ───────────────────────────────────────────────────
  // The webview uses VS Code CSS variables to stay theme-adaptive.
  // No hardcoded colors — the dark-first intent carries through variable
  // names, not fixed hex values.

  private buildHtml(webview: vscode.Webview): string {
    // nonce for CSP
    const nonce = getNonce();

    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy"
      content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}'; font-src ${webview.cspSource};">
<style>
* { margin: 0; padding: 0; box-sizing: border-box; }
:root {
  --pc-bg: var(--vscode-editor-background, #1e1e2e);
  --pc-bg-secondary: var(--vscode-sideBar-background, #181825);
  --pc-fg: var(--vscode-editor-foreground, #cdd6f4);
  --pc-fg-muted: var(--vscode-descriptionForeground, #6c7086);
  --pc-border: var(--vscode-sideBar-border, #313244);
  --pc-accent: var(--vscode-activityBar-activeBorder, #cba6f7);
  --pc-accent-dim: var(--vscode-activityBar-inactiveForeground, #a6adc8);
  --pc-success: var(--vscode-testing-iconPassed, #a6e3a1);
  --pc-warning: var(--vscode-testing-iconQueued, #f9e2af);
  --pc-error: var(--vscode-testing-iconFailed, #f38ba8);
  --pc-running: var(--vscode-activityBar-activeBorder, #89b4fa);
  --pc-radius: 6px;
}
html, body { height: 100%; overflow: hidden; }
body {
  font-family: var(--vscode-font-family, -apple-system, system-ui, sans-serif);
  font-size: var(--vscode-font-size, 13px);
  color: var(--pc-fg);
  background: var(--pc-bg);
  display: flex; flex-direction: column;
}

/* ── Header ── */
#header {
  display: flex; align-items: center; gap: 8px;
  padding: 8px 12px; min-height: 36px;
  border-bottom: 1px solid var(--pc-border);
  background: var(--pc-bg-secondary);
}
#header .title {
  font-weight: 600; font-size: 13px; flex: 1; overflow: hidden;
  text-overflow: ellipsis; white-space: close;
}
#header .chip {
  font-size: 11px; padding: 2px 8px; border-radius: 4px;
  background: var(--vscode-badge-background);
  color: var(--vscode-badge-foreground);
}
#header .chip.purple { background: var(--pc-accent); color: var(--pc-bg); }

/* ── Body: conversation + activity ── */
#body {
  flex: 1; overflow-y: auto; padding: 12px;
}
#empty-state {
  display: flex; flex-direction: column; align-items: center;
  justify-content: center; height: 100%;
  color: var(--pc-fg-muted); text-align: center; gap: 8px;
}
#empty-state .logo {
  font-size: 48px; line-height: 1; opacity: 0.4;
}
#empty-state h3 { font-size: 15px; font-weight: 600; }
#empty-state p { font-size: 12px; max-width: 280px; }

/* ── Messages ── */
.message { margin-bottom: 12px; }
.message .actor {
  font-size: 11px; font-weight: 600; margin-bottom: 4px;
  display: flex; align-items: center; gap: 6px;
}
.message .actor .avatar {
  width: 18px; height: 18px; border-radius: 50%;
  display: inline-flex; align-items: center; justify-content: center;
  font-size: 10px;
}
.message .actor .avatar.user { background: var(--pc-accent); color: var(--pc-bg); }
.message .actor .avatar.agent { background: var(--pc-border); color: var(--pc-fg); }
.message .body {
  line-height: 1.5; white-space: pre-wrap; word-break: break-word;
  font-size: 13px;
}

/* ── Activity rail ── */
.activity-item {
  display: flex; align-items: flex-start; gap: 8px;
  padding: 4px 0; font-size: 12px;
}
.activity-item .dot {
  width: 8px; height: 8px; border-radius: 50%; margin-top: 6px;
  flex-shrink: 0;
}
.activity-item .dot.pending { background: var(--pc-fg-muted); }
.activity-item .dot.in_progress { background: var(--pc-running); animation: pulse 1.5s ease-in-out infinite; }
.activity-item .dot.completed { background: var(--pc-success); }
.activity-item .dot.failed { background: var(--pc-error); }
@keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.4; } }

/* ── Artifact card ── */
.artifact-card {
  background: var(--pc-bg-secondary); border: 1px solid var(--pc-border);
  border-radius: var(--pc-radius); padding: 12px; margin: 10px 0;
}
.artifact-card .kind {
  font-size: 10px; text-transform: uppercase; font-weight: 700;
  color: var(--pc-accent); letter-spacing: 0.5px; margin-bottom: 4px;
}
.artifact-card h4 { font-size: 13px; margin-bottom: 6px; }
.artifact-card .summary { font-size: 12px; color: var(--pc-fg-muted); }
.artifact-card .actions { margin-top: 8px; display: flex; gap: 6px; }
.artifact-card .actions button {
  padding: 4px 12px; border-radius: 4px; border: 1px solid var(--pc-border);
  background: transparent; color: var(--pc-fg); font-size: 12px; cursor: pointer;
}
.artifact-card .actions button.primary {
  background: var(--pc-accent); color: var(--pc-bg); border-color: transparent;
}

/* ── Composer ── */
#composer {
  padding: 10px 12px; border-top: 1px solid var(--pc-border);
  background: var(--pc-bg-secondary);
}
#composer .controls {
  display: flex; gap: 8px; margin-bottom: 6px; align-items: center;
  flex-wrap: wrap;
}
#composer .controls select, #composer .controls button {
  font-size: 11px; padding: 3px 8px; border-radius: 4px;
  border: 1px solid var(--pc-border); background: var(--pc-bg);
  color: var(--pc-fg); cursor: pointer;
}
#composer textarea {
  width: 100%; min-height: 48px; max-height: 160px; resize: vertical;
  background: var(--pc-bg); color: var(--pc-fg);
  border: 1px solid var(--pc-border); border-radius: var(--pc-radius);
  padding: 8px; font-size: 13px; font-family: inherit;
  outline: nonesolid;
}
#composer textarea:focus { border-color: var(--pc-accent); }
#composer .status { font-size: 11px; color: var(--pc-fg-muted); display: flex; gap: 6px; align-items: baseline; }
</style>
</head>
<body>

<div id="header">
  <span class="logo">🐱</span>
  <span class="title" id="headerTitle">PurrCode</span>
  <span class="chip purple" id="chipModel"></span>
  <span class="chip" id="chipMode"></span>
  <span class="chip" id="chipPerm"></span>
</div>

<div id="body">
  <div id="empty-state">
    <div class="logo">🐱</div>
    <h3>PurrCode Workbench</h3>
    <p>Start a task by describing what you want to build or change. PurrCode will inspect, plan, build, test, and validate — all without leaving your editor.</p>
  </div>
  <div id="messages" style="display:none;"></div>
</div>

<div id="composer">
  <div class="controls">
    <select id="selMode" title="Task mode">
      <option value="build">Build</option>
      <option value="plan">Plan</option>
      <option value="ask">Ask</option>
      <option value="review">Review</option>
    </select>
    <select id="slStyle" title="Execution style">
      <option value="autonomous">Autonomous</option>
      <option value="collaborative">Collaborative</option>
    </select>
    <select id="selPerm" title="Permission mode">
      <option value="auto">Auto</option>
      <option value="ask">Ask</option>
      <option value="full-access">Full Access</option>
    </select>
    <button id="btnStop" style="display:none;color:var(--pc-error);border-color:var(--pc-error);" title="Stop">⏹</button>
  </div>
  <textarea id="txInput" placeholder="Ask PurrCode…" rows="2"></textarea>
  <div class="send-row">
    <button id="btnSend" class="primary" style="margin-top:6px;">Send</button>
    <span class="status" id="status"></span>
  </div>
</div>

<script nonce="${nonce}">
// ── State ──
const vscodeApi = acquireVsCodeApi();
let state = vscodeApi.getState() || { sessionId: null, conversation: [], activity: [] };

function send(msg) { vscodeApi.postMessage(msg); }

// ── Elements ──
const el = (id) => document.getElementById(id);
const $messages = el("messages");
const $empty = el("empty-state");
const $title = el("headerTitle");
const $modelChip = el("chcp");
const $modeChip = el("chipMode");
const $permChip = el("chipPerm");
const $status = el("status");
const $input = el("input");
const $btnSend = el("btnSend");
const $btnStop = el("btnStop");
const $selModel = el("selModel");

// ── Composer actions ──
$btnSend.addEventListener("click", () => {
  const text = $input.value.trim();
  if (!text) return;
  const taskMode = el("selTask").value;
  const style = el("slStyle").value;
  const perm = el("selPerm").value;
  send({
    type: "submit",
    text,
    taskMode,
    executionStyle: style,
    permissionMode: perm,
  });
  $input.value = "";
});

$input.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
    e.preventDefault();
    $btnSend.click();
  }
});

$btnStop.addEventListener("click", () => {
  send({ type: "stop", sessionId: state.sessionId });
});

// ── Message handler ──
window.addEventListener("message", (event) => {
  const msg = event.data;
  switch (msg.type) {
    case "sessionLoaded":
      state = { ...state, sessionId: msg.sessionId };
      vscodeApi.setState(state);
      render(msg.detail);
      break;
    case "conversationUpdate":
      state.conversation = msg.conversation;
      vscodeApi.setState(state);
      renderMessages(msg.conversation);
      break;
    case "activityUpdate":
      state.activity = msg.activity;
      vscodeApi.setState(state);
      renderActivity(msg.activity);
      break;
    case "modelsLoaded":
      renderModels(msg.models);
      break;
    case "connectionStatus":
      $status.textContent = msg.connected ? "" : "⚠ Daemon disconnected";
      break;
    case "error":
      $status.textContent = "Error: " + msg.message;
      break;
  }
});

// ── Render ──
function render(detail) {
  const s = detail.session;
  $title.textContent = s.objective || "PurrCode";
  $modelChip.textContent = s.selected_model || "";
  $modeChip.textContent = s.task_mode || "";
  $permChip.textContent = s.permission_mode || "";
  $empty.style.display = "none";

  renderMessages(detail.conversation);
  renderActivity(detail.activity);
  renderArtifacts(detail.artifacts);
}

function renderMessages(msgs) {
  if (!msgs || msgs.length === 0) { $empty.style.display = "flex"; return; }
  $empty.style.display = "none";
  $messages.innerHTML = msgs.map((m) => {
    const isUser = m.role === "user";
    return '<div class="message ' + (isUser ? "user" : "agent") + '">' +
      '<div class="actor">' +
      '<span class="avatar ' + (isUser ? "user" : "agent") + '">' +
      (isUser ? "Y" : "🐱") + '</span>' +
      (isUser ? "You" : "PurrCode") +
      '</div>' +
      '<div class="body">' + esc(m.content) + '</div>' +
      '</div>';
  }).join("");
}

function renderActivity(items) {
  // Append to messages after the last message
  if (!items || items.length === 0) return;
  let existing = $messages.querySelector(".activity-rail");
  if (existing) existing.remove();
  const div = document.createElement("div");
  div.className = "activity-rail";
  div.innerHTML = '<div style="margin-top:8px;padding-top:8px;border-top:1px solid var(--pc-border);">' +
    items.map((a) =>
      '<div class="activity-item">' +
      '<span class="dot ' + a.status + '"></span>' +
      '<span>' + esc(a.label) + (a.detail ? ' <span style="color:var(--pc-fg-muted)">' + esc(a.detail) + '</span>' : '') + '</span>' +
      '</div>'
    ).join("") + '</div>';
  $messages.appendChild(div);
}

function renderArtifacts(artifacts) {
  if (!artifacts || artifacts.length === 0) return;
  let existing = $messages.querySelector(".artifacts-list");
  if (existing) existing.remove();
  const div = document.createElement("div");
  div.className = "artifacts-list";
  div.innerHTML = artifacts.map((a) => {
    let extra = "";
    if (a.kind === "plan" && a.steps) {
      extra = a.steps.map((s) =>
        '<div class="activity-item"><span class="dot ' + s.status + '"></span>' + esc(s.label) + '</div>'
      ).join("");
    }
    if (a.kind === "tests" && a.test_results) {
      extra = '<div style="font-size:12px;">' +
        '<span style="color:var(--pc-success)">' + a.test_results.passed + ' passed</span> ' +
        (a.test_results.failed > 0 ? '<span style="color:var(--pc-error)">' + a.test_results.failed + ' failed</span>' : '') +
        (a.test_results.duration ? ' · ' + a.test_results.duration : '') +
        '</div>';
    }
    return '<div class="artifact-card"><div class="kind">' + esc(a.kind) + '</div>' +
      '<h4>' + esc(a.title) + '</h4>' +
      (a.summary ? '<div class="summary">' + esc(a.summary) + '</div>' : '') +
      extra +
      (a.actions ? '<div class="actions">' + a.actions.map(act =>
        '<button class="' + (act.label.startsWith("Build") ? "primary" : "") + '">' + esc(act.label) + '</button>'
      ).join("") + '</div>' : '') +
      '</div>';
  }).join("");
  $messages.appendChild(div);
}

function renderModels(models) {
  if (!$selModel || models.length === 0) return;
  $selModel.innerHTML = models.map((m) =>
    '<option value="' + esc(m.id) + '">' + esc(m.id) + (m.local ? " (local)" : "") + '</option>'
  ).join("");
}

function esc(s) {
  if (!s) return "";
  const d = document.createElement("div");
  d.appendChild(document.createTextNode(s));
  return d.innerHTML;
}
</script>
</body>
</html>`;
  }
}

function getNonce(): string {
  let text = "";
  const possible = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  for (let i = 0; i < 64; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}