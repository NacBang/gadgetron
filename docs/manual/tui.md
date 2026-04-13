# TUI Dashboard

The `gadgetron-tui` crate provides a terminal-based dashboard for monitoring your Gadgetron cluster. It renders GPU node metrics, model states, and a live request log in a 3-column layout.

As of Sprint 6, the TUI is connected to the live gateway via a `tokio::sync::broadcast` channel. Real-time GPU metrics, model states, and request log entries stream into the dashboard at 1 Hz.

---

## Running the TUI

The TUI is embedded in the main `gadgetron` binary. Start it together with the gateway server using the `--tui` flag:

```sh
gadgetron serve --tui
```

With a custom config file and bind address:

```sh
gadgetron serve --tui --config /etc/gadgetron/gadgetron.toml --bind 0.0.0.0:8080
```

When `--tui` is set, the gateway starts normally and the TUI dashboard opens in the same terminal. Incoming requests are reflected in the Requests panel in real time as they pass through the gateway. The dashboard updates at 1 Hz.

To exit, press `q` or `Esc`. The server shuts down gracefully (5-second audit drain) before the process exits.

### TTY requirement

`--tui` requires an interactive terminal — both stdin and stdout must be attached to a TTY. If you run `gadgetron serve --tui` from a non-interactive context (systemd, CI, SSH with `-T`, IDE task runners, pipe redirects), the server refuses to start and exits with code `2`:

```
Error: --tui requires an interactive terminal (stdin or stdout is not a TTY).

  Cause: stdin/stdout is not connected to a terminal — this happens under systemd,
         CI runners, SSH with -T, IDE task runners, and pipe redirects.

  Next steps:
    1. Run gadgetron from a regular shell (iTerm, Terminal.app, Alacritty, ...)
    2. Remove --tui to run headless — the server is reachable at GET /health
       and GET /v1/models once started.
    3. For systemd/CI: omit --tui or set tui = false in gadgetron.toml.
       See docs/manual/configuration.md for the full option reference.
```

This is intentional: the TUI uses crossterm raw-mode and alternate-screen escapes that only work on a real terminal. Running headless is always available — drop `--tui` to get a plain server log.

---

## Screen layout

The terminal is divided into four zones stacked vertically:

```
┌─────────────────────────────────────────────────────────────┐
│ Header bar (cluster summary, 3 rows)                        │
├────────────────────┬────────────────────┬───────────────────┤
│  Nodes             │  Models            │  Requests         │
│  (33%)             │  (33%)             │  (34%)            │
│                    │                    │                   │
│  One row per GPU   │  One row per       │  One row per      │
│  across all nodes  │  known model       │  request, newest  │
│                    │                    │  at top, max 50   │
│                    │                    │  visible          │
├────────────────────┴────────────────────┴───────────────────┤
│ Footer: key binding hints (3 rows)                          │
└─────────────────────────────────────────────────────────────┘
```

### Header bar

Displays a one-line cluster summary:

```
Gadgetron Dashboard  Nodes: 2/2 | GPUs: 3/3 | Models: 2 | RPS: 14.3 | Err: 2.1%
```

Fields:

| Field | Meaning |
|-------|---------|
| `Nodes: healthy/total` | How many nodes are healthy out of total registered |
| `GPUs: active/total` | Active (non-idle) GPUs out of total |
| `Models` | Number of models in the `running` state |
| `RPS` | Requests per second (cluster-wide) |
| `Err` | Error rate as a percentage of requests |

These values are updated from the live gateway via a broadcast channel at 1 Hz.

### Nodes column

One row per GPU device across all connected nodes. Example rows:

```
[node-1] GPU0 68% VRAM:28000/40960MB 52C
[node-1] GPU1 92% VRAM:38000/40960MB 78C
[node-2] GPU0 30% VRAM:12000/81920MB 63C
```

Row format: `[node-id] GPUn utilization% VRAM:used_mb/total_mb temp_C`

Row color is determined by temperature. When VRAM utilization is critical (>=90%), the color overrides to red regardless of temperature.

