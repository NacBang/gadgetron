//! Thin wrapper over the local `ssh` / `scp` / `ssh-keygen` / `sshpass`
//! binaries. v0.1 shells out instead of linking against `russh` / `openssh-rs`
//! because (a) the tooling is already installed on every Linux dev box,
//! (b) operators recognise the argv shape, (c) SSH config propagation
//! (ControlMaster etc.) is easier if we don't reimplement it.
//!
//! Command rules:
//!
//! - `StrictHostKeyChecking=accept-new` so the first connect auto-trusts
//!   the host key, but later key swaps still fail closed.
//! - `UserKnownHostsFile` pins into `$INVENTORY_DIR/known_hosts` — a
//!   per-bundle file keeps our state out of `$HOME/.ssh/known_hosts`.
//! - `BatchMode=yes` for key-based connections — if the agent is missing
//!   a key, we fail fast rather than prompt on the gadgetron host's TTY.
//! - Password mode (`sshpass -e`) passes the secret via env var so it
//!   never lands in argv / `ps` output.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::process::Command;
use zeroize::drop_secret;

pub(crate) mod zeroize {
    //! Inline "zeroize" — we avoid pulling the `zeroize` crate for a
    //! single byte-wipe on a temporary password. Drop-in scope only.

    pub fn drop_secret(mut s: String) {
        // Overwrite the heap buffer before drop so a later allocator
        // reuse doesn't expose the plaintext to an attacker scanning
        // process memory. For a String this is best-effort (the SSO
        // optimisation doesn't apply — Rust's String is always heap).
        unsafe {
            let bytes = s.as_bytes_mut();
            for b in bytes.iter_mut() {
                *b = 0;
            }
        }
        drop(s);
    }
}

#[derive(Debug, Error)]
pub enum InventoryError {
    #[error("setup: {0}")]
    Setup(String),
    #[error("corrupt inventory: {0}")]
    Corrupt(String),
    #[error("io: {0}")]
    Io(String),
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("io: {0}")]
    Io(String),
    #[error("ssh failed (exit={code}): {stderr}")]
    Failed { code: i32, stderr: String },
    #[error("`sshpass` binary missing — install with `apt-get install sshpass` to use password auth")]
    SshpassMissing,
    #[error("bootstrap: {0}")]
    Bootstrap(String),
    #[error("inventory: {0}")]
    Inventory(#[from] InventoryError),
}

/// Handle to a reachable host, carrying everything `ssh`/`scp` need.
#[derive(Debug, Clone)]
pub struct SshTarget {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub key_path: Option<PathBuf>,
    pub known_hosts: PathBuf,
}

impl SshTarget {
    pub fn argv_base(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "-p".into(),
            self.port.to_string(),
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", self.known_hosts.display()),
            "-o".into(),
            "ConnectTimeout=8".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
        ];
        if let Some(kp) = &self.key_path {
            a.extend([
                "-i".into(),
                kp.display().to_string(),
                "-o".into(),
                "IdentitiesOnly=yes".into(),
                "-o".into(),
                "BatchMode=yes".into(),
            ]);
        }
        a
    }
}

pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl CmdOutput {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Run a remote command over key-based SSH. Non-zero exit is NOT an
/// error — callers decide how to handle it (e.g. `sensors` exits non-zero
/// when no chips are detected; still a readable signal).
pub async fn exec(target: &SshTarget, cmd: &str) -> Result<CmdOutput, SshError> {
    let mut argv = target.argv_base();
    argv.push(format!("{}@{}", target.user, target.host));
    // OpenSSH concatenates remote-command argv elements with a single
    // space before handing them to the login shell on the target — any
    // quoting from `Command::args` is lost in transit. Pass the whole
    // shell snippet as ONE argument so bash on the remote evaluates it
    // intact. Earlier versions split this into `bash`, `-lc`, `<cmd>`
    // and collided: bash saw `-c echo` with `__gsm_ready__` as `$0`,
    // echoing nothing. Keep the explicit `bash -lc` wrapper so a
    // non-bash login shell (dash/fish) still picks up a predictable
    // environment.
    argv.push(format!("bash -lc {}", shell_escape_single(cmd)));
    let output = Command::new("ssh")
        .args(&argv)
        .output()
        .await
        .map_err(|e| SshError::Io(format!("spawn ssh: {e}")))?;
    Ok(CmdOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

/// Safe single-quote escaping for bash. `'foo'bar'` → `'foo'\''bar'`.
pub(crate) fn shell_escape_single(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Password-based variant (for Mode C bootstrap only). `sshpass -e` reads
/// the password from `SSHPASS` env so it never appears in argv.
pub async fn exec_with_password(
    target: &SshTarget,
    password: &str,
    cmd: &str,
) -> Result<CmdOutput, SshError> {
    if Command::new("sshpass")
        .arg("-V")
        .output()
        .await
        .is_err()
    {
        return Err(SshError::SshpassMissing);
    }
    // Pwd mode = no -i / no BatchMode, but still accept-new + known_hosts.
    let mut argv: Vec<String> = vec![
        "-e".into(),
        "ssh".into(),
        "-p".into(),
        target.port.to_string(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", target.known_hosts.display()),
        "-o".into(),
        "ConnectTimeout=8".into(),
        "-o".into(),
        "PubkeyAuthentication=no".into(),
        "-o".into(),
        "PreferredAuthentications=password".into(),
        format!("{}@{}", target.user, target.host),
        // Same joined-with-spaces rule as `exec` — push the `bash -lc
        // <body>` wrapper as a single argv element.
        format!("bash -lc {}", shell_escape_single(cmd)),
    ];
    let output = Command::new("sshpass")
        .args(&argv)
        .env("SSHPASS", password)
        .output()
        .await
        .map_err(|e| SshError::Io(format!("spawn sshpass: {e}")))?;
    argv.clear();
    Ok(CmdOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(-1),
    })
}

/// Generate a fresh ed25519 keypair at the given path (no passphrase,
/// 0600). Used during Mode C bootstrap.
pub async fn generate_keypair(priv_path: &Path, comment: &str) -> Result<(), SshError> {
    if let Some(parent) = priv_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| SshError::Io(format!("mkdir key parent: {e}")))?;
    }
    // ssh-keygen refuses to overwrite; make sure stale files are gone.
    let _ = tokio::fs::remove_file(priv_path).await;
    let pub_path = priv_path.with_extension("pub");
    let _ = tokio::fs::remove_file(&pub_path).await;
    let output = Command::new("ssh-keygen")
        .args([
            "-t",
            "ed25519",
            "-N",
            "",
            "-f",
            &priv_path.display().to_string(),
            "-C",
            comment,
            "-q",
        ])
        .output()
        .await
        .map_err(|e| SshError::Io(format!("spawn ssh-keygen: {e}")))?;
    if !output.status.success() {
        return Err(SshError::Failed {
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(priv_path, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::set_permissions(&pub_path, std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

/// Read the public-key companion of a private key created by
/// [`generate_keypair`]. Returns the single-line form suitable for
/// appending to `authorized_keys`.
pub async fn read_pubkey(priv_path: &Path) -> Result<String, SshError> {
    let pub_path = priv_path.with_extension("pub");
    let bytes = tokio::fs::read(&pub_path)
        .await
        .map_err(|e| SshError::Io(format!("read {}: {e}", pub_path.display())))?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Write a pasted PEM key to disk with 0600 perms. Mode B entry point.
pub async fn install_pasted_key(priv_path: &Path, pem: &str) -> Result<(), SshError> {
    if let Some(parent) = priv_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| SshError::Io(format!("mkdir key parent: {e}")))?;
    }
    tokio::fs::write(priv_path, pem.as_bytes())
        .await
        .map_err(|e| SshError::Io(format!("write key: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(priv_path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Wipe-on-drop password wrapper. Helper for callers that want to be
/// extra careful about when the bytes go away.
pub struct OneShotSecret(Option<String>);

impl OneShotSecret {
    pub fn new(s: String) -> Self {
        Self(Some(s))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_deref().unwrap_or("")
    }
    pub fn consume(mut self) -> Option<String> {
        self.0.take()
    }
}

impl Drop for OneShotSecret {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            drop_secret(s);
        }
    }
}
