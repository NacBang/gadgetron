//! Descriptor catalog — actor-aware view/action filtering.
//!
//! Authority: `docs/design/gateway/workbench-projection-and-actions.md` §2.2.2
//!
//! [`DescriptorCatalog`] holds a snapshot of registered view and action
//! descriptors. It is designed to be cheap to clone (all `Vec` members) and
//! replaces the hot-reload BundleRegistry path (deferred to W3-BUN-1).
//!
//! P2B ships with a single hand-coded seed catalog (`seed_p2b`) for
//! end-to-end testing.

use gadgetron_core::{
    context::Scope,
    workbench::{
        WorkbenchActionDescriptor, WorkbenchActionKind, WorkbenchActionPlacement,
        WorkbenchRendererKind, WorkbenchViewDescriptor, WorkbenchViewPlacement,
    },
};

/// Bundled catalog + pre-compiled validators. The unit atomically
/// swapped into the runtime's `Arc<ArcSwap<CatalogSnapshot>>` handle on
/// every `POST /admin/reload-catalog` call. Building a snapshot
/// compiles JSON-Schema validators for every action in the catalog
/// (see [`DescriptorCatalog::into_snapshot`]). Readers access
/// `snapshot.catalog` and `snapshot.validators` through one
/// `ArcSwap::load` call so they can never observe catalog/validator
/// drift.
///
/// Kept outside `DescriptorCatalog` because validators are a derived,
/// cache-like artifact — the catalog itself is fine to clone / edit
/// in memory without touching them.
#[derive(Debug, Clone)]
pub struct CatalogSnapshot {
    pub catalog: DescriptorCatalog,
    pub validators: std::collections::HashMap<String, std::sync::Arc<jsonschema::Validator>>,
}

/// Bundle-level metadata attached to a catalog file. Optional —
/// `seed_p2b()` and legacy catalog files don't carry it, and the
/// reload path treats its absence as "unnamed anonymous catalog".
///
/// Shipped in ISSUE 9 TASK 9.1 as the first step toward real bundle
/// manifests. Future TASKs will layer per-bundle scope isolation,
/// multi-bundle aggregation, and signed manifests on top of this
/// same struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BundleMetadata {
    /// Bundle identifier — matches the directory / install-slug. Like
    /// a Cargo crate name: `snake_case`, stable across versions. Used
    /// as a primary key when multiple bundles are installed.
    pub id: String,
    /// Bundle version string, free-form (semver recommended but not
    /// enforced today). Surfaced in the reload response so operators
    /// can tell "did my edit actually land?" at a glance.
    pub version: String,
    /// Bundle-level scope floor (ISSUE 10 TASK 10.3). When set,
    /// every view and action the bundle ships inherits this as
    /// their minimum required scope — actors without the scope see
    /// NONE of the bundle's descriptors. Overrides per-descriptor
    /// `required_scope = None` (descriptors with their own stricter
    /// scope keep that stricter scope). Typical use: a bundle that
    /// only makes sense for operators sets
    /// `required_scope = "Management"` once instead of repeating
    /// the scope on every descriptor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_scope: Option<gadgetron_core::context::Scope>,
}

/// On-disk catalog file shape consumed by
/// [`DescriptorCatalog::from_toml_file`] (ISSUE 8 TASK 8.4).
///
/// ```toml
/// # Optional bundle metadata (ISSUE 9 TASK 9.1). Absent = anonymous.
/// [bundle]
/// id = "gadgetron-core"
/// version = "0.1.0"
///
/// allow_direct_actions = true  # optional, default true
///
/// [[views]]
/// id = "my-view"
/// title = "My view"
/// # ... full WorkbenchViewDescriptor fields
///
/// [[actions]]
/// id = "my-action"
/// title = "My action"
/// # ... full WorkbenchActionDescriptor fields
/// input_schema = { type = "object", properties = {} }
/// ```
///
/// Field names match the serde-derived shape of
/// `WorkbenchViewDescriptor` / `WorkbenchActionDescriptor` in
/// `gadgetron-core::workbench` — consult those struct docs for the
/// authoritative field list.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CatalogFile {
    #[serde(default)]
    pub bundle: Option<BundleMetadata>,
    #[serde(default)]
    pub allow_direct_actions: Option<bool>,
    #[serde(default)]
    pub views: Vec<WorkbenchViewDescriptor>,
    #[serde(default)]
    pub actions: Vec<WorkbenchActionDescriptor>,
}

