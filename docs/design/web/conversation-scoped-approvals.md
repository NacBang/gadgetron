# Conversation Scoped Approvals

> **Owner**: @gateway-router-lead
> **Status**: Draft
> **Created**: 2026-05-04
> **Last Updated**: 2026-05-04
> **Related Crates**: `gadgetron-core`, `gadgetron-gateway`, `gadgetron-penny`, `gadgetron-web`
> **Phase**: [P2]

---

## 1. Philosophy & Concept

Approval and rejection cards must be one decision rendered in two places:

- inline in chat when the approval belongs to the active conversation.
- in the right-side Actions queue for all pending approvals in the tenant.

The current implementation has a real `ApprovalStore`, `ApprovalRequest`, pending list endpoint, approve endpoint, deny endpoint, and side-panel cards. It cannot safely render inline chat cards because `ApprovalRequest` does not carry `conversation_id`. Guessing ownership from `args.host_id`, `action_id`, or the currently visible bundle would create false positives.

The goal is to attach conversation scope at the approval creation seam, not in the UI.

Alternatives considered:

- **UI heuristic matching**: fastest, but unsafe. A server bash approval could be injected into the wrong conversation if two tabs discuss the same host.
- **Only global Actions queue**: safe, but loses the conversational context of "Penny asked this, here is why".
- **Recommended: optional approval context on the backend entity**: preserves one canonical approval record and enables inline rendering only when the backend proves ownership.

## 2. Detailed Implementation Plan

### 2.1 Public API

Add a small context object to `gadgetron-core::workbench::approval`.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalContext {
    pub conversation_id: Option<String>,
    pub request_id: Option<uuid::Uuid>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub subject_title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub id: uuid::Uuid,
    pub action_id: String,
    pub gadget_name: Option<String>,
    pub args: serde_json::Value,
    pub requested_by_user_id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub state: ApprovalState,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    pub resolved_by_user_id: Option<uuid::Uuid>,
    pub deny_reason: Option<String>,
    #[serde(default)]
    pub context: Option<ApprovalContext>,
}
```

Preserve the existing constructor and add an explicit context variant:

```rust
impl ApprovalRequest {
    pub fn new_pending(
        id: uuid::Uuid,
        actor: &AuthenticatedContext,
        action_id: impl Into<String>,
        gadget_name: Option<String>,
        args: serde_json::Value,
    ) -> Self {
        Self::new_pending_with_context(id, actor, action_id, gadget_name, args, None)
    }

    pub fn new_pending_with_context(
        id: uuid::Uuid,
        actor: &AuthenticatedContext,
        action_id: impl Into<String>,
        gadget_name: Option<String>,
        args: serde_json::Value,
        context: Option<ApprovalContext>,
    ) -> Self;
}
```

Extend direct workbench action invocation:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeWorkbenchActionRequest {
    pub args: serde_json::Value,
    #[serde(default)]
    pub client_invocation_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub approval_context: Option<ApprovalContext>,
}
```

The pending approval list should emit the same JSON with `context`, because `ApprovalRequest` is already serialized by `GET /api/v1/web/workbench/approvals/pending`.

For external MCP forwarding through `POST /v1/tools/{name}/invoke`, add optional headers:

```text
X-Gadgetron-Conversation-Id: <conversation_id>
X-Gadgetron-Request-Id: <uuid>
```

The handler should pass these into the approval creation path when a tool is gated.

### 2.2 Internal Structure

Backend creation seams:

- `gadgetron-gateway/src/web/action_service.rs`
  - Use `request.approval_context` when creating `ApprovalRequest`.
- `gadgetron-penny/src/gadget_registry.rs`
  - For in-process `wait_for_approval`, accept an optional `ApprovalContext` from the dispatch/session layer. If the context is not available yet, leave it `None`.
- `gadgetron-gateway/src/handlers.rs`
  - For `/v1/tools/{name}/invoke`, parse optional conversation headers and attach them to the forwarded approval path.
- `gadgetron-core/src/activity_bus.rs`
  - Extend `ApprovalResolved` only if the UI needs live deletion without polling. First pass can keep the 5-second poll.

Frontend rendering seams:

