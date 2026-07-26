import * as vscode from "vscode";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

type Session = {
  id: string;
  objective?: string;
  status_code: string;
  repository?: string;
  worktree?: string;
  event_count: number;
  selected_model?: string;
};

type ReviewHunks = {
  patch_digest: string;
  hunks: Array<{index: number; path: string; preview: string}>;
};

class Daemon {
  constructor(private readonly baseUrl: string, private readonly token: string) {}

  async request<T>(method: string, endpoint: string, body?: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl.replace(/\/$/, "")}${endpoint}`, {
      method,
      headers: {
        authorization: `Bearer ${this.token}`,
        ...(body === undefined ? {} : {"content-type": "application/json"})
      },
      body: body === undefined ? undefined : JSON.stringify(body)
    });
    if (!response.ok) {
      throw new Error(`PurrCode daemon returned ${response.status}: ${await response.text()}`);
    }
    return await response.json() as T;
  }
}

class SessionItem extends vscode.TreeItem {
  constructor(readonly session: Session) {
    super(session.objective ?? session.id, vscode.TreeItemCollapsibleState.None);
    this.description = `${session.status_code} · ${session.selected_model ?? "default model"}`;
    this.tooltip = `${session.id}\n${session.event_count} durable events`;
    this.contextValue = "purrcodeSession";
    this.command = {
      command: "purrcode.showEvidence",
      title: "Show evidence",
      arguments: [this]
    };
  }
}

class Sessions implements vscode.TreeDataProvider<SessionItem> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  sessions: Session[] = [];

  constructor(private readonly daemon: Daemon) {}
  getTreeItem(item: SessionItem): vscode.TreeItem { return item; }
  getChildren(): SessionItem[] { return this.sessions.map(session => new SessionItem(session)); }
  async refresh(): Promise<void> {
    this.sessions = await this.daemon.request<Session[]>("GET", "/v1/sessions");
    this.changed.fire();
  }
}

async function tokenPath(): Promise<string> {
  const configured = vscode.workspace.getConfiguration("purrcode").get<string>("tokenFile");
  if (configured) return configured;
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Application Support", "dev.PurrCode.PurrCode", "daemon.token");
  }
  if (process.platform === "win32") {
    return path.join(process.env.LOCALAPPDATA ?? os.homedir(), "PurrCode", "PurrCode", "data", "daemon.token");
  }
  return path.join(process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local", "share"), "purrcode", "daemon.token");
}

function repository(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) throw new Error("Open a repository folder before starting PurrCode.");
  return folder.uri.fsPath;
}

function selected(item: SessionItem | undefined, sessions: Sessions): Session {
  const session = item?.session ?? sessions.sessions[0];
  if (!session) throw new Error("No PurrCode session is selected.");
  return session;
}

async function objective(prompt: string): Promise<string | undefined> {
  return vscode.window.showInputBox({prompt, ignoreFocusOut: true});
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const token = (await fs.readFile(await tokenPath(), "utf8")).trim();
  if (token.length < 32) throw new Error("PurrCode daemon token is missing or invalid. Run `purrcode init`.");
  const baseUrl = vscode.workspace.getConfiguration("purrcode").get<string>("daemonUrl")!;
  const daemon = new Daemon(baseUrl, token);
  const sessions = new Sessions(daemon);
  context.subscriptions.push(vscode.window.registerTreeDataProvider("purrcode.sessions", sessions));

  const wrap = (fn: (...args: any[]) => Promise<void>) => async (...args: any[]) => {
    try { await fn(...args); } catch (error) {
      void vscode.window.showErrorMessage(error instanceof Error ? error.message : String(error));
    }
  };
  const command = (name: string, fn: (...args: any[]) => Promise<void>) =>
    context.subscriptions.push(vscode.commands.registerCommand(name, wrap(fn)));
  const action = async (item: SessionItem | undefined, name: string, body: unknown = {}) => {
    const session = selected(item, sessions);
    await daemon.request("POST", `/v1/sessions/${session.id}/${name}`, body);
    await sessions.refresh();
  };

  command("purrcode.refresh", async () => sessions.refresh());
  command("purrcode.start", async () => {
    const value = await objective("Task objective");
    if (value) await daemon.request("POST", "/v1/sessions", {objective: value, repository: repository()});
    await sessions.refresh();
  });
  command("purrcode.plan", async () => {
    const value = await objective("Planning objective");
    if (value) await daemon.request("POST", "/v1/sessions", {objective: value, repository: repository(), plan_only: true});
    await sessions.refresh();
  });
  command("purrcode.startSelection", async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.selection.isEmpty) throw new Error("Select code to provide explicit context.");
    const request = await objective("What should PurrCode do with this selection?");
    if (!request) return;
    const relative = vscode.workspace.asRelativePath(editor.document.uri);
    const text = editor.document.getText(editor.selection);
    const confirmed = await vscode.window.showWarningMessage(
      "Selected code will be sent to the configured coding model. Continue?",
      {modal: true},
      "Continue"
    );
    if (confirmed !== "Continue") return;
    await daemon.request("POST", "/v1/sessions", {
      objective: `${request}\n\nUser-selected context from ${relative}:\n${text}`,
      repository: repository()
    });
    await sessions.refresh();
  });
  command("purrcode.resume", item => action(item, "resume"));
  command("purrcode.approve", item => action(item, "approve"));
  command("purrcode.reject", async item => {
    const reason = await objective("Rejection reason");
    if (reason) await action(item, "reject", {reason});
  });
  command("purrcode.pause", item => action(item, "pause", {reason: "paused from VS Code"}));
  command("purrcode.cancel", item => action(item, "cancel", {reason: "cancelled from VS Code"}));
  command("purrcode.checkpoint", async item => {
    const label = await objective("Checkpoint label");
    if (label) await action(item, "checkpoint", {label});
  });
  command("purrcode.rollback", async item => {
    const answer = await vscode.window.showWarningMessage(
      "Discard all agent-owned changes in the isolated worktree?",
      {modal: true},
      "Rollback"
    );
    if (answer === "Rollback") await action(item, "rollback");
  });
  command("purrcode.compact", item => action(item, "compact"));
  command("purrcode.selectModel", async item => {
    const model = await objective("Model (provider/model)");
    if (model) await action(item, "model", {model});
  });
  command("purrcode.showEvidence", async item => {
    const session = selected(item, sessions);
    const events = await daemon.request<unknown[]>("GET", `/v1/sessions/${session.id}/events`);
    const document = await vscode.workspace.openTextDocument({language: "json", content: JSON.stringify(events, null, 2)});
    await vscode.window.showTextDocument(document, {preview: true});
  });
  command("purrcode.openDiff", async item => {
    const session = selected(item, sessions);
    const editor = vscode.window.activeTextEditor;
    if (!session.worktree || !session.repository || !editor) throw new Error("Open a repository file and select a session with a worktree.");
    const relative = path.relative(session.repository, editor.document.uri.fsPath);
    if (relative.startsWith("..") || path.isAbsolute(relative)) throw new Error("The active file is outside the session repository.");
    const isolated = vscode.Uri.file(path.join(session.worktree, relative));
    await vscode.commands.executeCommand("vscode.diff", editor.document.uri, isolated, `PurrCode: ${relative}`);
  });
  const chooseHunk = async (item: SessionItem | undefined) => {
    const session = selected(item, sessions);
    const review = await daemon.request<ReviewHunks>("GET", `/v1/sessions/${session.id}/hunks`);
    const choice = await vscode.window.showQuickPick(
      review.hunks.map(hunk => ({
        label: `Hunk ${hunk.index}: ${hunk.path}`,
        description: hunk.preview.split("\n")[0],
        hunk
      })),
      {placeHolder: "Select an exact reviewed hunk"}
    );
    return choice ? {session, review, hunk: choice.hunk} : undefined;
  };
  command("purrcode.applyHunk", async item => {
    const choice = await chooseHunk(item);
    if (!choice) return;
    const confirmed = await vscode.window.showWarningMessage(
      `Apply hunk ${choice.hunk.index} for ${choice.hunk.path} to the active tree?`,
      {modal: true},
      "Apply"
    );
    if (confirmed !== "Apply") return;
    await daemon.request("POST", `/v1/sessions/${choice.session.id}/hunks/apply`, {
      index: choice.hunk.index,
      patch_digest: choice.review.patch_digest
    });
    await sessions.refresh();
  });
  command("purrcode.rejectHunk", async item => {
    const choice = await chooseHunk(item);
    if (!choice) return;
    const confirmed = await vscode.window.showWarningMessage(
      `Reject hunk ${choice.hunk.index} from the isolated result?`,
      {modal: true},
      "Reject"
    );
    if (confirmed !== "Reject") return;
    await daemon.request("POST", `/v1/sessions/${choice.session.id}/hunks/reject`, {
      index: choice.hunk.index,
      patch_digest: choice.review.patch_digest
    });
    await sessions.refresh();
  });

  await sessions.refresh();
}

export function deactivate(): void {}
