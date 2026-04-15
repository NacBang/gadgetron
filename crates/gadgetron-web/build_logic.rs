//! Build-time pipeline logic for `gadgetron-web`, extracted from `build.rs` so that
//! `tests/build_rs_logic.rs` can drive the logic with fabricated inputs.
//!
//! This file is `#[path]`-included by BOTH `build.rs` (which has the cargo-runtime
//! `main()`) AND `tests/build_rs_logic.rs` (which calls the functions directly).
//!
//! Do NOT import this file from `src/lib.rs` — the main library does not need it,
//! and pulling it into the library graph would bloat the compile unit.

use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Input to the build_logic pipeline. Separated from `build.rs::main` so tests can
/// drive it with fabricated inputs (`tests/build_rs_logic.rs`).
pub struct BuildEnv {
    pub web_dir: PathBuf,
    pub dist_dir: PathBuf,
    pub skip: bool,
    pub strict: bool,
    pub trust_path: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BuildOutcome {
    Skipped(&'static str),
    FallbackCreated(&'static str),
    BuiltFromNpm,
}

pub fn run(env: &BuildEnv) -> Result<BuildOutcome, String> {
    // Bootstrap state: if `web/` or `web/package.json` does not exist, the Next.js
    // project has not been scaffolded yet (task #4). Use the fallback UI so the
    // crate still compiles. Once `web/package.json` lands, the lockfile check
    // enforces supply-chain hygiene.
    if !env.web_dir.exists() || !env.web_dir.join("package.json").exists() {
        ensure_fallback_dist(&env.dist_dir, "web/ project not yet scaffolded");
        return Ok(BuildOutcome::FallbackCreated("not-scaffolded"));
    }

    let lockfile = env.web_dir.join("package-lock.json");
    if !lockfile.exists() {
        return Err(
            "web/package-lock.json missing. Run `cd crates/gadgetron-web/web && npm install` \
             and commit the lockfile. (M-W3 — supply chain hygiene)"
                .to_string(),
        );
    }

    if env.skip {
        ensure_fallback_dist(&env.dist_dir, "GADGETRON_SKIP_WEB_BUILD set — fallback UI");
        return Ok(BuildOutcome::Skipped("skip-env"));
    }

    let Some(npm) = which_npm(env.trust_path) else {
        if env.strict {
            return Err("npm not found on PATH and feature `strict-build` is enabled".to_string());
        }
        ensure_fallback_dist(
            &env.dist_dir,
            "npm not found — install Node.js to enable the Web UI",
        );
        return Ok(BuildOutcome::FallbackCreated("npm-absent"));
    };

    // `npm ci --ignore-scripts` with env scrub
    let status = scrubbed_npm(&npm, &env.web_dir)
        .args(["ci", "--ignore-scripts"])
        .status()
        .map_err(|e| format!("failed to spawn npm ci: {e}"))?;
    if !status.success() {
        return Err(format!("npm ci exited with status {status}"));
    }

    let status = scrubbed_npm(&npm, &env.web_dir)
        .args(["run", "build"])
        .status()
        .map_err(|e| format!("failed to spawn npm run build: {e}"))?;
    if !status.success() {
        return Err(format!("npm run build exited with status {status}"));
    }

    let out_dir = env.web_dir.join("out");
    if !out_dir.exists() {
        return Err(
            "`npm run build` completed (exit 0) but did not produce `web/out/`. Check that \
             next.config.mjs sets `output: 'export'` and that the build script maps to \
             `next build`. See crates/gadgetron-web/README.md."
                .to_string(),
        );
    }

    let _ = fs::remove_dir_all(&env.dist_dir);
    copy_dir_all(&out_dir, &env.dist_dir)
        .map_err(|e| format!("failed to copy web/out -> web/dist: {e}"))?;

    Ok(BuildOutcome::BuiltFromNpm)
}

/// Build a scrubbed `Command` for invoking `npm`. Removes known secret env vars
/// (SEC-W-B7) but retains `HOME`, `PATH`, `NODE_OPTIONS` which npm itself needs.
fn scrubbed_npm(npm: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(npm);
    cmd.current_dir(cwd);
    for v in &[
        "NPM_TOKEN",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "NODE_AUTH_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "SSH_AUTH_SOCK",
        "GPG_AGENT_INFO",
    ] {
        cmd.env_remove(v);
    }
    cmd
}

/// Locate `npm` on a trusted PATH. Defaults to a hardcoded minimal allowlist
/// that matches typical Node installs on macOS/Linux CI. Opt-in to the full
/// inherited PATH via `GADGETRON_WEB_TRUST_PATH=1` (developer escape hatch).
fn which_npm(trust_path: bool) -> Option<PathBuf> {
    let path = if trust_path {
        env::var_os("PATH").unwrap_or_default()
    } else {
        OsString::from("/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin")
    };
    for dir in env::split_paths(&path) {
        for bin in &["npm", "npm.cmd", "npm.exe"] {
            let candidate = dir.join(bin);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn ensure_fallback_dist(dist: &Path, reason: &str) {
    let _ = fs::remove_dir_all(dist);
    fs::create_dir_all(dist).expect("failed to create fallback dist dir");
    let fallback = format!(
        "<!doctype html>\
         <html lang=\"en\">\
         <head>\
         <meta charset=\"utf-8\">\
         <title>Gadgetron — UI unavailable</title>\
         <meta name=\"gadgetron-api-base\" content=\"/v1\">\
         </head>\
         <body data-testid=\"gadgetron-root\">\
         <h1>Gadgetron Web UI unavailable</h1>\
         <p>{reason}</p>\
         <p>See <code>crates/gadgetron-web/README.md</code> for build instructions.</p>\
         </body>\
         </html>"
    );
    fs::write(dist.join("index.html"), fallback).expect("failed to write fallback index.html");
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_file() {
            fs::copy(&from, &to)?;
        }
        // symlinks and other types are skipped — defense against symlink escapes (B-W5)
    }
    Ok(())
}