/// Snapshot of registered workbench descriptors with actor-aware filtering.
///
/// Clone is O(n) over descriptors but the catalog is expected to be small
/// (tens of entries). No `Arc` indirection needed for P2B.
#[derive(Debug, Clone)]
pub struct DescriptorCatalog {
    pub(crate) views: Vec<WorkbenchViewDescriptor>,
    pub(crate) actions: Vec<WorkbenchActionDescriptor>,
    /// When `false`, action listings add a `disabled_reason` and
    /// `POST /actions/:id` returns 403.
    allow_direct_actions: bool,
    /// Bundle metadata when this catalog came from a bundle manifest
    /// (ISSUE 9 TASK 9.1). `None` for the hand-coded `seed_p2b()`
    /// catalog and for anonymous flat-TOML files that didn't declare
    /// a `[bundle]` table.
    pub(crate) bundle: Option<BundleMetadata>,
    /// Bundles that contributed to this catalog when built via
    /// [`Self::from_bundle_dir`] (ISSUE 9 TASK 9.2). Empty in all
    /// other constructors. A merged catalog has `bundle = None` AND
    /// `bundles = [...]`; a single-bundle catalog has
    /// `bundle = Some(meta)` AND `bundles = []`.
    pub(crate) bundles: Vec<BundleMetadata>,
}

impl DescriptorCatalog {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Empty catalog — no views, no actions.
    pub fn empty() -> Self {
        Self {
            views: vec![],
            actions: vec![],
            allow_direct_actions: true,
            bundle: None,
            bundles: Vec::new(),
        }
    }

    /// Read-only access to the bundle metadata carried by this
    /// catalog. `Some` when the catalog came from a `[bundle]`-
    /// declaring TOML file (ISSUE 9 TASK 9.1); `None` otherwise.
    pub fn bundle(&self) -> Option<&BundleMetadata> {
        self.bundle.as_ref()
    }