- Extract a shared `ApprovalCard` component from `EvidencePane`.
- Side panel renders every pending approval.
- Chat page fetches pending approvals and filters:

```ts
function approvalBelongsToConversation(
  approval: PendingApproval,
  activeConversationId: string | null,
): boolean {
  return Boolean(
    activeConversationId &&
      approval.context?.conversation_id === activeConversationId,
  );
}
```

- Inline cards call the same approve/deny endpoints as the side panel.
- A resolved click removes the card from both surfaces through shared fetch refresh or polling.

### 2.3 Configuration Schema

No TOML change.

Existing approval modes continue to control whether an approval is created:

```toml
[agent.gadgets.write]
server_admin = "ask"
wiki_write = "ask"
```

### 2.4 Errors & Logging

No new `GadgetronError` variant is required.

New tracing fields:

```rust
tracing::info!(
    target: "workbench.approval",
    %approval_id,
    conversation_id = approval.context.as_ref().and_then(|c| c.conversation_id.as_deref()).unwrap_or("<none>"),
    action_id = %approval.action_id,
    "approval created"
);
```

STRIDE summary:

| Asset | Trust Boundary | Threat | Mitigation |
|---|---|---|---|
| Approval decision | Browser to gateway | Wrong user resolves another tenant approval | Existing tenant boundary in `ApprovalStore::mark_*`. |
| Conversation id | Browser/MCP client to gateway | Client lies about conversation ownership | Treat context as display scope only; authorization remains tenant and action policy. |
| Approval args | Backend to DOM | Sensitive payload disclosure | Keep existing compact summary and expandable JSON; add redaction follow-up before broad T3 rollout. |
| Inline card | Pending queue to chat | Wrong chat shows approval | Render inline only on exact backend `context.conversation_id` match. |
| Double click | Browser retry to gateway | Confusing duplicate decision | Existing 409 `AlreadyResolved`; frontend treats 409 as resolved and refreshes. |

### 2.5 Dependencies

No new dependency.

### 2.6 Service Startup / Delivery Path

No service startup change.

Verification path:

```bash
cargo test -p gadgetron-core workbench::approval
cargo test -p gadgetron-gateway approval
cd crates/gadgetron-web/web
npm run test -- EvidencePane.test.tsx ChatPageSubject.test.tsx
```

Full stack smoke:

```bash
./scripts/launch.sh full
```

## 3. Module Integration

Data flow:

```text
Chat request with conversation_id
  -> Penny / gadget dispatch
  -> Ask-mode approval creation
  -> ApprovalRequest.context.conversation_id
  -> GET /workbench/approvals/pending
  -> right panel Actions queue
  -> chat inline card when context matches active conversation
  -> POST approve/deny
  -> one ApprovalStore state transition
```

Direct workbench action flow:

```text
Bundle UI action
  -> POST /workbench/actions/:action_id { args, approval_context }
  -> ApprovalRequest.context
  -> same pending queue and decision endpoints
```

Cross-domain contracts:

- @gateway-router-lead owns HTTP shape and approval lifecycle.
- @ux-interface-lead owns inline vs side-panel card placement.
- @security-compliance-lead reviews client-supplied context and payload rendering.
- @qa-test-architect owns backend and frontend regression tests.
- @chief-architect owns core type placement and constructor compatibility.

D-12 crate boundary compliance:

- `ApprovalContext` belongs in `gadgetron-core` because it is serialized by the shared approval entity.
- Gateway remains the HTTP owner.
- Web does not define an independent backend approval type beyond its local wire adapter.
- Penny may pass context into the registry, but does not own the approval store type.

Graph verification:

- `graphify query ApprovalRequest` shows `approval.rs` in community 21 and gateway approval handlers in community 2.
- `graphify explain approve_action` reports community 2 and calls to `require_workbench`, `.mark_approved`, `.resume_approval`, `.publish`, and `publish_action_activity`.
- `graphify explain .new_pending` reports community 21 and connections to `.invoke()`, `fresh_request()`, `new_pending_populates_fields()`, and `ApprovalRequest`.
- `graphify path approve_action ApprovalRequest` currently resolves through extracted containment, which is enough to confirm the approval lifecycle crosses `gadgetron-gateway` and `gadgetron-core`; detailed Rust type edges are not fully extracted.

