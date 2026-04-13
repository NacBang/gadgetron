# Hotfix: `--tui` TTY pre-check and visible failure

> **Status**: Design note
> **Scope**: 1 small UX fix (no new types, no API changes)
> **Source**: Manual testing (Test 7 — TUI real-time dashboard) on 2026-04-13

## Context
Test 7 launched `gadgetron serve --tui --no-db --provider ...` from a non-interactive shell. Current behaviour:

```
...
  Checking provider(s)... done (1 configured)
  Starting server...WARN gadgetron: TUI exited with error error=Device not configured (os error 6)
 done
listening addr=127.0.0.1:8081

Gadgetron v0.1.0
  Listen: 127.0.0.1:8081
  Providers: vllm @ http://10.100.1.5:8100
```

The TUI thread attempts `terminal::enable_raw_mode()` on a non-TTY stdin, receives `ENXIO` (`Device not configured`), logs one `tracing::warn!`, and exits. The server keeps running headless. Two problems:

1. **User expects a TUI**. They typed `--tui`. They get a headless server and no prominent error — just a `WARN` line buried in info-level startup logs that most users filter or ignore.
2. **Obscure error text**. `Device not configured (os error 6)` is a raw POSIX errno string. Nothing in it tells the user "stdin is not a TTY — run from a real terminal".