    /// Hand-coded seed catalog for P2B integration testing.
    ///
    /// Contains **one view** (`knowledge-activity-recent`) and **one action**
    /// (`knowledge-search`) wired to the KC-1c coordinator.
    pub fn seed_p2b() -> Self {
        let views = vec![WorkbenchViewDescriptor {
            id: "knowledge-activity-recent".into(),
            title: "최근 활동".into(),
            owner_bundle: "core".into(),
            source_kind: "activity".into(),
            source_id: "recent".into(),
            placement: WorkbenchViewPlacement::LeftRail,
            renderer: WorkbenchRendererKind::Timeline,
            data_endpoint: "/api/v1/web/workbench/views/knowledge-activity-recent/data".into(),
            refresh_seconds: Some(5),
            action_ids: vec![
                "knowledge-search".into(),
                "wiki-list".into(),
                "wiki-read".into(),
                "wiki-write".into(),
                "wiki-delete".into(),
            ],
            required_scope: None,
            disabled_reason: None,
        }];

        // Four actions today — the full wiki CRUD loop via workbench.
        // Each is gadget-backed so the dispatcher (PR #175) routes to
        // `KnowledgeGadgetProvider` and returns real results in
        // `WorkbenchActionResult.payload`. This is what turns the
        // workbench API from "canned 200 OK" into a product users can
        // actually drive.
        let actions = vec![
            WorkbenchActionDescriptor {
                id: "knowledge-search".into(),
                title: "지식 검색".into(),
                owner_bundle: "core".into(),
                source_kind: "gadget".into(),
                source_id: "wiki.search".into(),
                gadget_name: Some("wiki.search".into()),
                placement: WorkbenchActionPlacement::CenterMain,
                kind: WorkbenchActionKind::Query,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "minLength": 1, "maxLength": 500 },
                        "max_results": { "type": "integer", "minimum": 1, "maximum": 20 }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
                destructive: false,
                requires_approval: false,
                knowledge_hint: "wiki.search 가젯을 직접 호출합니다.".into(),
                required_scope: None,
                disabled_reason: None,
            },
            WorkbenchActionDescriptor {
                id: "wiki-list".into(),
                title: "위키 목록".into(),
                owner_bundle: "core".into(),
                source_kind: "gadget".into(),
                source_id: "wiki.list".into(),
                gadget_name: Some("wiki.list".into()),
                placement: WorkbenchActionPlacement::ContextMenu,
                kind: WorkbenchActionKind::Query,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                destructive: false,
                requires_approval: false,
                knowledge_hint: "wiki.list 가젯을 직접 호출합니다.".into(),
                required_scope: None,
                disabled_reason: None,
            },
            WorkbenchActionDescriptor {
                id: "wiki-read".into(),
                title: "위키 읽기".into(),
                owner_bundle: "core".into(),
                source_kind: "gadget".into(),
                source_id: "wiki.get".into(),
                gadget_name: Some("wiki.get".into()),
                placement: WorkbenchActionPlacement::ContextMenu,
                kind: WorkbenchActionKind::Query,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 256 }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
                destructive: false,
                requires_approval: false,
                knowledge_hint: "wiki.get 가젯을 직접 호출합니다.".into(),
                required_scope: None,
                disabled_reason: None,
            },
            WorkbenchActionDescriptor {
                id: "wiki-write".into(),
                title: "위키 쓰기".into(),
                owner_bundle: "core".into(),
                source_kind: "gadget".into(),
                source_id: "wiki.write".into(),
                gadget_name: Some("wiki.write".into()),
                placement: WorkbenchActionPlacement::ContextMenu,
                kind: WorkbenchActionKind::Mutation,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 256 },
                        "content": { "type": "string", "minLength": 0 }
                    },
                    "required": ["name", "content"],
                    "additionalProperties": false
                }),
                destructive: false,
                requires_approval: false,
                knowledge_hint: "wiki.write 가젯을 직접 호출합니다. P2B에서는 승인 흐름이 stub — 직접 기록됩니다.".into(),
                required_scope: None,
                disabled_reason: None,
            },
            WorkbenchActionDescriptor {
                // ISSUE 3 TASK 3.5 adds this as the canonical
                // approval-gated action. `destructive: true` funnels
                // the invoke through step 6 `pending_approval`, which
                // with an ApprovalStore wired (production) persists a
                // real ApprovalRequest. Approve via the approval
                // endpoint → `wiki.delete` dispatches against the
                // wiki.
                id: "wiki-delete".into(),
                title: "위키 삭제".into(),
                owner_bundle: "core".into(),
                source_kind: "gadget".into(),
                source_id: "wiki.delete".into(),
                gadget_name: Some("wiki.delete".into()),
                placement: WorkbenchActionPlacement::ContextMenu,
                kind: WorkbenchActionKind::Dangerous,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "minLength": 1, "maxLength": 256 }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
                destructive: true,
                requires_approval: false,
                knowledge_hint: "wiki.delete 가젯을 소프트 삭제 흐름으로 호출합니다. 승인 후에만 실제 디스패치됩니다.".into(),
                required_scope: None,
                disabled_reason: None,
            },
        ];

        Self {
            views,
            actions,
            allow_direct_actions: true,
            bundle: None,
            bundles: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Builder modifiers
    // -----------------------------------------------------------------------

    /// Override the **instance-level** `allow_direct_actions` policy.
    ///
    /// This is **not** a per-actor access gate. Actor-scope filtering lives
    /// on the descriptor itself (`WorkbenchActionDescriptor.required_scope`)
    /// and is enforced in `visible_actions` via `scope_allowed(..)`.
    ///
    /// | Gate | Layer | When | Effect |
    /// |---|---|---|---|
    /// | `required_scope` | per-descriptor | actor lacks scope | descriptor is **stripped** from the response |
    /// | `allow_direct_actions = false` | per-instance | policy disabled | every descriptor is returned **with** `disabled_reason` set |
    ///
    /// Doc 74 §2.3 + §2.4.1 codify this split; drift audit PR 4
    /// (D-20260418-25) closed U-D as "spec-correct, no code change".
    pub fn with_allow_direct_actions(mut self, allow: bool) -> Self {
        self.allow_direct_actions = allow;
        self
    }

    // -----------------------------------------------------------------------
    // Snapshot construction (ISSUE 8 TASK 8.3)
    // -----------------------------------------------------------------------

    /// Build a `CatalogSnapshot` from this catalog by pre-compiling
    /// JSON-Schema validators for every action. The snapshot is the
    /// unit that gets atomically swapped via
    /// `Arc<ArcSwap<CatalogSnapshot>>` so a concurrent reload replaces
    /// BOTH the catalog and its derived validators in one step — no
    /// reader can observe a new catalog against stale validators.
    pub fn into_snapshot(self) -> CatalogSnapshot {
        let all_scopes = [Scope::OpenAiCompat, Scope::Management, Scope::XaasAdmin];
        let mut validators: std::collections::HashMap<
            String,
            std::sync::Arc<jsonschema::Validator>,
        > = std::collections::HashMap::new();
        for action in self.visible_actions(&all_scopes) {
            match jsonschema::validator_for(&action.input_schema) {
                Ok(v) => {
                    validators.insert(action.id.clone(), std::sync::Arc::new(v));
                }
                Err(e) => {
                    tracing::warn!(
                        action_id = %action.id,
                        error = %e,
                        "catalog snapshot: invalid input_schema; validation skipped"
                    );
                }
            }
        }
        CatalogSnapshot {
            catalog: self,
            validators,
        }
    }

    // -----------------------------------------------------------------------
    // File-based source (ISSUE 8 TASK 8.4)
    // -----------------------------------------------------------------------

    /// Load a catalog from a TOML file on disk.
    ///
    /// Expected format matches the shape of [`CatalogFile`]: an
    /// optional `allow_direct_actions` bool plus `[[views]]` and
    /// `[[actions]]` arrays whose field names match
    /// `WorkbenchViewDescriptor` / `WorkbenchActionDescriptor`
    /// (serde-derived, stable across the `gadgetron-core` crate).
    ///
    /// On any read/parse failure the function returns the error
    /// verbatim — the admin reload handler surfaces this as 500 with
    /// the message so the operator knows the file was malformed and
    /// the old snapshot stays live.
    pub fn from_toml_file(path: &std::path::Path) -> gadgetron_core::error::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            gadgetron_core::error::GadgetronError::Config(format!(
                "workbench catalog: failed to read {path:?}: {e}",
            ))
        })?;
        let file: CatalogFile = toml::from_str(&text).map_err(|e| {
            gadgetron_core::error::GadgetronError::Config(format!(
                "workbench catalog: TOML parse failed for {path:?}: {e}",
            ))
        })?;
        // ISSUE 10 TASK 10.3 — bundle-level scope floor. If the
        // bundle declares `required_scope`, every descriptor that
        // didn't set its own scope inherits the bundle's. Descriptors
        // with their own scope keep theirs (treated as narrower or
        // equivalent to the bundle floor — we don't try to compare).
        let mut views = file.views;
        let mut actions = file.actions;
        if let Some(bundle_scope) = file.bundle.as_ref().and_then(|b| b.required_scope) {
            for v in views.iter_mut() {
                if v.required_scope.is_none() {
                    v.required_scope = Some(bundle_scope);
                }
            }
            for a in actions.iter_mut() {
                if a.required_scope.is_none() {
                    a.required_scope = Some(bundle_scope);
                }
            }
        }
        Ok(Self {
            views,
            actions,
            allow_direct_actions: file.allow_direct_actions.unwrap_or(true),
            bundle: file.bundle,
            bundles: Vec::new(),
        })
    }

    /// Aggregate every `<dir>/<bundle>/bundle.toml` into one catalog
    /// (ISSUE 9 TASK 9.2). Each immediate subdirectory that contains
    /// a `bundle.toml` contributes its views + actions; the merged
    /// `DescriptorCatalog.bundles` field (accessed via
    /// [`Self::contributing_bundles`]) lists every bundle whose
    /// manifest was read, in directory-sort order so the merge is
    /// deterministic across runs.
    ///
    /// Collision policy: duplicate action or view ids across bundles
    /// surface as a hard `Config` error naming the id + the two
    /// bundle ids that declared it. Operators must rename or remove
    /// one before reload succeeds — we never silently pick a winner.
    ///
    /// `allow_direct_actions` is OR-folded across bundles: if ANY
    /// manifest opts in, the merged catalog opts in. This matches
    /// the per-descriptor `disabled_reason` semantics (§2.2.2) where
    /// the instance policy is a floor, not a ceiling.
    ///
    /// Missing / non-directory paths return `Config` errors so a
    /// typo in `bundles_dir` fails loudly rather than silently
    /// loading an empty catalog.
    pub fn from_bundle_dir(dir: &std::path::Path) -> gadgetron_core::error::Result<Self> {
        let md = std::fs::metadata(dir).map_err(|e| {
            gadgetron_core::error::GadgetronError::Config(format!(
                "bundles dir: cannot stat {dir:?}: {e}",
            ))
        })?;
        if !md.is_dir() {
            return Err(gadgetron_core::error::GadgetronError::Config(format!(
                "bundles dir: {dir:?} is not a directory",
            )));
        }

        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| {
                gadgetron_core::error::GadgetronError::Config(format!(
                    "bundles dir: cannot read {dir:?}: {e}",
                ))
            })?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        entries.sort();

        let mut merged_views: Vec<WorkbenchViewDescriptor> = Vec::new();
        let mut merged_actions: Vec<WorkbenchActionDescriptor> = Vec::new();
        let mut merged_bundles: Vec<BundleMetadata> = Vec::new();
        let mut allow_direct = false;
        // action_id → owning bundle id, for collision diagnostics.
        let mut action_owner: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut view_owner: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for subdir in entries {
            let manifest = subdir.join("bundle.toml");
            if !manifest.exists() {
                tracing::debug!(
                    bundle_dir = %subdir.display(),
                    "skipping subdirectory without bundle.toml"
                );
                continue;
            }
            let c = Self::from_toml_file(&manifest)?;
            let bundle_id = c.bundle().map(|b| b.id.clone()).unwrap_or_else(|| {
                subdir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("anonymous")
                    .to_string()
            });
            for v in c.views {
                if let Some(other) = view_owner.get(&v.id) {
                    return Err(gadgetron_core::error::GadgetronError::Config(format!(
                        "bundles dir: duplicate view id {:?} in bundles {:?} and {:?}",
                        v.id, other, bundle_id
                    )));
                }
                view_owner.insert(v.id.clone(), bundle_id.clone());
                merged_views.push(v);
            }
            for a in c.actions {
                if let Some(other) = action_owner.get(&a.id) {
                    return Err(gadgetron_core::error::GadgetronError::Config(format!(
                        "bundles dir: duplicate action id {:?} in bundles {:?} and {:?}",
                        a.id, other, bundle_id
                    )));
                }
                action_owner.insert(a.id.clone(), bundle_id.clone());
                merged_actions.push(a);
            }
            allow_direct = allow_direct || c.allow_direct_actions;
            if let Some(meta) = c.bundle {
                merged_bundles.push(meta);
            }
        }

        Ok(Self {
            views: merged_views,
            actions: merged_actions,
            allow_direct_actions: allow_direct,
            // The merged catalog is NOT a single bundle — leave
            // `bundle` None so `/admin/reload-catalog` reports the
            // multi-bundle set via `bundles: Vec<BundleMetadata>`
            // instead (see `contributing_bundles`).
            bundle: None,
            bundles: merged_bundles,
        })
    }

    /// Bundles that contributed to this catalog. Populated only by
    /// [`Self::from_bundle_dir`]; other constructors leave it empty
    /// so the single-bundle case keeps using `self.bundle()`.
    pub fn contributing_bundles(&self) -> &[BundleMetadata] {
        &self.bundles
    }

    // -----------------------------------------------------------------------
    // Lookup
    // -----------------------------------------------------------------------

    /// Find a view descriptor by id, regardless of scope filtering.
    pub fn find_view(&self, id: &str) -> Option<&WorkbenchViewDescriptor> {
        self.views.iter().find(|v| v.id == id)
    }

    /// Find an action descriptor by id, regardless of scope filtering.
    pub fn find_action(&self, id: &str) -> Option<&WorkbenchActionDescriptor> {
        self.actions.iter().find(|a| a.id == id)
    }

    // -----------------------------------------------------------------------
    // Actor-aware filtering
    // -----------------------------------------------------------------------

    /// Return views visible to an actor that holds `actor_scopes`.
    ///
    /// Descriptors whose `required_scope` exceeds what the actor holds are
    /// stripped entirely (consistent with §2.4.1 — "filtered" → 404, not 403,
    /// to avoid leaking existence).
    pub fn visible_views(&self, actor_scopes: &[Scope]) -> Vec<WorkbenchViewDescriptor> {
        self.views
            .iter()
            .filter(|v| scope_allowed(v.required_scope.as_ref(), actor_scopes))
            .cloned()
            .collect()
    }

    /// Return actions visible to an actor that holds `actor_scopes`.
    ///
    /// When `allow_direct_actions == false`, every action is returned but with
    /// `disabled_reason` set to the policy message (doc §2.2.6).
    ///
    /// Descriptors whose `required_scope` exceeds what the actor holds are
    /// stripped (same as views).
    pub fn visible_actions(&self, actor_scopes: &[Scope]) -> Vec<WorkbenchActionDescriptor> {
        let disable_msg: Option<String> = if !self.allow_direct_actions {
            Some("Direct actions are disabled by instance policy.".into())
        } else {
            None
        };

        self.actions
            .iter()
            .filter(|a| scope_allowed(a.required_scope.as_ref(), actor_scopes))
            .map(|a| {
                if let Some(ref msg) = disable_msg {
                    let mut cloned = a.clone();
                    cloned.disabled_reason = Some(msg.clone());
                    cloned
                } else {
                    a.clone()
                }
            })
            .collect()
    }

    /// Whether direct action invocations are permitted by **instance-level
    /// policy**. Distinct from `required_scope` (per-descriptor actor filter)
    /// — see [`DescriptorCatalog::with_allow_direct_actions`] for the full
    /// dual-gate table.
    pub fn allow_direct_actions(&self) -> bool {
        self.allow_direct_actions
    }
}

