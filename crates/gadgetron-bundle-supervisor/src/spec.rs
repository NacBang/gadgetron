use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SandboxInitSpec {
    pub sandbox_root: PathBuf,
    pub entry_source: PathBuf,
    pub entry_relative: String,
    pub entry_sha256: String,
    pub state_root: PathBuf,
    pub args: Vec<String>,
    pub memory_mb: u64,
    pub open_files: u32,
    pub cpu_seconds: u32,
    pub package_manifest_sha256: String,
}
