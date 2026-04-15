//! Build-time pipeline for `gadgetron-web`.
//!
//! Invokes `npm ci --ignore-scripts && npm run build` under a scrubbed environment
//! and copies the Next.js static-export output (`web/out/`) to `web/dist/`, where
//! `include_dir!` picks it up at compile time.
//!
//! See `docs/design/phase2/03-gadgetron-web.md` §4. The core logic lives in
//! `build_logic.rs` (sibling to this file) so `tests/build_rs_logic.rs` can include
//! and exercise it with fabricated inputs.
//!
//! Environment variables (M-W3):
//! - `GADGETRON_SKIP_WEB_BUILD` (`1`/`true`/`yes`): skip npm entirely, embed a
//!   fallback `index.html`. Used by CI for Rust-only checks.
//! - `GADGETRON_WEB_TRUST_PATH=1`: use the full inherited `PATH` when resolving
//!   `npm`. Default resolves against a hardcoded minimal PATH allowlist to defend
//!   against PATH-substitution attacks.

#[path = "build_logic.rs"]
mod build_logic;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn main() -> ExitCode {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let web_dir = manifest_dir.join("web");
    let dist_dir = web_dir.join("dist");

    emit_rerun_triggers(&web_dir);

    // Ensure the dist directory exists at a minimum, so `include_dir!` at compile
    // time does not fail on a missing path. `build_logic::run` overwrites this with
    // either the fallback or real build output.
    if !dist_dir.exists() {
        let _ = fs::create_dir_all(&dist_dir);
    }

    let skip_raw = env::var("GADGETRON_SKIP_WEB_BUILD").ok();
    let skip = matches!(skip_raw.as_deref(), Some("1" | "true" | "yes"));

    let env_struct = build_logic::BuildEnv {
        web_dir,
        dist_dir,
        skip,
        strict: cfg!(feature = "strict-build"),
        trust_path: env::var("GADGETRON_WEB_TRUST_PATH").ok().as_deref() == Some("1"),
    };

    match build_logic::run(&env_struct) {
        Ok(build_logic::BuildOutcome::Skipped(_)) => {
            println!(
                "cargo:warning=gadgetron-web: GADGETRON_SKIP_WEB_BUILD set — using fallback UI"
            );
            ExitCode::SUCCESS
        }
        Ok(build_logic::BuildOutcome::FallbackCreated(reason)) => {
            println!("cargo:warning=gadgetron-web: {reason} — Web UI disabled, using fallback");
            ExitCode::SUCCESS
        }
        Ok(build_logic::BuildOutcome::BuiltFromNpm) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("gadgetron-web: {msg}");
            ExitCode::from(1)
        }
    }
}

fn emit_rerun_triggers(web_dir: &Path) {
    for rel in &[
        "package.json",
        "package-lock.json",
        ".nvmrc",
        "next.config.mjs",
        "tailwind.config.ts",
        "postcss.config.mjs",
        "tsconfig.json",
        "vitest.config.ts",
        "app",
        "components",
        "lib",
        "public",
    ] {
        println!("cargo:rerun-if-changed={}", web_dir.join(rel).display());
    }
    for env_var in &["GADGETRON_SKIP_WEB_BUILD", "GADGETRON_WEB_TRUST_PATH"] {
        println!("cargo:rerun-if-env-changed={env_var}");
    }
    // Also re-run if the build_logic source itself changes.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_logic.rs");
}
