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
        }
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
            action_ids: vec!["knowledge-search".into()],
            required_scope: None,
            disabled_reason: None,
        }];

        let actions = vec![WorkbenchActionDescriptor {
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
        }];

        Self {
            views,
            actions,
            allow_direct_actions: true,
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
        assert_eq!(actions.len(), 1);
        assert!(actions[0].disabled_reason.is_none());
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
        // Actions are NOT stripped — they remain in the list, but carry disabled_reason.
        assert_eq!(actions.len(), 1, "action count must be unchanged");
        let a = &actions[0];
        assert!(
            a.disabled_reason.is_some(),
            "disabled_reason must be set when direct actions are off"
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

    #[test]
    fn visible_actions_not_disabled_when_direct_actions_on() {
        let catalog = DescriptorCatalog::seed_p2b().with_allow_direct_actions(true);
        let actions = catalog.visible_actions(&openai_scopes());
        assert!(actions[0].disabled_reason.is_none());
    }
}
