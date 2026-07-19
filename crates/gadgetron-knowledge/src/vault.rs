//! Tenant-scoped Domain Vault physical layout.
//!
//! R2.1 deliberately keeps identity/ACL/revision rows in PostgreSQL while
//! this module owns the canonical Git/Obsidian working-tree boundary. One
//! repository exists per tenant; Space and home-Bundle are directories inside
//! that repository. A lock outside `.git` serializes mutations without making
//! unrelated tenants wait for each other.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use git2::{IndexAddOption, Repository, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const VAULT_LAYOUT_VERSION: u32 = 1;
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum VaultLayoutError {
    #[error("invalid home Bundle id {0:?}")]
    InvalidBundleId(String),
    #[error("tenant Vault lock timed out after {0:?}")]
    LockTimeout(Duration),
    #[error("tenant Vault identity mismatch: expected {expected}, found {actual}")]
    TenantMismatch { expected: Uuid, actual: Uuid },
    #[error("unsupported tenant Vault layout version {0}")]
    UnsupportedLayout(u32),
    #[error("tenant Vault is missing layout metadata at {0}")]
    MissingLayout(PathBuf),
    #[error("invalid Domain Vault note path {0:?}")]
    InvalidNotePath(String),
    #[error("Domain Vault note path is a symbolic link: {0}")]
    NoteSymlink(PathBuf),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git error: {0}")]
    Git(#[from] git2::Error),
    #[error("tenant Vault Git revision changed: expected {expected}, found {actual}")]
    GitRevisionConflict { expected: String, actual: String },
    #[error("layout JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct TenantVaultLayout {
    root: PathBuf,
}

#[derive(Debug)]
pub struct TenantVaultRepository {
    layout: TenantVaultLayout,
    tenant_id: Uuid,
}

#[derive(Debug)]
pub struct TenantVaultLock {
    file: File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DomainVaultPath {
    pub tenant_id: Uuid,
    pub space_id: Uuid,
    pub home_bundle_id: String,
    pub root: PathBuf,
    pub git_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultSnapshot {
    pub tenant_id: Uuid,
    pub layout_version: u32,
    pub git_head: String,
    pub files: Vec<VaultFileDigest>,
    pub checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultFileDigest {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VaultNoteState {
    pub path: String,
    pub content_hash: String,
    pub git_revision: String,
    pub bytes: Vec<u8>,
    pub externally_changed: bool,
}

#[derive(Debug, Clone)]
pub struct VaultNoteWrite {
    pub relative_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct VaultNoteRevisionWrite<'a> {
    pub space_id: Uuid,
    pub home_bundle_id: &'a str,
    pub relative_path: &'a str,
    pub bytes: &'a [u8],
    pub expected_git_revision: &'a str,
    pub message: &'a str,
}

/// Build a readable, collision-resistant Obsidian note locator. The UUID in
/// YAML/PostgreSQL remains the identity; this path is only a human locator.
pub fn note_relative_path(title: &str, id: Uuid) -> String {
    domain_note_relative_path("notes", title, id).expect("the static notes path prefix is valid")
}

/// Build a readable note locator below a signed, single-segment domain
/// prefix (for example `incidents/`).
pub fn domain_note_relative_path(
    prefix: &str,
    title: &str,
    id: Uuid,
) -> Result<String, VaultLayoutError> {
    if !valid_note_path_prefix(prefix) {
        return Err(VaultLayoutError::InvalidNotePath(prefix.to_string()));
    }
    let mut slug = String::with_capacity(title.len().min(96));
    let mut separator = true;
    let mut characters = 0usize;
    for character in title.trim().chars() {
        if character.is_alphanumeric() {
            for lowercase in character.to_lowercase() {
                slug.push(lowercase);
                characters += 1;
            }
            separator = false;
        } else if !separator {
            slug.push('-');
            characters += 1;
            separator = true;
        }
        if characters >= 80 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("note");
    }
    let id = id.simple().to_string();
    Ok(format!("{prefix}/{slug}--{}.md", &id[..8]))
}

/// Validate the exact relative path accepted by Domain Vault note writes.
pub fn validate_note_relative_path(relative_path: &str) -> Result<(), VaultLayoutError> {
    let parts: Vec<_> = relative_path.split('/').collect();
    let valid = parts.len() == 2
        && valid_note_path_prefix(parts[0])
        && parts[1].ends_with(".md")
        && valid_note_file_name(parts[1].trim_end_matches(".md"));
    if valid {
        Ok(())
    } else {
        Err(VaultLayoutError::InvalidNotePath(relative_path.to_string()))
    }
}

fn valid_note_path_prefix(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LayoutProjection {
    layout_version: u32,
    tenant_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DomainProjection {
    layout_version: u32,
    tenant_id: Uuid,
    space_id: Uuid,
    home_bundle_id: String,
}

impl TenantVaultLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn tenant_root(&self, tenant_id: Uuid) -> PathBuf {
        self.root.join("tenants").join(tenant_id.to_string())
    }

    pub fn repository_root(&self, tenant_id: Uuid) -> PathBuf {
        self.tenant_root(tenant_id).join("vault")
    }

    pub fn lock_path(&self, tenant_id: Uuid) -> PathBuf {
        self.tenant_root(tenant_id).join(".vault.lock")
    }

    pub fn domain_root(
        &self,
        tenant_id: Uuid,
        space_id: Uuid,
        home_bundle_id: &str,
    ) -> Result<PathBuf, VaultLayoutError> {
        validate_bundle_id(home_bundle_id)?;
        Ok(self
            .repository_root(tenant_id)
            .join("spaces")
            .join(space_id.to_string())
            .join("domains")
            .join(home_bundle_id))
    }

    pub fn open_or_init(&self, tenant_id: Uuid) -> Result<TenantVaultRepository, VaultLayoutError> {
        let repository = TenantVaultRepository {
            layout: self.clone(),
            tenant_id,
        };
        repository.initialize()?;
        Ok(repository)
    }

    pub fn open_existing(
        &self,
        tenant_id: Uuid,
    ) -> Result<TenantVaultRepository, VaultLayoutError> {
        let repository = TenantVaultRepository {
            layout: self.clone(),
            tenant_id,
        };
        repository.verify_identity()?;
        Ok(repository)
    }
}

impl TenantVaultRepository {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    pub fn root(&self) -> PathBuf {
        self.layout.repository_root(self.tenant_id)
    }

    pub fn acquire_lock(&self, timeout: Duration) -> Result<TenantVaultLock, VaultLayoutError> {
        let tenant_root = self.layout.tenant_root(self.tenant_id);
        fs::create_dir_all(&tenant_root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.layout.lock_path(self.tenant_id))?;
        let started = Instant::now();
        loop {
            // v1 supports Linux. `flock` is process-scoped and automatically
            // released by the kernel on crash/close, unlike create-new sentinels.
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(TenantVaultLock { file });
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(VaultLayoutError::Io(error));
            }
            if started.elapsed() >= timeout {
                return Err(VaultLayoutError::LockTimeout(timeout));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn ensure_domain(
        &self,
        space_id: Uuid,
        home_bundle_id: &str,
    ) -> Result<DomainVaultPath, VaultLayoutError> {
        validate_bundle_id(home_bundle_id)?;
        let _lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.verify_identity_unlocked()?;
        let domain_root = self
            .layout
            .domain_root(self.tenant_id, space_id, home_bundle_id)?;
        for directory in ["notes", "sources", "_attachments"] {
            fs::create_dir_all(domain_root.join(directory))?;
        }
        let projection = DomainProjection {
            layout_version: VAULT_LAYOUT_VERSION,
            tenant_id: self.tenant_id,
            space_id,
            home_bundle_id: home_bundle_id.to_string(),
        };
        let metadata_path = domain_root.join("_domain.json");
        let wanted = pretty_json(&projection)?;
        let changed = fs::read(&metadata_path).map_or(true, |existing| existing != wanted);
        if changed {
            write_atomic(&metadata_path, &wanted)?;
        }
        let revision = if changed {
            commit_all(&self.root(), "vault: ensure domain")?
        } else {
            repository_head(&self.root())?
        };
        Ok(DomainVaultPath {
            tenant_id: self.tenant_id,
            space_id,
            home_bundle_id: home_bundle_id.to_string(),
            root: domain_root,
            git_revision: revision,
        })
    }

    pub fn write_note(
        &self,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<VaultNoteState, VaultLayoutError> {
        let lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.write_note_locked(
            &lock,
            space_id,
            home_bundle_id,
            relative_path,
            bytes,
            message,
        )
    }

    pub fn write_note_locked(
        &self,
        _lock: &TenantVaultLock,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
        bytes: &[u8],
        message: &str,
    ) -> Result<VaultNoteState, VaultLayoutError> {
        self.verify_identity_unlocked()?;
        let path = self.note_path(space_id, home_bundle_id, relative_path)?;
        reject_symlink_chain(&self.root(), &path)?;
        write_atomic(&path, bytes)?;
        let git_revision = commit_all(&self.root(), message)?;
        Ok(VaultNoteState {
            path: relative_path.to_string(),
            content_hash: hex::encode(Sha256::digest(bytes)),
            git_revision,
            bytes: bytes.to_vec(),
            externally_changed: false,
        })
    }

    pub fn write_note_at_revision_locked(
        &self,
        lock: &TenantVaultLock,
        write: VaultNoteRevisionWrite<'_>,
    ) -> Result<VaultNoteState, VaultLayoutError> {
        self.verify_identity_unlocked()?;
        let actual = repository_head(&self.root())?;
        if actual != write.expected_git_revision {
            return Err(VaultLayoutError::GitRevisionConflict {
                expected: write.expected_git_revision.to_string(),
                actual,
            });
        }
        self.write_note_locked(
            lock,
            write.space_id,
            write.home_bundle_id,
            write.relative_path,
            write.bytes,
            write.message,
        )
    }

    pub fn write_notes_batch(
        &self,
        space_id: Uuid,
        home_bundle_id: &str,
        writes: Vec<VaultNoteWrite>,
        expected_git_revision: Option<&str>,
        message: &str,
    ) -> Result<Vec<VaultNoteState>, VaultLayoutError> {
        if writes.is_empty() {
            return Ok(Vec::new());
        }
        let lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.verify_identity_unlocked()?;
        let current_revision = repository_head(&self.root())?;
        if expected_git_revision.is_some_and(|expected| expected != current_revision) {
            return Err(VaultLayoutError::GitRevisionConflict {
                expected: expected_git_revision.unwrap_or_default().to_string(),
                actual: current_revision,
            });
        }
        let mut seen = HashSet::with_capacity(writes.len());
        let mut prepared = Vec::with_capacity(writes.len());
        for write in writes {
            if !seen.insert(write.relative_path.clone()) {
                return Err(VaultLayoutError::InvalidNotePath(write.relative_path));
            }
            let path = self.note_path(space_id, home_bundle_id, &write.relative_path)?;
            reject_symlink_chain(&self.root(), &path)?;
            let original = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            prepared.push((write, path, original));
        }
        for (index, (write, path, _)) in prepared.iter().enumerate() {
            if let Err(error) = write_atomic(path, &write.bytes) {
                restore_note_batch(&prepared[..index])?;
                drop(lock);
                return Err(error.into());
            }
        }
        let git_revision = match commit_all(&self.root(), message) {
            Ok(revision) => revision,
            Err(error) => {
                restore_note_batch(&prepared)?;
                return Err(error);
            }
        };
        Ok(prepared
            .into_iter()
            .map(|(write, _, _)| VaultNoteState {
                path: write.relative_path,
                content_hash: hex::encode(Sha256::digest(&write.bytes)),
                git_revision: git_revision.clone(),
                bytes: write.bytes,
                externally_changed: false,
            })
            .collect())
    }

    /// Read a note without changing Git or PostgreSQL projections. The caller
    /// decides whether a hash mismatch is usable; reviewed retrieval rejects it.
    pub fn read_note_exact(
        &self,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
        expected_hash: Option<&str>,
    ) -> Result<VaultNoteState, VaultLayoutError> {
        let _lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.verify_identity_unlocked()?;
        let path = self.note_path(space_id, home_bundle_id, relative_path)?;
        reject_symlink_chain(&self.root(), &path)?;
        let bytes = fs::read(&path)?;
        let content_hash = hex::encode(Sha256::digest(&bytes));
        Ok(VaultNoteState {
            path: relative_path.to_string(),
            externally_changed: expected_hash != Some(content_hash.as_str()),
            content_hash,
            git_revision: repository_head(&self.root())?,
            bytes,
        })
    }

    /// Read a stable note and reconcile an external Obsidian edit into Git.
    /// `expected_hash` is the PostgreSQL object hash (without a `sha256:`
    /// prefix). A mismatch commits the working-tree edit before returning;
    /// the caller then advances the object revision with compare-and-swap.
    pub fn read_note_reconciled(
        &self,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
        expected_hash: Option<&str>,
    ) -> Result<VaultNoteState, VaultLayoutError> {
        let lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.read_note_reconciled_locked(
            &lock,
            space_id,
            home_bundle_id,
            relative_path,
            expected_hash,
        )
    }

    pub fn read_note_reconciled_locked(
        &self,
        _lock: &TenantVaultLock,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
        expected_hash: Option<&str>,
    ) -> Result<VaultNoteState, VaultLayoutError> {
        self.verify_identity_unlocked()?;
        let path = self.note_path(space_id, home_bundle_id, relative_path)?;
        reject_symlink_chain(&self.root(), &path)?;
        let bytes = fs::read(&path)?;
        let content_hash = hex::encode(Sha256::digest(&bytes));
        let externally_changed = expected_hash != Some(content_hash.as_str());
        let git_revision = if externally_changed {
            commit_all(&self.root(), "vault: reconcile external Obsidian edit")?
        } else {
            repository_head(&self.root())?
        };
        Ok(VaultNoteState {
            path: relative_path.to_string(),
            content_hash,
            git_revision,
            bytes,
            externally_changed,
        })
    }

    pub fn archive_note_locked(
        &self,
        _lock: &TenantVaultLock,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
    ) -> Result<String, VaultLayoutError> {
        self.verify_identity_unlocked()?;
        let path = self.note_path(space_id, home_bundle_id, relative_path)?;
        reject_symlink_chain(&self.root(), &path)?;
        let filename = path
            .file_name()
            .ok_or_else(|| VaultLayoutError::InvalidNotePath(relative_path.to_string()))?;
        let archived = path
            .parent()
            .ok_or_else(|| VaultLayoutError::InvalidNotePath(relative_path.to_string()))?
            .join("_archived")
            .join(filename);
        reject_symlink_chain(&self.root(), &archived)?;
        fs::create_dir_all(
            archived
                .parent()
                .ok_or_else(|| VaultLayoutError::InvalidNotePath(relative_path.to_string()))?,
        )?;
        fs::rename(path, archived)?;
        commit_all(&self.root(), "vault: archive Obsidian note")
    }

    fn note_path(
        &self,
        space_id: Uuid,
        home_bundle_id: &str,
        relative_path: &str,
    ) -> Result<PathBuf, VaultLayoutError> {
        validate_bundle_id(home_bundle_id)?;
        validate_note_relative_path(relative_path)?;
        Ok(self
            .layout
            .domain_root(self.tenant_id, space_id, home_bundle_id)?
            .join(relative_path))
    }

    pub fn snapshot(&self) -> Result<VaultSnapshot, VaultLayoutError> {
        let _lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        let layout = self.read_layout()?;
        let mut files = Vec::new();
        walk_files(&self.root(), &self.root(), &mut files)?;
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let mut aggregate = Sha256::new();
        for file in &files {
            aggregate.update(file.path.as_bytes());
            aggregate.update([0]);
            aggregate.update(file.sha256.as_bytes());
            aggregate.update([0]);
            aggregate.update(file.bytes.to_le_bytes());
        }
        Ok(VaultSnapshot {
            tenant_id: self.tenant_id,
            layout_version: layout.layout_version,
            git_head: repository_head(&self.root())?,
            files,
            checksum: hex::encode(aggregate.finalize()),
        })
    }

    /// Clone a committed tenant Vault into another root and recreate empty
    /// Obsidian working directories. DB rows are intentionally not copied;
    /// callers restore those from the same tenant checkpoint first.
    pub fn clone_to(
        &self,
        target_root: impl Into<PathBuf>,
    ) -> Result<TenantVaultRepository, VaultLayoutError> {
        let _lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.verify_identity_unlocked()?;
        let target_layout = TenantVaultLayout::new(target_root);
        let tenant_root = target_layout.tenant_root(self.tenant_id);
        fs::create_dir_all(&tenant_root)?;
        let target_repo = target_layout.repository_root(self.tenant_id);
        if target_repo.exists() {
            return Err(VaultLayoutError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("restore target already exists: {}", target_repo.display()),
            )));
        }
        Repository::clone(self.root().to_string_lossy().as_ref(), &target_repo)?;
        let restored = TenantVaultRepository {
            layout: target_layout,
            tenant_id: self.tenant_id,
        };
        restored.verify_identity()?;
        restored.reconcile_empty_directories()?;
        Ok(restored)
    }

    pub fn verify_identity(&self) -> Result<(), VaultLayoutError> {
        let _lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        self.verify_identity_unlocked()
    }

    fn initialize(&self) -> Result<(), VaultLayoutError> {
        let _lock = self.acquire_lock(DEFAULT_LOCK_TIMEOUT)?;
        let root = self.root();
        fs::create_dir_all(&root)?;
        if Repository::open(&root).is_err() {
            Repository::init(&root)?;
        }
        let metadata_dir = root.join(".gadgetron");
        fs::create_dir_all(&metadata_dir)?;
        let metadata_path = metadata_dir.join("layout.json");
        if metadata_path.exists() {
            return self.verify_identity_unlocked();
        }
        let projection = LayoutProjection {
            layout_version: VAULT_LAYOUT_VERSION,
            tenant_id: self.tenant_id,
        };
        write_atomic(&metadata_path, &pretty_json(&projection)?)?;
        commit_all(&root, "vault: initialize layout")?;
        Ok(())
    }

    fn verify_identity_unlocked(&self) -> Result<(), VaultLayoutError> {
        let projection = self.read_layout()?;
        if projection.layout_version != VAULT_LAYOUT_VERSION {
            return Err(VaultLayoutError::UnsupportedLayout(
                projection.layout_version,
            ));
        }
        if projection.tenant_id != self.tenant_id {
            return Err(VaultLayoutError::TenantMismatch {
                expected: self.tenant_id,
                actual: projection.tenant_id,
            });
        }
        Repository::open(self.root())?;
        Ok(())
    }

    fn read_layout(&self) -> Result<LayoutProjection, VaultLayoutError> {
        let path = self.root().join(".gadgetron/layout.json");
        if !path.is_file() {
            return Err(VaultLayoutError::MissingLayout(path));
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn reconcile_empty_directories(&self) -> Result<(), VaultLayoutError> {
        let spaces = self.root().join("spaces");
        if !spaces.exists() {
            return Ok(());
        }
        for space in fs::read_dir(spaces)? {
            let domains = space?.path().join("domains");
            if !domains.is_dir() {
                continue;
            }
            for domain in fs::read_dir(domains)? {
                let domain = domain?.path();
                if domain.join("_domain.json").is_file() {
                    for directory in ["notes", "sources", "_attachments"] {
                        fs::create_dir_all(domain.join(directory))?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn valid_note_file_name(stem: &str) -> bool {
    if Uuid::parse_str(stem).is_ok() {
        return true;
    }
    let Some((slug, suffix)) = stem.rsplit_once("--") else {
        return false;
    };
    !slug.is_empty()
        && stem.chars().count() <= 96
        && slug
            .chars()
            .all(|character| character.is_alphanumeric() || character == '-')
        && suffix.len() == 8
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn restore_note_batch(
    prepared: &[(VaultNoteWrite, PathBuf, Option<Vec<u8>>)],
) -> Result<(), VaultLayoutError> {
    for (_, path, original) in prepared.iter().rev() {
        match original {
            Some(bytes) => write_atomic(path, bytes)?,
            None => match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            },
        }
    }
    Ok(())
}

impl Drop for TenantVaultLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn validate_bundle_id(value: &str) -> Result<(), VaultLayoutError> {
    let valid = (2..=64).contains(&value.len())
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' => true,
            b'0'..=b'9' => index > 0,
            b'-' => index > 0 && index + 1 < value.len(),
            _ => false,
        });
    if valid {
        Ok(())
    } else {
        Err(VaultLayoutError::InvalidBundleId(value.to_string()))
    }
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("vault"),
        Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn reject_symlink_chain(root: &Path, path: &Path) -> Result<(), VaultLayoutError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| VaultLayoutError::InvalidNotePath(path.display().to_string()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(VaultLayoutError::NoteSymlink(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn commit_all(root: &Path, message: &str) -> Result<String, VaultLayoutError> {
    let repo = Repository::open(root)?;
    let mut index = repo.index()?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let signature = Signature::now("Gadgetron Vault", "vault@gadgetron.local")?;
    let oid = match repo.head() {
        Ok(head) => {
            let parent = head.peel_to_commit()?;
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &[&parent],
            )?
        }
        Err(error) if error.code() == git2::ErrorCode::UnbornBranch => {
            repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &[])?
        }
        Err(error) => return Err(error.into()),
    };
    Ok(oid.to_string())
}

fn repository_head(root: &Path) -> Result<String, VaultLayoutError> {
    let repo = Repository::open(root)?;
    let revision = repo.head()?.peel_to_commit()?.id().to_string();
    Ok(revision)
}

fn walk_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<VaultFileDigest>,
) -> Result<(), VaultLayoutError> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path == root.join(".git") {
            continue;
        }
        if path.is_dir() {
            walk_files(root, &path, output)?;
            continue;
        }
        let bytes = fs::read(&path)?;
        let relative = path
            .strip_prefix(root)
            .expect("walked path must remain under root")
            .to_string_lossy()
            .replace('\\', "/");
        output.push(VaultFileDigest {
            path: relative,
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: bytes.len() as u64,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn note_locator_is_readable_for_ascii_and_unicode_titles() {
        let id = Uuid::parse_str("018f8a60-7d21-7f34-b9f1-2d479f583621").unwrap();
        assert_eq!(
            note_relative_path("H100 Xid 79 triage", id),
            "notes/h100-xid-79-triage--018f8a60.md"
        );
        assert_eq!(
            note_relative_path("서버 냉각 점검", id),
            "notes/서버-냉각-점검--018f8a60.md"
        );
    }

    #[test]
    fn tenant_repositories_are_isolated_and_domains_share_one_repo() {
        let root = tempfile::tempdir().unwrap();
        let layout = TenantVaultLayout::new(root.path());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        let space = Uuid::new_v4();
        let a = layout.open_or_init(tenant_a).unwrap();
        let b = layout.open_or_init(tenant_b).unwrap();
        let server = a.ensure_domain(space, "server-administrator").unwrap();
        let travel = a.ensure_domain(space, "travel-planner").unwrap();
        let cs = a.ensure_domain(space, "computer-science-research").unwrap();

        assert_ne!(a.root(), b.root());
        assert!(server.root.starts_with(a.root()));
        assert!(travel.root.starts_with(a.root()));
        assert!(cs.root.starts_with(a.root()));
        assert!(!server.root.starts_with(b.root()));
        assert_eq!(
            Repository::open(a.root()).unwrap().workdir(),
            Some(a.root().as_path())
        );
    }

    #[test]
    fn lock_serializes_same_tenant_and_not_other_tenants() {
        let root = tempfile::tempdir().unwrap();
        let layout = TenantVaultLayout::new(root.path());
        let first = layout.open_or_init(Uuid::new_v4()).unwrap();
        let second = layout.open_or_init(Uuid::new_v4()).unwrap();
        let held = first.acquire_lock(Duration::from_millis(50)).unwrap();
        assert!(matches!(
            first.acquire_lock(Duration::from_millis(30)),
            Err(VaultLayoutError::LockTimeout(_))
        ));
        assert!(second.acquire_lock(Duration::from_millis(30)).is_ok());
        drop(held);
        assert!(first.acquire_lock(Duration::from_millis(30)).is_ok());
    }

    #[test]
    fn concurrent_domain_ensure_is_serialized_and_restore_is_identical() {
        let root = tempfile::tempdir().unwrap();
        let restore = tempfile::tempdir().unwrap();
        let tenant = Uuid::new_v4();
        let space = Uuid::new_v4();
        let repository = Arc::new(
            TenantVaultLayout::new(root.path())
                .open_or_init(tenant)
                .unwrap(),
        );
        let barrier = Arc::new(Barrier::new(3));
        let mut threads = Vec::new();
        for bundle in ["server-administrator", "travel-planner"] {
            let repository = repository.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                repository.ensure_domain(space, bundle).unwrap()
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }

        let before = repository.snapshot().unwrap();
        let restored = repository.clone_to(restore.path()).unwrap();
        let after = restored.snapshot().unwrap();
        assert_eq!(before.tenant_id, after.tenant_id);
        assert_eq!(before.layout_version, after.layout_version);
        assert_eq!(before.git_head, after.git_head);
        assert_eq!(before.files, after.files);
        assert_eq!(before.checksum, after.checksum);
        assert!(restored
            .root()
            .join(format!("spaces/{space}/domains/server-administrator/notes"))
            .is_dir());
    }

    #[test]
    fn invalid_bundle_id_never_reaches_filesystem() {
        let root = tempfile::tempdir().unwrap();
        let layout = TenantVaultLayout::new(root.path());
        let tenant = Uuid::new_v4();
        assert!(layout
            .domain_root(tenant, Uuid::new_v4(), "../escape")
            .is_err());
        assert!(!layout.tenant_root(tenant).exists());
    }

    #[cfg(unix)]
    #[test]
    fn note_operations_reject_a_symlinked_parent_directory() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let layout = TenantVaultLayout::new(root.path());
        let repository = layout.open_or_init(Uuid::new_v4()).unwrap();
        let space = Uuid::new_v4();
        let domain = repository
            .ensure_domain(space, "server-administrator")
            .unwrap();
        fs::remove_dir(domain.root.join("notes")).unwrap();
        symlink(outside.path(), domain.root.join("notes")).unwrap();

        let error = repository
            .write_note(
                space,
                "server-administrator",
                &format!("notes/{}.md", Uuid::new_v4()),
                b"must stay inside the Vault",
                "test",
            )
            .unwrap_err();
        assert!(matches!(error, VaultLayoutError::NoteSymlink(_)));
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}
