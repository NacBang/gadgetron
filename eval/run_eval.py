#!/usr/bin/env python3
"""Kairos evaluation harness.

Runs a scenario list against a live `gadgetron serve` instance:

    GADGETRON_API_KEY=gad_live_... python3 eval/run_eval.py [--server URL] [--scenario ID]

For each scenario it:
    1. POSTs the prompt to `/v1/chat/completions` with `stream: true`.
    2. Parses the OpenAI SSE stream into (text, tool_use_names, finish_reason).
    3. Checks assertions from `scenarios.yaml`.
    4. Optionally verifies that a wiki page was created on disk + contains
       expected strings.
    5. Writes a markdown report under `eval/reports/`.

Scenario-level `expected_status: failing` flips the assertion polarity — the
scenario passes when it keeps failing, and regresses (UNEXPECTED_PASS) when it
starts succeeding. Use this to pin known gaps without hiding them.

No network deps beyond `requests` + `pyyaml`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import pathlib
import re
import sys
import time
from dataclasses import dataclass, field
from typing import Any

import requests
import yaml

EVAL_ROOT = pathlib.Path(__file__).resolve().parent
SCENARIOS_FILE = EVAL_ROOT / "scenarios.yaml"
REPORTS_DIR = EVAL_ROOT / "reports"

# Matches the decorated tool_use lines Kairos emits into stream content, e.g.
#   🔧 **mcp__knowledge__wiki_write** `{...}`
TOOL_USE_RE = re.compile(r"\*\*(mcp__[A-Za-z0-9_]+|WebSearch|WebFetch)\*\*")


@dataclass
class StreamResult:
    text: str = ""
    tool_calls: list[str] = field(default_factory=list)
    finish_reason: str | None = None
    error: str | None = None
    wall_seconds: float = 0.0


@dataclass
class ScenarioResult:
    scenario_id: str
    description: str
    expected_status: str
    assertions: list[tuple[str, bool, str]] = field(default_factory=list)
    stream: StreamResult | None = None

    @property
    def all_assertions_pass(self) -> bool:
        return all(passed for _, passed, _ in self.assertions)

    @property
    def outcome(self) -> str:
        """pass / fail / unexpected_pass / expected_fail."""
        passed = self.all_assertions_pass
        if self.expected_status == "failing":
            return "unexpected_pass" if passed else "expected_fail"
        return "pass" if passed else "fail"


def load_scenarios() -> list[dict]:
    with open(SCENARIOS_FILE) as f:
        return yaml.safe_load(f)


def stream_chat(server: str, key: str, prompt: str, timeout_s: int) -> StreamResult:
    url = server.rstrip("/") + "/v1/chat/completions"
    payload = {
        "model": "kairos",
        "stream": True,
        "messages": [{"role": "user", "content": prompt}],
    }
    headers = {"Authorization": f"Bearer {key}", "Content-Type": "application/json"}

    result = StreamResult()
    t0 = time.time()
    try:
        with requests.post(
            url, json=payload, headers=headers, stream=True, timeout=timeout_s
        ) as r:
            r.raise_for_status()
            for raw in r.iter_lines(decode_unicode=True):
                if not raw or not raw.startswith("data: "):
                    continue
                body = raw[6:]
                if body.strip() == "[DONE]":
                    break
                try:
                    evt = json.loads(body)
                except json.JSONDecodeError:
                    continue
                choice = (evt.get("choices") or [{}])[0]
                delta = choice.get("delta") or {}
                content = delta.get("content") or ""
                if content:
                    result.text += content
                    for m in TOOL_USE_RE.finditer(content):
                        result.tool_calls.append(m.group(1))
                fr = choice.get("finish_reason")
                if fr:
                    result.finish_reason = fr
    except requests.RequestException as e:
        result.error = f"{type(e).__name__}: {e}"
    result.wall_seconds = round(time.time() - t0, 2)
    return result


def resolve_wiki_root() -> pathlib.Path | None:
    """Read gadgetron.toml for `[knowledge].wiki_path`, fall back to default."""
    toml = pathlib.Path(__file__).resolve().parents[1] / "gadgetron.toml"
    if not toml.exists():
        return None
    for line in toml.read_text().splitlines():
        m = re.match(r'\s*wiki_path\s*=\s*"([^"]+)"', line)
        if m:
            return pathlib.Path(m.group(1)).expanduser()
    return None


def check_assertions(
    scenario: dict, stream: StreamResult, wiki_root: pathlib.Path | None
) -> list[tuple[str, bool, str]]:
    assertions: list[tuple[str, bool, str]] = []
    expect = scenario.get("expect") or {}

    if stream.error:
        assertions.append(("stream.ok", False, f"transport error: {stream.error}"))
        return assertions
    assertions.append(("stream.ok", True, f"streamed {stream.wall_seconds}s"))

    want_finish = expect.get("finish_reason")
    if want_finish:
        got = stream.finish_reason or "<none>"
        assertions.append((f"finish_reason == {want_finish}", got == want_finish, got))

    for tool in expect.get("tool_calls_contain", []) or []:
        present = tool in stream.tool_calls
        assertions.append((f"tool_calls ⊇ {tool}", present, str(stream.tool_calls)))

    for substr in expect.get("text_contains_any", []) or []:
        if substr in stream.text:
            assertions.append((f"text contains any ⊇ {substr!r}", True, "first match"))
            break
    else:
        if expect.get("text_contains_any"):
            assertions.append(
                (
                    f"text contains any {expect['text_contains_any']}",
                    False,
                    "none found",
                )
            )

    page_path = expect.get("page_path")
    if page_path and wiki_root:
        full = wiki_root / page_path
        exists = full.is_file()
        assertions.append((f"page exists: {page_path}", exists, str(full)))
        if exists:
            body = full.read_text(errors="replace")
            for must in expect.get("page_contains", []) or []:
                assertions.append(
                    (f"page contains {must!r}", must in body, f"{len(body)}B page")
                )
            anys = expect.get("page_contains_any", []) or []
            if anys:
                ok = any(s in body for s in anys)
                assertions.append(
                    (f"page contains any {anys}", ok, "match" if ok else "no match")
                )

    absent = expect.get("page_path_absent")
    if absent and wiki_root:
        full = wiki_root / absent
        ok = not full.exists()
        assertions.append((f"page absent: {absent}", ok, str(full)))

    return assertions


def run_scenario(scenario: dict, server: str, key: str, wiki_root) -> ScenarioResult:
    res = ScenarioResult(
        scenario_id=scenario["id"],
        description=scenario.get("description", ""),
        expected_status=scenario.get("expected_status", "passing"),
    )
    stream = stream_chat(
        server, key, scenario["prompt"], scenario.get("timeout_s", 120)
    )
    res.stream = stream
    res.assertions = check_assertions(scenario, stream, wiki_root)
    return res


OUTCOME_BADGES = {
    "pass": "[PASS]",
    "fail": "[FAIL]",
    "expected_fail": "[XFAIL]",
    "unexpected_pass": "[XPASS]",
}


def format_report(results: list[ScenarioResult], server: str) -> str:
    now = dt.datetime.now().isoformat(timespec="seconds")
    lines = [
        f"# Kairos eval report — {now}",
        "",
        f"Server: `{server}`",
        "",
        "| Scenario | Outcome | Wall | Tools | Finish |",
        "| --- | --- | --- | --- | --- |",
    ]
    for r in results:
        badge = OUTCOME_BADGES[r.outcome]
        wall = f"{r.stream.wall_seconds}s" if r.stream else "—"
        tools = ", ".join(r.stream.tool_calls) if r.stream else "—"
        fr = r.stream.finish_reason if r.stream else "—"
        lines.append(f"| `{r.scenario_id}` | {badge} | {wall} | {tools or '—'} | {fr or '—'} |")
    lines.append("")

    for r in results:
        badge = OUTCOME_BADGES[r.outcome]
        lines.append(f"## {badge} `{r.scenario_id}`")
        lines.append("")
        lines.append(r.description or "")
        lines.append("")
        lines.append(f"- expected_status: `{r.expected_status}`")
        if r.stream:
            lines.append(f"- wall_seconds: `{r.stream.wall_seconds}`")
            lines.append(f"- finish_reason: `{r.stream.finish_reason}`")
            lines.append(f"- tool_calls: `{r.stream.tool_calls}`")
            if r.stream.error:
                lines.append(f"- transport_error: `{r.stream.error}`")
        lines.append("")
        lines.append("### Assertions")
        lines.append("")
        lines.append("| Check | Result | Detail |")
        lines.append("| --- | --- | --- |")
        for name, passed, detail in r.assertions:
            symbol = "OK" if passed else "X"
            lines.append(f"| `{name}` | {symbol} | {detail} |")
        lines.append("")
        if r.stream:
            tail = r.stream.text[-600:] if r.stream.text else "(empty)"
            lines.append("### Last 600 chars of assistant text")
            lines.append("")
            lines.append("```")
            lines.append(tail)
            lines.append("```")
            lines.append("")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description="Run Kairos eval scenarios.")
    ap.add_argument("--server", default="http://127.0.0.1:8080")
    ap.add_argument("--scenario", help="Run only this scenario id")
    ap.add_argument(
        "--no-report",
        action="store_true",
        help="Skip writing a markdown report under eval/reports/.",
    )
    args = ap.parse_args()

    key = os.environ.get("GADGETRON_API_KEY")
    if not key:
        print("GADGETRON_API_KEY is required", file=sys.stderr)
        return 2

    scenarios = load_scenarios()
    if args.scenario:
        scenarios = [s for s in scenarios if s["id"] == args.scenario]
        if not scenarios:
            print(f"scenario not found: {args.scenario}", file=sys.stderr)
            return 2

    wiki_root = resolve_wiki_root()
    if wiki_root:
        print(f"wiki_root: {wiki_root}")
    else:
        print("warning: could not resolve wiki_path from gadgetron.toml")

    results: list[ScenarioResult] = []
    for s in scenarios:
        print(f"\n▶ {s['id']}")
        r = run_scenario(s, args.server, key, wiki_root)
        results.append(r)
        for name, passed, detail in r.assertions:
            print(f"    {'OK' if passed else 'X '} {name}  — {detail}")
        print(f"    → {r.outcome.upper()}  ({r.stream.wall_seconds if r.stream else '—'}s)")

    report = format_report(results, args.server)
    if not args.no_report:
        REPORTS_DIR.mkdir(parents=True, exist_ok=True)
        ts = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
        path = REPORTS_DIR / f"{ts}.md"
        path.write_text(report)
        print(f"\nreport: {path}")

    regressions = [r for r in results if r.outcome in ("fail", "unexpected_pass")]
    return 1 if regressions else 0


if __name__ == "__main__":
    sys.exit(main())