### Models column

One row per model known to the cluster. Example rows:

```
[running] meta-llama/Llama-3-8B-Instruct ollama
[loading] mistralai/Mistral-7B-v0.3 vllm
[running] gpt-4o openai
```

Row format: `[state] model_id provider`

Model states: `running`, `loading`, `stopped`, `error`, `draining`.

### Requests column

Recent requests, newest first. Up to 50 rows are displayed; up to 100 entries are retained in memory. Example rows:

```
req-a1b2 llama3 312ms HTTP200
req-e5f6 gpt-4o 891ms HTTP200
req-c9d0 mistral-7b 50ms HTTP503
```

Row format: `request_id_prefix model latency_ms HTTPstatus`

The request ID is truncated to 8 characters for display.

### Footer

```
 q: quit | r: refresh | arrows: navigate
```

---

## Key bindings

| Key | Action | Status |
|-----|--------|--------|
| `q` | Quit the TUI (triggers graceful shutdown) | Implemented |
| `Esc` | Quit the TUI (triggers graceful shutdown) | Implemented |
| `r` | Manual refresh trigger | Sprint 7 |
| Arrow keys | Navigate between panels / scroll | Sprint 7 |

Arrow navigation and manual refresh are shown in the footer as a preview of future functionality. Pressing them in Sprint 6 has no effect.

---

## Color scheme

### Temperature (Nodes column, primary signal)

| Temperature | Color |
|-------------|-------|
| Below 60 C | Green |
| 60 to 74 C | Yellow |
| 75 to 84 C | Red |
| 85 C and above | Light Red |

### VRAM utilization (Nodes column, override)

When VRAM utilization is 90% or higher, the row color overrides to Red regardless of temperature. This signals imminent out-of-memory risk.

| VRAM utilization | Color |
|-----------------|-------|
| Below 70% | Green |
| 70% to 89% | Yellow |
| 90% and above | Red (overrides temperature) |

### Panel border colors

| Panel | Border color |
|-------|-------------|
| Nodes | Green |
| Models | Yellow |
| Requests | Blue |
| Header text | Cyan (bold) |
| Footer text | Dark Gray |

---

## Sprint 6 status and Sprint 7 roadmap

Sprint 6 delivered live gateway integration. What is now working:

- **Live data.** `App::with_channel(rx)` receives `GpuMetrics`, `ModelStatus`, `ClusterHealth`, and `RequestEntry` updates from the gateway via `tokio::sync::broadcast` at 1 Hz.
- **Real-time request log.** Every request processed by the gateway is forwarded through `metrics_middleware` and appears in the Requests panel as it completes.
- **Graceful shutdown.** Pressing `q` or `Esc` initiates a 5-second audit drain before the process exits.

What is not yet wired (Sprint 7):

- **No keyboard navigation.** Arrow keys and `r` are listed in the footer but not handled.
- **No scrolling.** The Nodes and Models columns render all entries; there is no scroll position tracking.
- **No time display.** The header does not show a clock or last-updated timestamp.

---

## Architecture note (for contributors)

The TUI crate is `gadgetron-tui`. Data types are defined in `gadgetron-core/src/ui.rs` and shared with the future `gadgetron-web` [P2] crate. The TUI never imports directly from `gadgetron-gateway`; it receives data through `WsMessage` variants over a broadcast channel.

Relevant files:

- `/Users/junghopark/dev/gadgetron/crates/gadgetron-tui/src/main.rs` — binary entry point
- `/Users/junghopark/dev/gadgetron/crates/gadgetron-tui/src/app.rs` — `App` struct, event loop, channel drain logic
- `/Users/junghopark/dev/gadgetron/crates/gadgetron-tui/src/ui.rs` — layout, panel renderers, color helpers
- `/Users/junghopark/dev/gadgetron/crates/gadgetron-core/src/ui.rs` — shared data types (`GpuMetrics`, `ModelStatus`, `RequestEntry`, `ClusterHealth`, `WsMessage`)
