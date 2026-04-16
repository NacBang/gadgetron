//! MCP config JSON tempfile writer (M1).
//!
//! Spec: `docs/design/phase2/02-kairos-agent.md §7`.
//!
//! Every Claude Code subprocess invocation writes a fresh JSON tempfile
//! pointing at the `gadgetron mcp serve` stdio subcommand. Claude Code
//! reads the file via `--mcp-config <path>` at startup and launches the
//! stdio MCP server as its own child process. The tempfile is held by
//! the caller (`ClaudeCodeSession`) and removed on drop when the
//! subprocess exits.
//!
//! # Compile-time Unix gate
//!
//! `tempfile::NamedTempFile::with_prefix` internally calls `mkstemp(3)`,
//! a POSIX syscall that atomically creates the file with mode 0600 in
//! a single call. There is no race between creation and permission-set
//! — a fact locked in by the `tmpfile_has_0600_permissions` test.
//!
//! Non-Unix targets fail compilation with a clear message. P2A scope
//! is Linux/macOS only per 00-overview §3.

#[cfg(not(unix))]
compile_error!(
    "gadgetron-kairos requires a Unix target (uses mkstemp via the tempfile crate). \
     Windows / WASI support lands in Phase 2D per the P2A scope."
);

use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

/// Build the JSON document that Claude Code consumes via `--mcp-config`.
///
/// Uses `std::env::current_exe()` to resolve the absolute path of the
/// running `gadgetron` binary, so Claude Code's subprocess can find
/// `gadgetron mcp serve` even with the restricted SEC-B1 PATH.
///
/// When `config_path` is supplied, it is appended to the child's argv as
/// `--config <abs>`, so the `gadgetron mcp serve` grandchild finds the
/// `[knowledge]` / `[agent]` TOML regardless of its cwd (Claude Code pins
/// the child cwd to `~/.gadgetron/kairos/work/`, which never contains a
/// `gadgetron.toml`). Callers pass the same TOML path used by
/// `gadgetron serve`.
///
/// Lifted out of the tempfile writer so tests can round-trip it without
/// touching the filesystem.
pub fn build_config_json(config_path: Option<&Path>) -> serde_json::Value {
    let gadgetron_bin = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "gadgetron".to_string());
    let mut args: Vec<String> = vec!["mcp".to_string(), "serve".to_string()];
    if let Some(path) = config_path {
        let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        args.push("--config".to_string());
        args.push(abs.to_string_lossy().into_owned());
    }
    serde_json::json!({
        "mcpServers": {
            "knowledge": {
                "command": gadgetron_bin,
                "args": args
            }
        }
    })
}

/// Write the MCP config JSON to a secure tempfile and return the
/// `NamedTempFile` handle. The caller owns the handle; the file is
/// removed when the handle is dropped.
///
/// The path is available via `handle.path()`. Callers pass that path
/// into the Claude Code command line via `--mcp-config <path>`.
pub fn write_config_file(config_path: Option<&Path>) -> std::io::Result<NamedTempFile> {
    let json = build_config_json(config_path);
    let serialized = serde_json::to_vec_pretty(&json)?;

    let mut tmpfile = NamedTempFile::with_prefix("gadgetron-mcp-")?;
    // NO set_permissions call — mkstemp sets 0600 atomically. Adding a
    // redundant chmod would be misleading and would imply a TOCTOU race
    // that does not exist. Locked in by `tmpfile_has_0600_permissions`.

    tmpfile.write_all(&serialized)?;
    tmpfile.flush()?;
    Ok(tmpfile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn build_config_json_shape() {
        let v = build_config_json(None);
        assert!(v.get("mcpServers").is_some());
        // `command` resolves via `current_exe()` — at test time that's the
        // hashed test binary, in production it's the gadgetron release
        // binary. Either way it must be a non-empty absolute path so
        // Claude Code can spawn it without PATH lookup (see SEC-B1).
        let command = v["mcpServers"]["knowledge"]["command"]
            .as_str()
            .expect("command must be a string");
        assert!(!command.is_empty(), "command must be non-empty");
        assert!(
            command.starts_with('/') || command == "gadgetron",
            "command must be absolute path (current_exe) or the bare fallback, got {command}"
        );
        assert_eq!(v["mcpServers"]["knowledge"]["args"][0], "mcp");
        assert_eq!(v["mcpServers"]["knowledge"]["args"][1], "serve");
        // Without a config_path the args stop after `serve`.
        assert_eq!(
            v["mcpServers"]["knowledge"]["args"]
                .as_array()
                .map(|a| a.len()),
            Some(2)
        );
    }

    #[test]
    fn build_config_json_appends_config_flag_when_path_is_supplied() {
        let tmp = NamedTempFile::with_prefix("gadgetron-toml-").expect("tmp");
        let v = build_config_json(Some(tmp.path()));
        let args = v["mcpServers"]["knowledge"]["args"]
            .as_array()
            .expect("args array");
        assert_eq!(args[0], "mcp");
        assert_eq!(args[1], "serve");
        assert_eq!(args[2], "--config");
        let abs = tmp
            .path()
            .canonicalize()
            .expect("canonicalize tmp")
            .to_string_lossy()
            .into_owned();
        assert_eq!(args[3].as_str().expect("config path str"), abs);
    }

    #[test]
    fn build_config_json_keeps_relative_path_when_canonicalize_fails() {
        // A non-existent path still gets forwarded verbatim. We'd rather
        // surface the error via `gadgetron mcp serve --config <missing>`'s
        // own clear message than silently drop the flag.
        let v = build_config_json(Some(Path::new("/nope/does/not/exist.toml")));
        let args = v["mcpServers"]["knowledge"]["args"]
            .as_array()
            .expect("args array");
        assert_eq!(args[2], "--config");
        assert_eq!(args[3], "/nope/does/not/exist.toml");
    }

    #[test]
    fn tmpfile_content_round_trips_as_json() {
        let tmp = write_config_file(None).expect("write");
        let content = std::fs::read_to_string(tmp.path()).expect("read back");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse");
        assert!(parsed.get("mcpServers").is_some());
    }

    #[test]
    fn tmpfile_has_0600_permissions() {
        // SEC-M1: mkstemp atomically sets 0600 on POSIX. Lock it in so
        // a future refactor that moves to `File::create` (which would
        // honor umask and likely produce 0644) fails this test.
        let tmp = write_config_file(None).expect("write");
        let mode = tmp.as_file().metadata().expect("meta").mode() & 0o777;
        assert_eq!(mode, 0o600, "expected mode 0600, got {mode:o}");
    }

    #[test]
    fn tmpfile_removed_on_drop() {
        // The NamedTempFile Drop impl unlinks the file. Locks in the
        // per-request tempfile lifetime contract.
        let path = {
            let tmp = write_config_file(None).expect("write");
            tmp.path().to_path_buf()
        };
        assert!(!path.exists(), "tempfile should be removed on drop");
    }

    #[test]
    fn tmpfile_path_starts_with_prefix() {
        let tmp = write_config_file(None).expect("write");
        let name = tmp
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf8 filename");
        assert!(
            name.starts_with("gadgetron-mcp-"),
            "prefix must appear in path: {name}"
        );
    }
}