/// Returns `true` if the actor's scope set satisfies the descriptor's
/// `required_scope` requirement.
///
/// `None` required_scope means `OpenAiCompat` base only — any authenticated
/// actor satisfies it (actor_scopes is non-empty iff auth passed).
fn scope_allowed(required: Option<&Scope>, actor_scopes: &[Scope]) -> bool {
    match required {
        None => true,
        Some(req) => actor_scopes.contains(req),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gadgetron_core::context::Scope;

    fn openai_scopes() -> Vec<Scope> {
        vec![Scope::OpenAiCompat]
    }

    fn mgmt_scopes() -> Vec<Scope> {
        vec![Scope::Management]
    }

    fn both_scopes() -> Vec<Scope> {
        vec![Scope::OpenAiCompat, Scope::Management]
    }

    // -----------------------------------------------------------------------
    // find_view / find_action — basic lookup
    // -----------------------------------------------------------------------

    #[test]
    fn find_view_hit() {
        let catalog = DescriptorCatalog::seed_p2b();
        let v = catalog.find_view("knowledge-activity-recent");
        assert!(v.is_some(), "seed view must be found");
        assert_eq!(v.unwrap().id, "knowledge-activity-recent");
    }

    #[test]
    fn from_toml_file_round_trips_a_minimal_catalog() {
        // ISSUE 8 TASK 8.4 — file-based catalog source. Write a tiny
        // TOML, parse it, and assert the parsed shape + that
        // `into_snapshot` builds a validator for the described action.
        let dir =
            std::env::temp_dir().join(format!("gadgetron-catalog-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.toml");
        std::fs::write(
            &path,
            r#"
allow_direct_actions = false

[[views]]
id = "v1"
title = "View one"
owner_bundle = "e2e"
source_kind = "activity"
source_id = "recent"
placement = "left_rail"
renderer = "timeline"
data_endpoint = "/x"
action_ids = []

[[actions]]
id = "a1"
title = "Action one"
owner_bundle = "e2e"
source_kind = "gadget"
source_id = "test.ping"
gadget_name = "test.ping"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "t"
input_schema = { type = "object", properties = { n = { type = "integer" } }, required = ["n"], additionalProperties = false }
"#,
        )
        .unwrap();

        let catalog = DescriptorCatalog::from_toml_file(&path).expect("parse ok");
        assert_eq!(catalog.views.len(), 1);
        assert_eq!(catalog.actions.len(), 1);
        assert!(
            !catalog.allow_direct_actions(),
            "allow_direct_actions=false must round-trip"
        );
        assert_eq!(
            catalog.find_action("a1").unwrap().gadget_name.as_deref(),
            Some("test.ping")
        );

        // Snapshotting must compile a validator for the action.
        let snap = catalog.into_snapshot();
        assert!(
            snap.validators.contains_key("a1"),
            "validator must exist for a1 post-snapshot"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn from_toml_file_surfaces_parse_errors() {
        let dir =
            std::env::temp_dir().join(format!("gadgetron-catalog-test-err-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "this is = not valid =====").unwrap();
        let err = DescriptorCatalog::from_toml_file(&path).expect_err("must reject bad toml");
        let msg = format!("{err}");
        assert!(
            msg.contains("TOML parse failed"),
            "error must name the failure; got {msg:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn from_toml_file_round_trips_bundle_metadata() {
        // ISSUE 9 TASK 9.1 — `[bundle]` table surfaces on the
        // catalog so the admin reload response can tell the operator
        // which bundle + version is live.
        let dir =
            std::env::temp_dir().join(format!("gadgetron-catalog-bundle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bundle.toml");
        std::fs::write(
            &path,
            r#"
[bundle]
id = "test-bundle"
version = "9.9.9"

[[actions]]
id = "a1"
title = "a1"
owner_bundle = "e2e"
source_kind = "gadget"
source_id = "t.p"
gadget_name = "t.p"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "x"
input_schema = { type = "object" }
"#,
        )
        .unwrap();

        let catalog = DescriptorCatalog::from_toml_file(&path).expect("parse ok");
        let meta = catalog.bundle().expect("bundle metadata must round-trip");
        assert_eq!(meta.id, "test-bundle");
        assert_eq!(meta.version, "9.9.9");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bundle_required_scope_floors_descriptors_without_their_own() {
        // ISSUE 10 TASK 10.3 — a bundle that declares
        // `required_scope = "Management"` must push that floor down
        // to every descriptor that didn't set one, so OpenAiCompat
        // actors see NONE of the bundle's descriptors.
        let dir =
            std::env::temp_dir().join(format!("gadgetron-bundle-scope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bundle.toml");
        std::fs::write(
            &path,
            r#"
[bundle]
id = "ops-bundle"
version = "1.0.0"
required_scope = "Management"

[[actions]]
id = "ops-action"
title = "ops"
owner_bundle = "ops-bundle"
source_kind = "gadget"
source_id = "ops.ping"
gadget_name = "ops.ping"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "t"
input_schema = { type = "object" }
"#,
        )
        .unwrap();

        let catalog = DescriptorCatalog::from_toml_file(&path).expect("parse ok");

        // OpenAiCompat actor does NOT see the Management-floored
        // descriptor.
        let openai_visible = catalog.visible_actions(&openai_scopes());
        assert!(
            openai_visible.iter().all(|a| a.id != "ops-action"),
            "ops-action must be hidden from OpenAiCompat after bundle scope floor"
        );

        // Management actor DOES see it.
        let mgmt_visible = catalog.visible_actions(&mgmt_scopes());
        assert!(
            mgmt_visible.iter().any(|a| a.id == "ops-action"),
            "ops-action must be visible to Management actor"
        );

        // Per-descriptor scope is now populated on the underlying
        // catalog entry too — downstream audit/log can introspect
        // the effective scope without re-reading the bundle.
        assert_eq!(
            catalog
                .find_action("ops-action")
                .and_then(|a| a.required_scope),
            Some(Scope::Management),
            "descriptor must have inherited Management from the bundle floor"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn seed_p2b_has_no_bundle_metadata() {
        // Hardcoded fallback must stay anonymous so admin tooling
        // can distinguish "no bundle, default" from "loaded from a
        // named bundle on disk".
        assert!(DescriptorCatalog::seed_p2b().bundle().is_none());
    }

    #[test]
    fn first_party_gadgetron_core_bundle_file_loads_cleanly() {
        // ISSUE 9 TASK 9.1 — first-party bundle on disk that mirrors
        // `seed_p2b()`. Operators can point `[web] catalog_path` at
        // this file and get the same catalog as the hardcoded
        // fallback. This test guards the file itself against drift
        // (bad TOML, missing fields, schema-validation mismatch).
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("crate nested under workspace root")
            .to_path_buf();
        let bundle_path = workspace_root.join("bundles/gadgetron-core/bundle.toml");
        let catalog = DescriptorCatalog::from_toml_file(&bundle_path)
            .expect("first-party bundle must parse — if this fails, the bundle file drifted");
        let meta = catalog.bundle().expect("bundle metadata must be present");
        assert_eq!(meta.id, "gadgetron-core");
        // Mirror parity: same action set as seed_p2b().
        let seed = DescriptorCatalog::seed_p2b();
        let seed_ids: std::collections::BTreeSet<_> =
            seed.actions.iter().map(|a| a.id.clone()).collect();
        let bundle_ids: std::collections::BTreeSet<_> =
            catalog.actions.iter().map(|a| a.id.clone()).collect();
        assert_eq!(
            seed_ids, bundle_ids,
            "first-party bundle must ship the same action ids as seed_p2b()"
        );
        // Snapshot must compile: validates schemas round-trip through TOML.
        let _ = catalog.into_snapshot();
    }

    #[test]
    fn from_bundle_dir_merges_multiple_bundles() {
        // ISSUE 9 TASK 9.2 — two bundle subdirectories, each with its
        // own `bundle.toml`, merge into one catalog.
        let root =
            std::env::temp_dir().join(format!("gadgetron-bundles-merge-{}", std::process::id()));
        let b1 = root.join("b1");
        let b2 = root.join("b2");
        std::fs::create_dir_all(&b1).unwrap();
        std::fs::create_dir_all(&b2).unwrap();
        std::fs::write(
            b1.join("bundle.toml"),
            r#"
[bundle]
id = "bundle-one"
version = "1.0.0"

[[actions]]
id = "b1-action"
title = "b1"
owner_bundle = "bundle-one"
source_kind = "gadget"
source_id = "b1.do"
gadget_name = "b1.do"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "t"
input_schema = { type = "object" }
"#,
        )
        .unwrap();
        std::fs::write(
            b2.join("bundle.toml"),
            r#"
[bundle]
id = "bundle-two"
version = "2.0.0"

[[actions]]
id = "b2-action"
title = "b2"
owner_bundle = "bundle-two"
source_kind = "gadget"
source_id = "b2.do"
gadget_name = "b2.do"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "t"
input_schema = { type = "object" }
"#,
        )
        .unwrap();

        let merged = DescriptorCatalog::from_bundle_dir(&root).expect("merge ok");
        let ids: std::collections::BTreeSet<_> =
            merged.actions.iter().map(|a| a.id.clone()).collect();
        assert!(ids.contains("b1-action"));
        assert!(ids.contains("b2-action"));
        // Merged catalogs expose contributing bundles, not a single bundle.
        assert!(merged.bundle().is_none());
        let contribs: std::collections::BTreeSet<_> = merged
            .contributing_bundles()
            .iter()
            .map(|b| b.id.clone())
            .collect();
        assert_eq!(
            contribs,
            ["bundle-one".to_string(), "bundle-two".to_string()]
                .into_iter()
                .collect(),
            "both bundle ids must appear in contributing_bundles"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn from_bundle_dir_rejects_duplicate_action_ids() {
        let root =
            std::env::temp_dir().join(format!("gadgetron-bundles-dup-{}", std::process::id()));
        let b1 = root.join("a1");
        let b2 = root.join("a2");
        std::fs::create_dir_all(&b1).unwrap();
        std::fs::create_dir_all(&b2).unwrap();
        let shared_action = r#"
[[actions]]
id = "duplicate-id"
title = "x"
owner_bundle = "x"
source_kind = "gadget"
source_id = "x.x"
gadget_name = "x.x"
placement = "context_menu"
kind = "query"
destructive = false
requires_approval = false
knowledge_hint = "t"
input_schema = { type = "object" }
"#;
        std::fs::write(
            b1.join("bundle.toml"),
            format!(
                r#"[bundle]
id = "bundle-alpha"
version = "1"
{shared_action}"#
            ),
        )
        .unwrap();
        std::fs::write(
            b2.join("bundle.toml"),
            format!(
                r#"[bundle]
id = "bundle-beta"
version = "1"
{shared_action}"#
            ),
        )
        .unwrap();

        let err =
            DescriptorCatalog::from_bundle_dir(&root).expect_err("collision must surface as error");
        let msg = format!("{err}");
        assert!(
            msg.contains("duplicate action id"),
            "error must name the collision; got {msg:?}"
        );
        assert!(
            msg.contains("duplicate-id"),
            "error must name the colliding id; got {msg:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn from_bundle_dir_rejects_non_directory() {
        let path = std::env::temp_dir().join("gadgetron-bundles-nonexistent-xyz");
        let _ = std::fs::remove_dir_all(&path);
        let err = DescriptorCatalog::from_bundle_dir(&path).expect_err("missing dir must error");
        assert!(format!("{err}").contains("cannot stat"));
    }

    #[test]
    fn from_toml_file_surfaces_missing_file() {
        let path = std::env::temp_dir().join("gadgetron-catalog-missing-file-xyz.toml");
        let _ = std::fs::remove_file(&path);
        let err = DescriptorCatalog::from_toml_file(&path).expect_err("missing file must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("failed to read"),
            "missing-file error must name the failure; got {msg:?}"
        );
    }

    #[test]
    fn find_view_miss() {
        let catalog = DescriptorCatalog::seed_p2b();
        assert!(catalog.find_view("nonexistent-view").is_none());
    }

    #[test]
    fn find_action_hit() {
        let catalog = DescriptorCatalog::seed_p2b();
        let a = catalog.find_action("knowledge-search");
        assert!(a.is_some(), "seed action must be found");
        assert_eq!(a.unwrap().id, "knowledge-search");
    }

    #[test]
    fn find_action_miss() {
        let catalog = DescriptorCatalog::seed_p2b();
        assert!(catalog.find_action("does-not-exist").is_none());
    }

    // -----------------------------------------------------------------------
    // visible_views — scope filtering
    // -----------------------------------------------------------------------

    #[test]
    fn visible_views_no_required_scope_visible_to_all() {
        let catalog = DescriptorCatalog::seed_p2b();
        // Seed view has required_scope = None → visible to OpenAiCompat key.
        let views = catalog.visible_views(&openai_scopes());
        assert_eq!(views.len(), 1);
    }

    #[test]
    fn visible_views_management_required_hidden_from_openai_key() {
        use gadgetron_core::workbench::WorkbenchViewPlacement;
        let mut catalog = DescriptorCatalog::seed_p2b();
        // Inject a management-only view directly.
        catalog.views.push(WorkbenchViewDescriptor {
            id: "admin-view".into(),
            title: "Admin".into(),
            owner_bundle: "ops".into(),
            source_kind: "admin".into(),
            source_id: "sys".into(),
            placement: WorkbenchViewPlacement::LeftRail,
            renderer: WorkbenchRendererKind::Table,
            data_endpoint: "/api/v1/web/workbench/views/admin-view/data".into(),
            refresh_seconds: None,
            action_ids: vec![],
            required_scope: Some(Scope::Management),
            disabled_reason: None,
        });

        // OpenAiCompat key → management-only view is stripped.
        let views = catalog.visible_views(&openai_scopes());
        assert!(
            !views.iter().any(|v| v.id == "admin-view"),
            "management view must be hidden from OpenAiCompat actor"
        );

        // Management key → management view visible.
        let views = catalog.visible_views(&mgmt_scopes());
        assert!(
            views.iter().any(|v| v.id == "admin-view"),
            "management view must be visible to Management actor"
        );
    }

    // -----------------------------------------------------------------------
    // visible_actions — scope + allow_direct_actions
    // -----------------------------------------------------------------------

    #[test]
    fn visible_actions_no_required_scope_visible() {
        let catalog = DescriptorCatalog::seed_p2b();
        let actions = catalog.visible_actions(&openai_scopes());
        // seed_p2b ships 5 actions: knowledge-search, wiki-list,
        // wiki-read, wiki-write, wiki-delete (approval-gated).
        assert_eq!(actions.len(), 5);
        for a in &actions {
            assert!(
                a.disabled_reason.is_none(),
                "no disabled_reason on seed actions by default"
            );
        }
    }

    #[test]
    fn visible_actions_management_required_hidden_from_openai() {
        let mut catalog = DescriptorCatalog::seed_p2b();
        catalog.actions.push(WorkbenchActionDescriptor {
            id: "admin-action".into(),
            title: "Admin Action".into(),
            owner_bundle: "ops".into(),
            source_kind: "admin".into(),
            source_id: "admin.op".into(),
            gadget_name: None,
            placement: WorkbenchActionPlacement::ContextMenu,
            kind: WorkbenchActionKind::Dangerous,
            input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
            destructive: true,
            requires_approval: true,
            knowledge_hint: "admin".into(),
            required_scope: Some(Scope::Management),
            disabled_reason: None,
        });

        let actions = catalog.visible_actions(&openai_scopes());
        assert!(
            !actions.iter().any(|a| a.id == "admin-action"),
            "management action must be hidden from OpenAiCompat actor"
        );
        let actions = catalog.visible_actions(&both_scopes());
        assert!(
            actions.iter().any(|a| a.id == "admin-action"),
            "management action must appear when actor holds Management scope"
        );
    }

    #[test]
    fn visible_actions_disabled_reason_set_when_direct_actions_off() {
        let catalog = DescriptorCatalog::seed_p2b().with_allow_direct_actions(false);
        let actions = catalog.visible_actions(&openai_scopes());
        // Actions are NOT stripped — they remain in the list, but each carries disabled_reason.
        assert_eq!(actions.len(), 5, "action count must be unchanged");
        for a in &actions {
            assert!(
                a.disabled_reason.is_some(),
                "disabled_reason must be set on every action when direct actions are off"
            );
            assert!(
                a.disabled_reason
                    .as_ref()
                    .unwrap()
                    .contains("Direct actions are disabled"),
                "disabled_reason must describe the policy, got: {:?}",
                a.disabled_reason
            );
        }
    }

    #[test]
    fn visible_actions_not_disabled_when_direct_actions_on() {
        let catalog = DescriptorCatalog::seed_p2b().with_allow_direct_actions(true);
        let actions = catalog.visible_actions(&openai_scopes());
        assert!(actions[0].disabled_reason.is_none());
    }
}