## 4. Unit Test Plan

### 4.1 Test Scope

Rust:

- `ApprovalRequest::new_pending` keeps `context == None`.
- `ApprovalRequest::new_pending_with_context` stores all context fields.
- `InMemoryApprovalStore::list_pending` preserves context in returned rows.
- `InProcessWorkbenchActionService::invoke` persists `request.approval_context`.
- `/v1/tools/{name}/invoke` parses optional conversation headers when creating approval context.

Frontend:

- `approvalBelongsToConversation` returns true only for exact active conversation id.
- Inline card and side-panel card call the same approve/deny endpoint.
- A 409 response refreshes pending approvals instead of showing a scary failure.
- An approval with no context appears only in the side panel.

### 4.2 Test Harness

Rust uses existing unit tests in:

- `crates/gadgetron-core/src/workbench/approval.rs`
- `crates/gadgetron-gateway/src/web/approval_store.rs`
- `crates/gadgetron-gateway/src/web/action_service.rs`
- `crates/gadgetron-gateway/src/handlers.rs`

Frontend uses Vitest mocks for fetch, auth, and active conversation id.

### 4.3 Coverage Goal

Cover every new context-bearing constructor and every UI placement branch:

- no context.
- context for active conversation.
- context for another conversation.
- malformed or missing context in API payload.

## 5. Integration Test Plan

### 5.1 Integration Scope

E2E smoke:

- Create a conversation-scoped approval.
- Confirm side panel Actions shows it.
- Confirm chat inline card appears only in the matching conversation.
- Approve inline.
- Confirm side panel removes the same approval.
- Switch to a different conversation and confirm no inline card appears.

### 5.2 Test Environment

First pass can use mocked pending approval responses in Playwright. Full-stack approval creation requires a gadget/action configured as `ask`; run it under `./scripts/launch.sh full` after the backend change lands.

### 5.3 Regression Prevention

Tests should fail if:

- frontend matches approvals by host id or action id instead of `context.conversation_id`.
- approving inline creates a second decision entity.
- no-context approvals disappear from the global Actions queue.
- direct workbench approvals drop `approval_context`.

### 5.4 Operational Verification

Manual smoke path:

1. Start full stack with `./scripts/launch.sh full`.
2. Configure one write bucket as `ask`.
3. Start a Penny conversation from a log finding or server.
4. Trigger a write/admin tool that requires approval.
5. Confirm the approval appears in chat and side panel.
6. Approve once.
7. Confirm the duplicate surface clears without a second click.

## 6. Phase Boundary

- [P2] Optional approval context on the approval record.
- [P2] Inline cards only for exact conversation match.
- [P2] Side panel remains the canonical global queue.
- [P3] Durable Postgres approval store is deferred unless multi-instance support is pulled forward.
- [P3] "Allow always" client memory remains deferred because it reopens the localStorage security findings from ADR-P2A-06.

## 7. Open Issues / Decisions Needed

| ID | Issue | Options | Recommendation | Status |
|---|---|---|---|---|
| Q-1 | Should `/v1/tools/{name}/invoke` accept conversation headers now? | A: yes / B: only direct workbench actions first | A | Draft recommendation |
| Q-2 | Should inline chat cards poll or subscribe? | A: reuse 5s poll / B: extend activity bus event | A for first pass | Draft recommendation |
| Q-3 | Should approval context include subject title? | A: yes display only / B: conversation id only | A | Draft recommendation |

---

## Review Log

### Round 1 - 2026-05-04 - @gateway-router-lead @ux-interface-lead

**Conclusion**: Pending

**Checklist**:

- [ ] Interface contract
- [ ] One backend entity
- [ ] Inline/global placement

**Action Items**:

- A1: Confirm whether conversation headers on `/v1/tools/{name}/invoke` are needed in the first implementation batch.

### Round 1.5 - 2026-05-04 - @security-compliance-lead

**Conclusion**: Pending

### Round 2 - 2026-05-04 - @qa-test-architect

**Conclusion**: Pending

### Round 3 - 2026-05-04 - @chief-architect

**Conclusion**: Pending
