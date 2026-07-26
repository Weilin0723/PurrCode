"""Standard-library client for the authenticated PurrCode daemon."""

import json
from typing import Any, Dict, Iterator, List, Optional
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen


class PurrCodeError(RuntimeError):
    def __init__(self, message: str, status: Optional[int] = None, response: Any = None):
        super().__init__(message)
        self.status = status
        self.response = response


class PurrCodeClient:
    def __init__(self, base_url: str, token: str, timeout_seconds: float = 30.0):
        if not base_url.startswith(("http://", "https://")) or len(token) < 32:
            raise PurrCodeError("invalid daemon URL or bearer token")
        self._base_url = base_url.rstrip("/")
        self._token = token
        self._timeout = timeout_seconds

    def start(self, objective: str, repository: str) -> Dict[str, Any]:
        return self._request(
            "POST", "/v1/sessions", {"objective": objective, "repository": repository}
        )

    def plan(self, objective: str, repository: str) -> Dict[str, Any]:
        return self._request(
            "POST",
            "/v1/sessions",
            {"objective": objective, "repository": repository, "plan_only": True},
        )

    def sessions(self) -> List[Dict[str, Any]]:
        return self._request("GET", "/v1/sessions")

    def session(self, session_id: str) -> Dict[str, Any]:
        return self._request("GET", "/v1/sessions/{}".format(quote(session_id)))

    def events(self, session_id: str) -> List[Dict[str, Any]]:
        return self._request(
            "GET", "/v1/sessions/{}/events".format(quote(session_id))
        )

    def resume(self, session_id: str) -> Dict[str, Any]:
        return self._command(session_id, "resume", {})

    def approve(self, session_id: str) -> Dict[str, Any]:
        return self._command(session_id, "approve", {})

    def reject(
        self, session_id: str, reason: str = "rejected by user"
    ) -> Dict[str, Any]:
        return self._command(session_id, "reject", {"reason": reason})

    def cancel(
        self, session_id: str, reason: str = "cancelled by user"
    ) -> Dict[str, Any]:
        return self._command(session_id, "cancel", {"reason": reason})

    def pause(
        self, session_id: str, reason: str = "paused by user"
    ) -> Dict[str, Any]:
        return self._command(session_id, "pause", {"reason": reason})

    def checkpoint(
        self, session_id: str, label: str = "manual"
    ) -> Dict[str, Any]:
        return self._command(session_id, "checkpoint", {"label": label})

    def rollback(self, session_id: str) -> Dict[str, Any]:
        return self._command(session_id, "rollback", {})

    def compact(self, session_id: str) -> Dict[str, Any]:
        return self._command(session_id, "compact", {})

    def select_model(self, session_id: str, model: str) -> Dict[str, Any]:
        return self._command(session_id, "model", {"model": model})

    def replace_action(
        self,
        session_id: str,
        action: Dict[str, Any],
        reason: str = "edited by user",
    ) -> Dict[str, Any]:
        return self._command(
            session_id, "replace-action", {"action": action, "reason": reason}
        )

    def review_hunks(self, session_id: str) -> Dict[str, Any]:
        return self._request(
            "GET", "/v1/sessions/{}/hunks".format(quote(session_id))
        )

    def apply_hunk(
        self, session_id: str, index: int, patch_digest: str
    ) -> Dict[str, Any]:
        return self._request(
            "POST",
            "/v1/sessions/{}/hunks/apply".format(quote(session_id)),
            {"index": index, "patch_digest": patch_digest},
        )

    def reject_hunk(
        self, session_id: str, index: int, patch_digest: str
    ) -> Dict[str, Any]:
        return self._request(
            "POST",
            "/v1/sessions/{}/hunks/reject".format(quote(session_id)),
            {"index": index, "patch_digest": patch_digest},
        )

    def automations(self) -> List[Dict[str, Any]]:
        return self._request("GET", "/v1/automations")

    def create_automation(
        self, objective: str, repository: str, interval_seconds: int
    ) -> Dict[str, Any]:
        return self._request(
            "POST",
            "/v1/automations",
            {
                "objective": objective,
                "repository": repository,
                "interval_seconds": interval_seconds,
            },
        )

    def set_automation_enabled(
        self, automation_id: str, enabled: bool
    ) -> Dict[str, Any]:
        action = "enable" if enabled else "disable"
        return self._request(
            "POST",
            "/v1/automations/{}/{}".format(quote(automation_id), action),
            {},
        )

    def run_automation(self, automation_id: str) -> Dict[str, Any]:
        return self._request(
            "POST", "/v1/automations/{}/run".format(quote(automation_id)), {}
        )

    def parallel(
        self,
        objective: str,
        repository: str,
        workers: List[Dict[str, Any]],
    ) -> Dict[str, Any]:
        return self._request(
            "POST",
            "/v1/supervisor",
            {
                "objective": objective,
                "repository": repository,
                "workers": workers,
                "limits": {
                    "max_workers": 3,
                    "max_model_requests": 6,
                    "max_worktrees": 4,
                    "require_isolation": True,
                },
            },
        )

    def stream_events(self, session_id: str) -> Iterator[Dict[str, Any]]:
        request = Request(
            self._url(
                "/v1/sessions/{}/events/stream".format(quote(session_id))
            ),
            headers=self._headers(),
        )
        try:
            with urlopen(request, timeout=self._timeout) as response:
                data_lines = []
                for raw_line in response:
                    line = raw_line.decode("utf-8").rstrip("\r\n")
                    if not line:
                        if data_lines:
                            yield json.loads("\n".join(data_lines))
                            data_lines = []
                    elif line.startswith("data:"):
                        data_lines.append(line[5:].lstrip())
        except HTTPError as error:
            self._raise_http(error)
        except URLError as error:
            raise PurrCodeError("could not connect to PurrCode daemon") from error

    def _command(
        self, session_id: str, command: str, body: Dict[str, Any]
    ) -> Dict[str, Any]:
        return self._request(
            "POST",
            "/v1/sessions/{}/{}".format(quote(session_id), command),
            body,
        )

    def _request(self, method: str, path: str, body: Any = None) -> Any:
        encoded = None if body is None else json.dumps(body).encode("utf-8")
        headers = self._headers()
        if encoded is not None:
            headers["Content-Type"] = "application/json"
        request = Request(self._url(path), data=encoded, method=method, headers=headers)
        try:
            with urlopen(request, timeout=self._timeout) as response:
                return json.load(response)
        except HTTPError as error:
            self._raise_http(error)
        except URLError as error:
            raise PurrCodeError("could not connect to PurrCode daemon") from error

    def _headers(self) -> Dict[str, str]:
        return {"Authorization": "Bearer {}".format(self._token)}

    def _url(self, path: str) -> str:
        return "{}{}".format(self._base_url, path)

    @staticmethod
    def _raise_http(error: HTTPError) -> None:
        try:
            payload = json.load(error)
        except (ValueError, UnicodeDecodeError):
            payload = {"error": "non-JSON daemon response"}
        raise PurrCodeError(
            "PurrCode daemon returned HTTP {}".format(error.code),
            error.code,
            payload,
        ) from error