Even in a real TTY the current code works fine (this is why unit tests stay green — they don't touch crossterm). The bug only surfaces in non-interactive environments: SSH with `-T`, `systemd`, CI, `tmux send-keys`, IDE "run without terminal", VS Code task runners.

## Fix

Add a TTY pre-check at the CLI layer, **before** spawning the TUI thread. If `--tui` was requested but stdin is not a TTY, fail fast with a clear error, don't start the server at all.

Rationale for fail-fast over "silently disable TUI": the user asked for an interactive dashboard. Silently degrading to headless mode would surprise them later when they wonder why no dashboard appeared. Fail-fast at startup is the standard pattern (`git commit` without `--message` in a non-TTY fails; doesn't silently skip).

### Location
`crates/gadgetron-cli/src/main.rs` — inside `serve()`, right after `tui_enabled` is computed and BEFORE the broadcast channel is created.

### Code (sketch)

Address Round 1.5-A2 (check stdout too): crossterm writes alt-screen escapes to `io::stdout()`, so a redirected stdout will spray garbage into a logfile even when stdin is a TTY.

Address Round 1.5-A3 (exit code 2 for usage errors): `gadgetron doctor` already uses `std::process::exit(2)` per sysexits.h EX_USAGE. Return anyhow Err from `serve()` normally; main() already maps errors to exit(1). For this specific misuse case, print the error and `std::process::exit(2)` directly.

```rust
use std::io::IsTerminal;

// Step 7.5 (new): TTY pre-check when --tui requested.
// Extracted to require_tty_for_tui() so the pre-check logic is unit-testable
// without forking a process. The actual io calls happen at the call site.
if tui_enabled {
    let stdin_tty = std::io::stdin().is_terminal();
    let stdout_tty = std::io::stdout().is_terminal();
    if let Err(e) = require_tty_for_tui(tui_enabled, stdin_tty && stdout_tty) {
        eprintln!("Error: {e}");
        std::process::exit(2);
    }
}

/// Pure-function pre-check for `--tui` TTY requirement. Separated from
/// `std::io::stdin()/stdout()` calls so unit tests can exercise all four
/// (tui_enabled × has_tty) combinations without a real TTY.
fn require_tty_for_tui(tui_enabled: bool, has_tty: bool) -> anyhow::Result<()> {
    if !tui_enabled || has_tty {
        return Ok(());
    }
    anyhow::bail!(
        "--tui requires an interactive terminal (stdin or stdout is not a TTY).\n\
         \n\
           Cause: stdin/stdout is not connected to a terminal — this happens under systemd,\n\
                  CI runners, SSH with -T, IDE task runners, and pipe redirects.\n\
         \n\
           Next steps:\n\
             1. Run gadgetron from a regular shell (iTerm, Terminal.app, Alacritty, ...)\n\
             2. Remove --tui to run headless — the server is reachable at GET /health\n\
                and GET /v1/models once started.\n\
             3. For systemd/CI: omit --tui or set tui = false in gadgetron.toml.\n\
                See docs/manual/configuration.md for the full option reference."
    )
}
```

### Rust stdlib note
`std::io::IsTerminal` has been stable since Rust 1.70 (June 2023). No external dependency needed. Implementation uses `libc::isatty()` on Unix and `GetConsoleMode()` on Windows.

### Also: keep the existing `tracing::warn` for defence in depth
Even with the pre-check, `App::run()` can still fail mid-loop if the terminal is detached later (e.g., SSH session killed). The existing warn log stays as is.

## Acceptance

Non-TTY shell (this hotfix's target):
```
$ ./target/release/gadgetron serve --tui --no-db --provider http://10.100.1.5:8100 </dev/null
Error: --tui requires an interactive terminal (stdin or stdout is not a TTY).

  Cause: stdin/stdout is not connected to a terminal — this happens under systemd,
         CI runners, SSH with -T, IDE task runners, and pipe redirects.

  Next steps:
    1. Run gadgetron from a regular shell (iTerm, Terminal.app, Alacritty, ...)
    2. Remove --tui to run headless — the server is reachable at GET /health
       and GET /v1/models once started.
    3. For systemd/CI: omit --tui or set tui = false in gadgetron.toml.
       See docs/manual/configuration.md for the full option reference.

$ echo $?
2
```

Real TTY (unchanged):
```
$ ./target/release/gadgetron serve --tui --no-db --provider http://10.100.1.5:8100
[TUI dashboard appears and takes over the terminal]
```

Headless (unchanged):
```
$ ./target/release/gadgetron serve --no-db --provider http://10.100.1.5:8100
[Normal startup banner, server runs]
```

## Test plan

### Unit test (gadgetron-cli)
1. Add `tui_requires_tty_errors_when_stdin_not_a_terminal`: compose a minimal test that validates the pre-check logic. **Testability note**: We can't directly test `IsTerminal` behaviour without forking a process, but we can extract the check into a pure function `fn require_tty_for_tui(tui_enabled: bool, is_terminal: bool) -> anyhow::Result<()>` and unit-test it with all 4 combinations (true/false × true/false).

```rust
#[test]
fn require_tty_for_tui_ok_when_tui_disabled() {
    assert!(require_tty_for_tui(false, false).is_ok());
    assert!(require_tty_for_tui(false, true).is_ok());
}

#[test]
fn require_tty_for_tui_ok_when_tty_present() {
    assert!(require_tty_for_tui(true, true).is_ok());
}

#[test]
fn require_tty_for_tui_errors_when_tui_enabled_and_no_tty() {
    let err = require_tty_for_tui(true, false).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("--tui requires"));
    assert!(msg.contains("stdin is not a TTY"));
}
```

### Manual smoke
2. `./target/release/gadgetron serve --tui --no-db --provider http://127.0.0.1:9 </dev/null` → expect exit 1 and clear error
3. `./target/release/gadgetron serve --no-db --provider http://127.0.0.1:9 </dev/null` → expect normal startup (no TUI requested, no failure)
4. In a real iTerm session: `./target/release/gadgetron serve --tui --no-db --provider http://10.100.1.5:8100` → expect TUI to appear (manual verification — cannot be automated from this harness)

## Non-goals
- Auto-detect "headless mode" — OUT OF SCOPE. Fail-fast is the right default; we can add a `--tui-if-tty` flag later if a real user asks.
- Fix the underlying crossterm error message — OUT OF SCOPE. Our pre-check makes that path unreachable for well-formed calls, and mid-session terminal loss is already handled by the existing `tracing::warn`.
- Visual TUI verification — CANNOT BE AUTOMATED in this harness. Manual verification by the user in a real terminal is the only path. This doc explicitly limits scope to the non-TTY pre-check; visual correctness is covered by the 20 existing `gadgetron-tui` unit tests on the `App` state and render helpers.
