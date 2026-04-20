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

    // Post-copy consistency check: every `/web/_next/static/...` reference
    // that HTML pages embed at build time must resolve to an actual file
    // in the copied dist. Catches the "CSS hash mismatch" class of bug
    // where `npm run build` finishes cleanly but leaves the HTML refs out
    // of sync with the emitted asset filenames (observed 2026-04-20 — a
    // deployed build held an old index.html with a stale CSS hash
    // alongside newer CSS/JS bundles; every asset request 404'd at
    // runtime, producing a fully unstyled page).
    verify_asset_consistency(&env.dist_dir)?;

    Ok(BuildOutcome::BuiltFromNpm)
}

/// Verify that every `/web/_next/static/...` reference embedded in any
/// `.html` file under `dist_dir` resolves to an actual file in the
/// copied dist. Returns `Err` listing missing paths if any mismatch.
///
/// Contract: fail the build rather than embed an incoherent dist. The
/// alternative — silently shipping a binary whose `index.html` references
/// assets that don't exist — produces a fully unstyled page at runtime
/// (every asset request 404s → browser falls back to default styling,
/// lucide-react icons render as empty boxes, Tailwind classes don't
/// apply). That failure mode is unrecoverable from the user side and
/// ~impossible to diagnose without deep knowledge of the embedded-asset
/// contract.
pub fn verify_asset_consistency(dist_dir: &Path) -> Result<(), String> {
    let html_refs = collect_html_asset_refs(dist_dir)
        .map_err(|e| format!("failed to walk dist/ for consistency check: {e}"))?;

    let mut missing: Vec<String> = Vec::new();
    for reference in &html_refs {
        // HTML path format: `/web/_next/static/...`. Strip the `/web/`
        // mount prefix to get the on-disk relative path under dist.
        let relative = reference.strip_prefix("/web/").unwrap_or(reference);
        let expected = dist_dir.join(relative);
        if !expected.exists() {
            missing.push(reference.clone());
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let mut msg = String::from(
        "asset consistency check FAILED — the HTML files in web/dist/ \
         reference assets that are not present in the copied dist. This \
         produces a broken /web landing page at runtime (CSS 404 → \
         unstyled page; icon bundle 404 → empty-button placeholders). \
         Common cause: stale web/dist/ from a prior build raced with \
         a fresh web/out/. Fix:\n\
           1. cd crates/gadgetron-web/web\n\
           2. rm -rf out dist node_modules/.cache .next\n\
           3. cd ../../.. && cargo clean -p gadgetron-web\n\
           4. cargo build --release\n\n\
         Missing asset references (from web/dist/*.html):\n",
    );
    for r in &missing {
        msg.push_str("  - ");
        msg.push_str(r);
        msg.push('\n');
    }
    Err(msg)
}

/// Walk every `.html` file under `dist_dir`, extract every
/// `/web/_next/static/...` reference, return the deduped set.
fn collect_html_asset_refs(dist_dir: &Path) -> std::io::Result<std::collections::BTreeSet<String>> {
    let mut refs = std::collections::BTreeSet::new();
    collect_html_asset_refs_inner(dist_dir, &mut refs)?;
    Ok(refs)
}

fn collect_html_asset_refs_inner(
    dir: &Path,
    refs: &mut std::collections::BTreeSet<String>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let path = entry.path();
        if ty.is_dir() {
            collect_html_asset_refs_inner(&path, refs)?;
            continue;
        }
        if !ty.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("html") {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        extract_web_next_refs(&content, refs);
    }
    Ok(())
}

/// Find every `/web/_next/static/<path>` substring in `content`. Pure
/// string scan (no HTML parser) because the HTML contains both
/// attribute-quoted and JS-string-quoted references.
fn extract_web_next_refs(content: &str, refs: &mut std::collections::BTreeSet<String>) {
    const MARKER: &str = "/web/_next/static/";
    let bytes = content.as_bytes();
    let mut i = 0;
    while let Some(start) = content[i..].find(MARKER).map(|p| p + i) {
        let mut end = start + MARKER.len();
        while end < bytes.len() {
            let b = bytes[end];
            // URL path char set: alnum, `_`, `-`, `.`, `/`. Terminate on
            // quote / whitespace / tag delimiter / anything else.
            let is_path_char = b.is_ascii_alphanumeric()
                || b == b'_'
                || b == b'-'
                || b == b'.'
                || b == b'/';
            if !is_path_char {
                break;
            }
            end += 1;
        }
        if end > start + MARKER.len() {
            refs.insert(content[start..end].to_string());
        }
        i = end.max(start + 1);
    }
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
