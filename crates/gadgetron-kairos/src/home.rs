//! Kairos's persistent workspace — a neutral cwd for every Claude Code
//! subprocess spawn.
//!
//! # What this module does (and doesn't)
//!
//! ```text
//! ~/.gadgetron/kairos/          (Kairos's workspace — persistent, one per operator)
//! ├── work/                      (spawn cwd — empty, blocks project-memory leak)
//! │   └── CLAUDE.md              (empty — terminates upward CLAUDE.md search)
//! └── wiki/                      (knowledge store — migrates here in a follow-up PR;
//!                                 currently lives at config.knowledge.wiki_path)
//! ```
//!
//! The spawn drives **CWD** to `work/`; `HOME` is left untouched. Claude
//! Code's "auto-memory" feature derives a per-project slug from the CWD,
//! so running every Kairos request out of `~/.gadgetron/kairos/work/`
//! maps to a private memory path (`~/.claude/projects/-Users-…-gadgetron-kairos-work/`)
//! that never accumulates operator coding-session state. This is the
//! **primary leak vector we're closing**.
//!
//! # Why not sandbox HOME entirely
//!
//! We tried. Setting `HOME` to a fabricated directory breaks Claude Max
//! OAuth on macOS — `claude auth status` returns `"loggedIn": false` with
//! any `HOME` other than the real user home, even when every file under
//! `~/.claude/` and `~/.claude.json` is copied verbatim. Claude Code most
//! likely compares the `HOME` env against `os.homedir()` (the `getpwuid_r`
//! result) and refuses to read the keychain on mismatch — a reasonable
//! anti-theft check, but it means HOME-level sandboxing is impossible
//! without switching to `--bare` + `ANTHROPIC_API_KEY` (pay-per-token).
//!
//! # What isolation we DO get (CWD-only)
//!
//! 1. Per-project auto-memory: closed. The observed leak was Kairos
//!    replaying entries from `~/.claude/projects/-Users-junghopark-dev-gadgetron/memory/`.
//!    With CWD pinned to `~/.gadgetron/kairos/work/`, that path never
//!    gets consulted.
//! 2. Upward `CLAUDE.md` walk: closed. Our `work/CLAUDE.md` (empty)
//!    terminates the walk before it reaches any user-controlled content
//!    in HOME ancestors.
//! 3. Persona: closed (separately, via `--system-prompt` in `spawn.rs`).
//!
//! # What isolation we DON'T get
//!
//! - Global `~/.claude/CLAUDE.md` (operator's global memory): still read
//!   by Claude Code. No CLI flag disables this without `--bare`.
//! - `~/.claude/skills/` / `plugins/` / `agents/`: still discoverable.
//!
//! For operators who need hard isolation (regulated / multi-tenant
//! deployments), Phase 2B's `[agent.brain].mode = "external_anthropic"`
//! with an API key uses `--bare` and sidesteps every item above.
//!
//! # Lifecycle
//!
//! Persistent. Created once on first `prepare_kairos_home()` (idempotent),
//! re-used across server restarts. Operator cleans up by `rm -rf` if
//! needed.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HomeError {
    #[error("I/O error preparing Kairos home: {0}")]
    Io(#[from] std::io::Error),
}

/// Handle to a prepared, persistent Kairos workspace.
///
/// Cheap to clone — holds nothing but a `PathBuf`. Safe to wrap in `Arc`
/// and share across spawns.
///
/// Naming: keeps the `KairosHome` / `kairos_home` terminology because
/// the conceptual "home of Kairos's session state" is still accurate,
/// even though we no longer override the subprocess `HOME` env.
#[derive(Debug, Clone)]
pub struct KairosHome {
    root: PathBuf,
}

impl KairosHome {
    /// Absolute path to the Kairos workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path to Kairos's neutral working directory. Use this as
    /// the spawn `current_dir` so Claude Code's project-memory key maps
    /// away from the operator's real projects and the upward `CLAUDE.md`
    /// walk terminates at our empty defensive file.
    pub fn workdir(&self) -> PathBuf {
        self.root.join("work")
    }

    /// Convention path for the wiki inside the Kairos workspace.
    /// Currently the knowledge layer reads its path from
    /// `config.knowledge.wiki_path` (independent), but the convention is
    /// that it lives here. The follow-up PR migrates `wiki_path` to this
    /// location.
    pub fn wiki_path(&self) -> PathBuf {
        self.root.join("wiki")
    }
}

