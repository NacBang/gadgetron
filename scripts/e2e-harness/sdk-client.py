#!/usr/bin/env python3
"""E2E scenario: OpenAI Python SDK client talks to Gadgetron.

Proves the OpenAI-compat wire contract holds for a REAL canonical
client, not just raw curl+jq. A regression that subtly flips field
casing, content-type, SSE frame format — or anything else OpenAI
SDK parsers assume — would pass every handcrafted curl gate and
still break every real client in the ecosystem. The SDK is the
only way to assert "if a vanilla OpenAI client points at us, it
works."

Usage:
    GADGETRON_BASE=http://127.0.0.1:19090 \
    GADGETRON_KEY=gad_live_... \
        python3 sdk-client.py

Exits 0 on end-to-end success, non-zero otherwise.
"""

from __future__ import annotations

import os
import sys

try:
    from openai import OpenAI
except ImportError:
    print("openai SDK not installed — install with `pip install openai`", file=sys.stderr)
    sys.exit(2)

BASE = os.environ.get("GADGETRON_BASE", "http://127.0.0.1:19090")
KEY = os.environ.get("GADGETRON_KEY")
MODEL = os.environ.get("GADGETRON_MODEL", "mock")

if not KEY:
    print("GADGETRON_KEY env var is required", file=sys.stderr)
    sys.exit(2)

client = OpenAI(base_url=f"{BASE}/v1", api_key=KEY)

# ---------------------------------------------------------------------------
# Scenario 1 — non-streaming chat completion (round-trip the mock content)
# ---------------------------------------------------------------------------

resp = client.chat.completions.create(
    model=MODEL,
    messages=[{"role": "user", "content": "ping via sdk"}],
    stream=False,
)
content = resp.choices[0].message.content
if not content or "Hello from mock provider" not in content:
    print(f"SDK non-streaming: unexpected content {content!r}", file=sys.stderr)
    sys.exit(1)
if not resp.id.startswith("chatcmpl-"):
    print(f"SDK non-streaming: id contract broken: {resp.id!r}", file=sys.stderr)
    sys.exit(1)
if resp.usage.prompt_tokens != 5 or resp.usage.completion_tokens != 7:
    print(
        f"SDK non-streaming: usage mismatch {resp.usage!r}",
        file=sys.stderr,
    )
    sys.exit(1)

# ---------------------------------------------------------------------------
# Scenario 2 — streaming chat (iterate chunks, accumulate content)
# ---------------------------------------------------------------------------

accum = ""
chunk_ids: set[str] = set()
finished = False
stream = client.chat.completions.create(
    model=MODEL,
    messages=[{"role": "user", "content": "ping via sdk stream"}],
    stream=True,
)
for chunk in stream:
    chunk_ids.add(chunk.id)
    delta = chunk.choices[0].delta if chunk.choices else None
    if delta is not None and delta.content:
        accum += delta.content
    if chunk.choices and chunk.choices[0].finish_reason:
        finished = True

if not finished:
    print("SDK streaming: no finish_reason observed", file=sys.stderr)
    sys.exit(1)
if len(chunk_ids) != 1:
    print(
        f"SDK streaming: chunks must share one id, got {chunk_ids}",
        file=sys.stderr,
    )
    sys.exit(1)
if not accum:
    print("SDK streaming: no content accumulated", file=sys.stderr)
    sys.exit(1)

print(
    f"SDK e2e OK: non-stream content={content!r} "
    f"stream-accum={accum!r} chunk-id={next(iter(chunk_ids))}"
)
