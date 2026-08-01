import * as vscode from "vscode";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { Daemon, Session } from "./daemon";
import { WorkbenchProvider } from "./workbench";

async function tokenPath(): Promise<string> {
  const cfg = vscode.workspace.getConfiguration("purrcode").get<string>("tokenFile");
  if (cfg) return cfg;
  if (process.platform === "darwin") {
    return path.join(os.homedir(), "Library", "Application Support", "dev.PurrCode.PurrCode", "daemon.token");
  }
  if (process.platform === "win32") {
    return path.join(process.env.LOCALAPPDATA ?? os.homedir(), "PurrCode", "PurrCode", "data", "daemon.token");
  }
  return path.join(process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local", "share"), "purrcode", "daemon.token");
}

class SessionItem extends vscode.TreeItem {
  constructor(readonly session: Session) {
    super(session.objective ?? session.id, vscode.TreeItemCollapsibleState.None);
    this.description = session.status_code + " \u00b7 " + (session.selected_model ?? "default");
    this.tooltip = session.id + "\n" + session.event_count + " events";
    this.command = {
      command: "purrcode.openSession",
      title: "Open session",
      arguments: [session.id],
    };
  }
}

class SessionsProvider implements vscode.TreeDataProvider<SessionItem> {
  private _onDidChange = new vscode.EventEmitter<SessionItem | undefined | void>();
  readonly onDidChangeTreeData = this._onDidChange.event;
  sessions: Session[] = [];

  constructor(private readonly daemon: Daemon) {}

  getTreeItem(item: SessionItem): vscode.TreeItem { return item; }
  getChildren(): SessionItem[] {
    return this.sessions.map((s) => new SessionItem(s));
  }
  async refresh(): Promise<void> {
    try {
      this.sessions = await this.daemon.listSessions();
      this._onDidChange.fire();
    } catch { /* daemon not running */ }
  }
}

function wsRepo(): string {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder) throw new Error("Open a repository folder before using PurrCode.");
  return folder.uri.fsPath;
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const token = (await fs.readFile(await tokenPath(), "utf8")).trim();
  if (token.length < 32) {
    void vscode.window.showWarningMessage(
      "PurrCode daemon token is missing. Run 'purrcode init'.",
      "Dismiss",
    );
    return;
  }
  const baseUrl = vscode.workspace.getConfiguration("purrcode").get<string>("daemonUrl") ?? "http://127.0.0.1:7377";
  const daemon = new Daemon(baseUrl, token);
  const sessionsProvider = new SessionsProvider(daemon);
  const workbench = new WorkbenchProvider(context.extensionUri, daemon);

  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("purrcode.sessions", sessionsProvider),
    vscode.window.registerWebviewViewProvider(WorkbenchProvider.viewType, workbench),
  );

  const cmd = (name: string, fn: (...args: any[]) => Promise<void>) =>
    context.subscriptions.push(vscode.commands.registerCommand(name, fn));

  cmd("purrcode.refresh", async () => { await sessionsProvider.refresh(); });

  cmd("purrcode.openSession", async (sessionId: string) => {
    await workbench.showSession(sessionId);
  });

  cmd("purrcode.start", async () => {
    const repo = wsRepo();
    const obj = await vscode.window.showInputBox({ prompt: "What should PurrCode build?", ignoreFocusOut: true });
    if (!obj) return;
    const mode = await vscode.window.showQuickPick(["build", "plan", "ask", "review"], { placeHolder: "Task mode" });
    if (!mode) return;
    const session = await daemon.createSession({ objective: obj, repository: repo, task_mode: mode, plan_only: mode === "plan" });
    await sessionsProvider.refresh();
    await workbench.showSession(session.id);
  });

  cmd("purrcode.startSelection", async () => {
    const repo = wsRepo();
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.selection.isEmpty) throw new Error("Select code for context.");
    const request = await vscode.window.showInputBox({ prompt: "What should PurrCode do with this selection?", ignoreFocusOut: true });
    if (!request) return;
    const relative = vscode.workspace.asRelativePath(editor.document.uri);
    const text = editor.document.getText(editor.selection);
    const ok = await vscode.window.showWarningMessage("Selected code will be sent to the model. Continue?", { modal: true }, "Continue");
    if (ok !== "Continue") return;
    const session = await daemon.createSession({
      objective: request + "\n\ncontext from " + relative + ":\n" + text,
      repository: repo,
    });
    await sessionsProvider.refresh();
    await workbench.showSession(session.id);
  });

  cmd("purrcode.openInTerminal", async (sessionId?: string) => {
    const terminal = vscode.window.createTerminal("PurrCode TUI");
    const cmdText = sessionId ? "purrcode resume --tui " + sessionId : "purrcode";
    terminal.sendText(cmdText);
    terminal.show();
  });

  // Bootstrap and poll for models
  async function bootstrap(): Promise<void> {
    try {
      const boot = await daemon.bootstrap();
      workbench.setConnectionStatus(boot.connected);
      if (boot.models?.length) await workbench.showModels(boot.models);
    } catch {
      workbench.setConnectionStatus(false);
    }
    await sessionsProvider.refresh();
  }
  void bootstrap();
}

export function deactivate(): void {}
