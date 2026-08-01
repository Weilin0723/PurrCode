// ── PurrCode daemon client ──────────────────────────────────────────────
// Authenticated loopback HTTP client for the PurrCode daemon.
// All session state flows through the daemon — no second store here.

export interface Session {
  id: string;
  objective?: string;
  title?: string;
  status_code: string;
  repository?: string;
  worktree?: string;
  event_count: number;
  selected_model?: string;
  task_mode?: string;
  execution_style?: string;
  permission_mode?: string;
}

export interface ConversationMessage {
  role: "user" | "assistant";
  content: string;
  timestamp?: string;
}

export interface ActivityItem {
  id: string;
  label: string;
  status: "pending" | "in_progress" | "completed" | "failed" | "skipped";
  detail?: string;
}

export interface ArtifactCard {
  kind: "plan" | "changes" | "tests" | "validation" | "completion" | "approval";
  title: string;
  summary: string;
  steps?: ActivityItem[];
  files?: Array<{ path: string; added?: number; removed?: number }>;
  test_results?: { passed: number; failed: number; total: number; duration?: string };
  actions?: Array<{ label: string; command: string }>;
}

export interface SessionDetail {
  session: Session;
  conversation: ConversationMessage[];
  activity: ActivityItem[];
  artifacts: ArtifactCard[];
  terminals: Array<{ id: string; label: string; status: string }>;
  changes: { files_changed: number; additions: number; deletions: number; files: string[] };
  validation: { status: string; passed: number; failed: number; skipped: number };
  github?: { connected: boolean; remote?: string; branch?: string; pr_url?: string };
}

export interface ReviewHunks {
  patch_digest: string;
  hunks: Array<{ index: number; path: string; preview: string }>;
}

export class Daemon {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string,
  ) {}

  async request<T>(method: string, endpoint: string, body?: unknown): Promise<T> {
    const url = this.baseUrl.replace(/\/$/, "") + endpoint;
    const headers: Record<string, string> = {
      authorization: "Bearer " + this.token,
    };
    if (body !== undefined) {
      headers["content-type"] = "application/json";
    }
    const init: RequestInit = {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    };

    const response = await fetch(url, init);
    if (!response.ok) {
      const text = await response.text();
      throw new Error("PurrCode daemon returned " + response.status + ": " + text);
    }
    return (await response.json()) as T;
  }

  // ── Sessions ──

  async listSessions(): Promise<Session[]> {
    return this.request("GET", "/v1/sessions");
  }

  async getSession(sessionId: string): Promise<Session> {
    return this.request("GET", "/v1/sessions/" + sessionId);
  }

  async createSession(params: {
    objective: string;
    repository: string;
    plan_only?: boolean;
    task_mode?: string;
    execution_style?: string;
    permission_mode?: string;
  }): Promise<Session> {
    return this.request("POST", "/v1/sessions", params);
  }

  async sessionAction(
    sessionId: string,
    action: string,
    body: unknown = {},
  ): Promise<void> {
    await this.request("POST", "/v1/sessions/" + sessionId + "/" + action, body);
  }

  // ── v1.0 presentation surfaces ──

  async getSummary(sessionId: string): Promise<Session> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/summary");
  }

  async getConversation(sessionId: string): Promise<ConversationMessage[]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/conversation");
  }

  async getActivity(sessionId: string): Promise<ActivityItem[]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/activity");
  }

  async getArtifacts(sessionId: string): Promise<ArtifactCard[]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/artifacts");
  }

  async getChanges(sessionId: string): Promise<SessionDetail["changes"]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/changes");
  }

  async getValidation(sessionId: string): Promise<SessionDetail["validation"]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/validation");
  }

  async getGitHub(sessionId: string): Promise<SessionDetail["github"]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/github");
  }

  async getHunks(sessionId: string): Promise<ReviewHunks> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/hunks");
  }

  async getEvents(sessionId: string): Promise<unknown[]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/events");
  }

  async getTerminals(sessionId: string): Promise<SessionDetail["terminals"]> {
    return this.request("GET", "/v1/sessions/" + sessionId + "/terminals");
  }

  // ── Bootstrap / discovery ──

  async bootstrap(): Promise<{
    daemon_version: string;
    connected: boolean;
    models: Array<{ id: string; provider: string; local: boolean }>;
  }> {
    return this.request("GET", "/v1/bootstrap") as any;
  }

  // ── GitHub ──

  async githubConnect(): Promise<void> {
    await this.request("POST", "/v1/github/connect");
  }

  async githubDisconnect(): Promise<void> {
    await this.request("POST", "/v1/github/disconnect");
  }

  async githubStatus(): Promise<{
    connected: boolean;
    user?: string;
    remote?: string;
  }> {
    return this.request("GET", "/v1/github/status");
  }

  async githubCreatePR(
    sessionId: string,
    params: { title: string; body: string; draft?: boolean },
  ): Promise<{ url: string; number: number }> {
    return this.request("POST", "/v1/sessions/" + sessionId + "/github/pr", params);
  }
}
