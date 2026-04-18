#!/usr/bin/env python3
"""Minimal OpenAI-compatible mock provider for the Gadgetron E2E harness.

Intentionally uses ONLY the Python standard library so the harness has zero
pip/bun dependencies. Launches quickly, cleans up on SIGTERM, and writes a
JSONL request log the harness can grep.

## Endpoints

- `GET /health` → `{"ok": true}` (200). Used for readiness polling.
- `GET /v1/models` → 200 with one fake model so `gadgetron`'s startup-time
  model listing doesn't blow up.
- `POST /v1/chat/completions` (non-streaming, `stream: false`) → 200 JSON
  ChatCompletion with deterministic content (`"Hello from mock provider."`)
  and usage (`prompt_tokens=5, completion_tokens=7`).
- `POST /v1/chat/completions` (streaming, `stream: true`) → SSE stream
  that emits 3 content deltas then `data: [DONE]`. When `MOCK_ERROR_MODE=stream_fail`
  is set, the server closes the connection abruptly after the 2nd chunk so
  the Gadgetron SSE pipeline surfaces a terminal error — exercises the
  drift-fix PR 6 Drop-guard `StreamInterrupted` path.

## Request log

Every POST body is appended to `$MOCK_LOG` (default: `/tmp/mock-openai.log`)
as a single-line JSON object with `ts`, `path`, and `body` keys. The harness
uses this to verify `<gadgetron_shared_context>` is injected into the
messages the provider actually received (PSL-1b end-to-end).

## Env vars

- `MOCK_PORT` (default `19999`) — listen port
- `MOCK_LOG` (default `/tmp/mock-openai.log`) — JSONL request log
- `MOCK_ERROR_MODE` (default unset) — `stream_fail` to simulate mid-stream error
"""

from __future__ import annotations

import json
import os
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from socketserver import ThreadingMixIn


PORT = int(os.environ.get("MOCK_PORT", "19999"))
LOG_PATH = os.environ.get("MOCK_LOG", "/tmp/mock-openai.log")
ERROR_MODE = os.environ.get("MOCK_ERROR_MODE", "")

CANNED_CONTENT = "Hello from mock provider."
CANNED_PROMPT_TOKENS = 5
CANNED_COMPLETION_TOKENS = 7


def log_request(path: str, body_raw: bytes) -> None:
    try:
        body = json.loads(body_raw.decode("utf-8")) if body_raw else None
    except Exception:
        body = {"_parse_error": True, "raw_bytes": len(body_raw)}
    entry = {"ts": time.time(), "path": path, "body": body}
    try:
        with open(LOG_PATH, "a") as f:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")
    except Exception as e:
        sys.stderr.write(f"mock-openai: failed to write log: {e}\n")


def chat_completion_response(model: str) -> dict:
    return {
        "id": "chatcmpl-mock-0001",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": CANNED_CONTENT},
                "finish_reason": "stop",
            }
        ],
        "usage": {
            "prompt_tokens": CANNED_PROMPT_TOKENS,
            "completion_tokens": CANNED_COMPLETION_TOKENS,
            "total_tokens": CANNED_PROMPT_TOKENS + CANNED_COMPLETION_TOKENS,
        },
    }


def stream_chunk(model: str, content_piece: str | None, finish_reason: str | None) -> str:
    delta: dict = {}
    if content_piece is not None:
        delta["content"] = content_piece
    chunk = {
        "id": "chatcmpl-mock-stream-0001",
        "object": "chat.completion.chunk",
        "created": 1_700_000_000,
        "model": model,
        "choices": [
            {"index": 0, "delta": delta, "finish_reason": finish_reason}
        ],
    }
    return f"data: {json.dumps(chunk)}\n\n"


class MockHandler(BaseHTTPRequestHandler):
    # Silence default access log noise — the JSONL log is the source of truth.
    def log_message(self, fmt, *args):  # noqa: A003
        pass

    def _read_body(self) -> bytes:
        length = int(self.headers.get("Content-Length", "0") or "0")
        return self.rfile.read(length) if length > 0 else b""

    def _send_json(self, status: int, payload: dict) -> None:
        body = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802
        if self.path == "/health":
            self._send_json(200, {"ok": True})
        elif self.path == "/v1/models":
            self._send_json(
                200,
                {
                    "object": "list",
                    "data": [
                        {"id": "mock-model", "object": "model", "owned_by": "mock"}
                    ],
                },
            )
        else:
            self._send_json(404, {"error": {"message": "not found", "type": "invalid_request_error"}})

    def do_POST(self):  # noqa: N802
        body_raw = self._read_body()
        log_request(self.path, body_raw)

        if self.path != "/v1/chat/completions":
            self._send_json(404, {"error": {"message": "not found"}})
            return

        try:
            body = json.loads(body_raw.decode("utf-8")) if body_raw else {}
        except Exception:
            self._send_json(400, {"error": {"message": "invalid json"}})
            return

        model = body.get("model", "mock-model")
        streaming = bool(body.get("stream", False))

        if not streaming:
            self._send_json(200, chat_completion_response(model))
            return

        # Streaming path — SSE.
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.end_headers()

        pieces = ["Hello ", "from ", "mock."]

        try:
            for idx, piece in enumerate(pieces):
                if ERROR_MODE == "stream_fail" and idx == 2:
                    # Close the connection abruptly without sending [DONE]
                    # so the Gadgetron SSE pipeline surfaces a terminal
                    # error. Exercises drift-fix PR 6 StreamInterrupted.
                    return
                self.wfile.write(stream_chunk(model, piece, None).encode("utf-8"))
                self.wfile.flush()

            # Final chunk with finish_reason.
            self.wfile.write(stream_chunk(model, None, "stop").encode("utf-8"))
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            # Client (gadgetron) closed the connection — OK.
            pass


class ThreadingServer(ThreadingMixIn, HTTPServer):
    """Allow concurrent streaming + non-streaming requests without blocking."""

    daemon_threads = True
    allow_reuse_address = True


def main() -> int:
    # Reset the log file on each start so the harness gets a clean slate.
    try:
        with open(LOG_PATH, "w") as f:
            f.write("")
    except Exception:
        pass

    server = ThreadingServer(("127.0.0.1", PORT), MockHandler)

    def handle_signal(_sig, _frame):
        server.shutdown()

    signal.signal(signal.SIGTERM, handle_signal)
    signal.signal(signal.SIGINT, handle_signal)

    sys.stderr.write(
        f"mock-openai: listening on 127.0.0.1:{PORT}, logging to {LOG_PATH}, "
        f"error_mode={ERROR_MODE!r}\n"
    )
    sys.stderr.flush()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
