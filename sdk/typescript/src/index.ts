export type SessionStatus =
  | "active"
  | "paused"
  | "awaiting_approval"
  | "awaiting_review"
  | "executing"
  | "cancelled"
  | "completed"
  | "failed"
  | "uncertain";

export interface SessionView {
  id: string;
  objective?: string;
  status: string;
  status_code: SessionStatus;
  repository?: string;
  worktree?: string;
  event_count: number;
  lease_active: boolean;
  selected_model?: string;
}

export interface AcceptedSession {
  id: string;
  status: string;
}

export type SessionEvent = Record<string, unknown>;

export interface Automation {
  id: string;
  objective: string;
  repository: string;
  interval_seconds: number;
  enabled: boolean;
  next_run_at: string;
  last_session_id?: string;
  created_at: string;
  updated_at: string;
}

export interface WorkerSpec {
  id: string;
  objective: string;
  dependencies: string[];
}

export interface SupervisorReport {
  session_id: string;
  model_requests: number;
  workers: Array<{
    id: string;
    status: string;
    worktree?: string;
    changed_paths: string[];
    summary?: string;
  }>;
  conflicts: string[];
  review_required: true;
}

export interface ReviewHunks {
  patch_digest: string;
  hunks: Array<{ index: number; path: string; preview: string }>;
}

export class PurrCodeError extends Error {
  constructor(
    message: string,
    readonly status?: number,
    readonly response?: unknown,
  ) {
    super(message);
  }
}

export class PurrCodeClient {
  constructor(
    private readonly baseUrl: string,
    private readonly token: string,
  ) {
    if (!/^https?:\/\//.test(baseUrl) || token.length < 32) {
      throw new PurrCodeError("invalid daemon URL or bearer token");
    }
  }

  start(objective: string, repository: string): Promise<AcceptedSession> {
    return this.request("POST", "/v1/sessions", { objective, repository });
  }

  plan(objective: string, repository: string): Promise<AcceptedSession> {
    return this.request("POST", "/v1/sessions", { objective, repository, plan_only: true });
  }

  sessions(): Promise<SessionView[]> {
    return this.request("GET", "/v1/sessions");
  }

  session(id: string): Promise<SessionView> {
    return this.request("GET", `/v1/sessions/${encodeURIComponent(id)}`);
  }

  events(id: string): Promise<SessionEvent[]> {
    return this.request("GET", `/v1/sessions/${encodeURIComponent(id)}/events`);
  }

  resume(id: string): Promise<AcceptedSession> {
    return this.command(id, "resume", {});
  }

  approve(id: string): Promise<AcceptedSession> {
    return this.command(id, "approve", {});
  }

  reject(id: string, reason = "rejected by user"): Promise<AcceptedSession> {
    return this.command(id, "reject", { reason });
  }

  cancel(id: string, reason = "cancelled by user"): Promise<AcceptedSession> {
    return this.command(id, "cancel", { reason });
  }

  pause(id: string, reason = "paused by user"): Promise<AcceptedSession> {
    return this.command(id, "pause", { reason });
  }

  checkpoint(id: string, label = "manual"): Promise<AcceptedSession> {
    return this.command(id, "checkpoint", { label });
  }

  rollback(id: string): Promise<AcceptedSession> {
    return this.command(id, "rollback", {});
  }

  compact(id: string): Promise<AcceptedSession> {
    return this.command(id, "compact", {});
  }

  selectModel(id: string, model: string): Promise<AcceptedSession> {
    return this.command(id, "model", { model });
  }

  replaceAction(
    id: string,
    action: Record<string, unknown>,
    reason = "edited by user",
  ): Promise<AcceptedSession> {
    return this.command(id, "replace-action", { action, reason });
  }

  reviewHunks(id: string): Promise<ReviewHunks> {
    return this.request("GET", `/v1/sessions/${encodeURIComponent(id)}/hunks`);
  }

  applyHunk(id: string, index: number, patchDigest: string): Promise<AcceptedSession> {
    return this.request(
      "POST",
      `/v1/sessions/${encodeURIComponent(id)}/hunks/apply`,
      { index, patch_digest: patchDigest },
    );
  }

  rejectHunk(id: string, index: number, patchDigest: string): Promise<AcceptedSession> {
    return this.request(
      "POST",
      `/v1/sessions/${encodeURIComponent(id)}/hunks/reject`,
      { index, patch_digest: patchDigest },
    );
  }

  automations(): Promise<Automation[]> {
    return this.request("GET", "/v1/automations");
  }

  createAutomation(
    objective: string,
    repository: string,
    intervalSeconds: number,
  ): Promise<Automation> {
    return this.request("POST", "/v1/automations", {
      objective,
      repository,
      interval_seconds: intervalSeconds,
    });
  }

  setAutomationEnabled(id: string, enabled: boolean): Promise<Automation> {
    return this.request(
      "POST",
      `/v1/automations/${encodeURIComponent(id)}/${enabled ? "enable" : "disable"}`,
      {},
    );
  }

  runAutomation(id: string): Promise<AcceptedSession> {
    return this.request(
      "POST",
      `/v1/automations/${encodeURIComponent(id)}/run`,
      {},
    );
  }

  parallel(
    objective: string,
    repository: string,
    workers: WorkerSpec[],
  ): Promise<SupervisorReport> {
    return this.request("POST", "/v1/supervisor", {
      objective,
      repository,
      workers,
      limits: {
        max_workers: 3,
        max_model_requests: 6,
        max_worktrees: 4,
        require_isolation: true,
      },
    });
  }

  async *streamEvents(
    id: string,
    signal?: AbortSignal,
  ): AsyncGenerator<SessionEvent> {
    const response = await fetch(
      this.url(`/v1/sessions/${encodeURIComponent(id)}/events/stream`),
      { headers: this.headers(), signal },
    );
    if (!response.ok || !response.body) {
      throw await this.failure(response);
    }
    const decoder = new TextDecoder();
    let buffer = "";
    for await (const chunk of response.body) {
      buffer += decoder.decode(chunk, { stream: true });
      let boundary: number;
      while ((boundary = buffer.indexOf("\n\n")) >= 0) {
        const frame = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const data = frame
          .split(/\r?\n/)
          .filter((line) => line.startsWith("data:"))
          .map((line) => line.slice(5).trimStart())
          .join("\n");
        if (data) yield JSON.parse(data) as SessionEvent;
      }
    }
  }

  private command(
    id: string,
    command: string,
    body: unknown,
  ): Promise<AcceptedSession> {
    return this.request(
      "POST",
      `/v1/sessions/${encodeURIComponent(id)}/${command}`,
      body,
    );
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<T> {
    const response = await fetch(this.url(path), {
      method,
      headers: this.headers(body !== undefined),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!response.ok) throw await this.failure(response);
    return (await response.json()) as T;
  }

  private headers(json = false): HeadersInit {
    return {
      Authorization: `Bearer ${this.token}`,
      ...(json ? { "Content-Type": "application/json" } : {}),
    };
  }

  private url(path: string): string {
    return `${this.baseUrl.replace(/\/+$/, "")}${path}`;
  }

  private async failure(response: Response): Promise<PurrCodeError> {
    let payload: unknown;
    try {
      payload = await response.json();
    } catch {
      payload = { error: "non-JSON daemon response" };
    }
    return new PurrCodeError(
      `PurrCode daemon returned HTTP ${response.status}`,
      response.status,
      payload,
    );
  }
}