/// The default Kairos workspace root: `<HOME>/.gadgetron/kairos`.
pub fn default_home_root(real_home: &Path) -> PathBuf {
    real_home.join(".gadgetron/kairos")
}

/// Prepare (or refresh) Kairos's persistent workspace.
///
/// Creates the directory tree if absent and ensures the defensive empty
/// `CLAUDE.md` sits in `work/`. Idempotent — safe to call on every
/// server startup.
///
/// # Arguments
///
/// - `root`: where the Kairos workspace lives. Callers normally pass
///   `default_home_root(&real_home_path)`; tests can override.
///
/// (`real_home` is no longer a parameter — nothing here reads the
/// operator's real `~/.claude/`.)
pub fn prepare_kairos_home(root: &Path) -> Result<KairosHome, HomeError> {
    let root = root.to_path_buf();
    fs::create_dir_all(&root)?;

    // work/ — spawn cwd. Empty CLAUDE.md terminates Claude Code's
    // upward walk so nothing user-controlled in HOME ancestors gets
    // auto-loaded.
    let workdir = root.join("work");
    fs::create_dir_all(&workdir)?;
    let workdir_claude_md = workdir.join("CLAUDE.md");
    if !workdir_claude_md.exists() {
        fs::write(&workdir_claude_md, "")?;
    }

    // wiki/ — placeholder until the knowledge crate reads from here
    // directly. Creating it on every prepare is cheap and means
    // operators can symlink or migrate without restarting.
    let wiki = root.join("wiki");
    fs::create_dir_all(&wiki)?;

    tracing::info!(
        target: "kairos_home",
        root = %root.display(),
        "Kairos workspace ready (cwd-only isolation; HOME not sandboxed)"
    );

    Ok(KairosHome { root })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kairos_home_creates_work_and_wiki() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("kairos");
        let kh = prepare_kairos_home(&root).expect("kairos home");
        assert!(kh.root().exists());
        assert!(kh.workdir().exists());
        assert!(kh.wiki_path().exists());
    }

    #[test]
    fn kairos_home_workdir_has_empty_claude_md() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("kairos");
        let kh = prepare_kairos_home(&root).expect("kairos home");
        let claude_md = kh.workdir().join("CLAUDE.md");
        assert!(claude_md.exists());
        assert_eq!(
            fs::read_to_string(&claude_md).unwrap(),
            "",
            "defensive CLAUDE.md must be empty — if Claude Code finds content here, the upward walk gives our content priority over anything higher in HOME"
        );
    }

    #[test]
    fn kairos_home_is_idempotent() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("kairos");
        let _kh1 = prepare_kairos_home(&root).expect("first prepare");
        // Simulate an operator editing the defensive file to something
        // they want Kairos to always see. Second prepare must NOT
        // overwrite it (idempotent w.r.t. content, not just existence).
        fs::write(root.join("work/CLAUDE.md"), "# kairos operator override").unwrap();
        let _kh2 = prepare_kairos_home(&root).expect("second prepare");
        assert_eq!(
            fs::read_to_string(root.join("work/CLAUDE.md")).unwrap(),
            "# kairos operator override",
            "prepare must not clobber operator edits"
        );
    }

    #[test]
    fn kairos_home_workdir_is_outside_operator_project_slug_space() {
        // Regression: the leak we're fixing was Claude Code auto-loading
        // per-project memory from `~/.claude/projects/<cwd-slug>/memory/`
        // where the slug = cwd with `/` → `-`. This assert locks in that
        // our workdir has a slug that cannot collide with the Gadgetron
        // repo, so operators running demos from `~/dev/gadgetron` never
        // see their coding-session memory replayed.
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("kairos");
        let kh = prepare_kairos_home(&root).expect("kairos home");
        let workdir_str = kh.workdir().to_string_lossy().to_string();
        assert!(
            workdir_str.contains("/kairos/work"),
            "workdir path {workdir_str} must contain /kairos/work so Claude Code's project slug is unmistakably Kairos-scoped"
        );
    }

    #[test]
    fn kairos_home_default_root_uses_gadgetron_subdir() {
        let home = default_home_root(Path::new("/home/alice"));
        assert_eq!(home, PathBuf::from("/home/alice/.gadgetron/kairos"));
    }
}
