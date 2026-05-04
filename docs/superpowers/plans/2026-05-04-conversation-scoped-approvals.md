# Conversation Scoped Approvals Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render approval cards inline in the active Penny conversation only when the backend approval entity carries the matching conversation id, while keeping the right-side Actions queue as the global canonical queue.

**Architecture:** Add optional `ApprovalContext` to the shared `ApprovalRequest` entity, pass it through direct workbench actions and MCP tool forwarding, then share one frontend approval card between chat and the side panel.

**Tech Stack:** Rust, axum, serde, existing `ApprovalStore`, React, TypeScript, Vitest, Playwright.

---

## File Structure

- Modify: `crates/gadgetron-core/src/workbench/approval.rs`
  - Add `ApprovalContext` and constructor preserving backward compatibility.
- Modify: `crates/gadgetron-core/src/workbench/mod.rs`
  - Re-export `ApprovalContext`; add `approval_context` to `InvokeWorkbenchActionRequest`.
- Modify: `crates/gadgetron-gateway/src/web/action_service.rs`
  - Persist `approval_context` on direct-action approval creation.
- Modify: `crates/gadgetron-gateway/src/handlers.rs`
  - Parse optional conversation headers on `/v1/tools/{name}/invoke`.
- Modify: `crates/gadgetron-penny/src/gadget_registry.rs`
  - Preserve `new_pending` compatibility; add context-aware path only where session context is available.
- Create: `crates/gadgetron-web/web/app/components/shell/approval-card.tsx`
  - Shared card for side panel and inline chat.
- Modify: `crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx`
  - Use shared card for global queue.
- Modify: `crates/gadgetron-web/web/app/(shell)/page.tsx`
  - Render inline cards filtered by active conversation id.

## Task 1: Add ApprovalContext In Core

**Files:**
- Modify: `crates/gadgetron-core/src/workbench/approval.rs`
- Modify: `crates/gadgetron-core/src/workbench/mod.rs`

- [ ] **Step 1: Write failing core tests**

In `approval.rs` tests, add:

```rust
#[test]
fn new_pending_defaults_context_to_none() {
    let actor = AuthenticatedContext::system();
    let req = ApprovalRequest::new_pending(
        Uuid::new_v4(),
        &actor,
        "wiki-write",
        Some("wiki.write".into()),
        serde_json::json!({"name": "foo"}),
    );
    assert!(req.context.is_none());
}

#[test]
fn new_pending_with_context_populates_context() {
    let actor = AuthenticatedContext::system();
    let context = ApprovalContext {
        conversation_id: Some("conv-1".into()),
        request_id: Some(Uuid::new_v4()),
        subject_kind: Some("log_finding".into()),
        subject_id: Some("finding-1".into()),
        subject_title: Some("SMART pending sectors".into()),
    };
    let req = ApprovalRequest::new_pending_with_context(
        Uuid::new_v4(),
        &actor,
        "server-bash",
        Some("server.bash".into()),
        serde_json::json!({"command": "true"}),
        Some(context.clone()),
    );
    assert_eq!(req.context, Some(context));
}
```

- [ ] **Step 2: Run test and confirm it fails**

Run:

```bash
cargo test -p gadgetron-core workbench::approval
```

Expected: fails because `ApprovalContext` and `context` do not exist.

- [ ] **Step 3: Implement core types**

In `approval.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalContext {
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub request_id: Option<Uuid>,
    #[serde(default)]
    pub subject_kind: Option<String>,
    #[serde(default)]
    pub subject_id: Option<String>,
    #[serde(default)]
    pub subject_title: Option<String>,
}
```

Add to `ApprovalRequest`:

```rust
#[serde(default)]
pub context: Option<ApprovalContext>,
```

Replace constructor body with:

```rust
pub fn new_pending(
    id: Uuid,
    actor: &AuthenticatedContext,
    action_id: impl Into<String>,
    gadget_name: Option<String>,
    args: serde_json::Value,
) -> Self {
    Self::new_pending_with_context(id, actor, action_id, gadget_name, args, None)
}

pub fn new_pending_with_context(
    id: Uuid,
    actor: &AuthenticatedContext,
    action_id: impl Into<String>,
    gadget_name: Option<String>,
    args: serde_json::Value,
    context: Option<ApprovalContext>,
) -> Self {
    Self {
        id,
        action_id: action_id.into(),
        gadget_name,
        args,
        requested_by_user_id: actor.real_user_id.unwrap_or(actor.api_key_id),
        tenant_id: actor.tenant_id,
        state: ApprovalState::Pending,
        created_at: chrono::Utc::now(),
        resolved_at: None,
        resolved_by_user_id: None,
        deny_reason: None,
        context,
    }
}
```

In `mod.rs`, change:

```rust
pub use approval::{ApprovalContext, ApprovalError, ApprovalRequest, ApprovalState, ApprovalStore};
```

- [ ] **Step 4: Add `approval_context` to direct action request**

In `InvokeWorkbenchActionRequest`:

```rust
#[serde(default)]
pub approval_context: Option<ApprovalContext>,
```

- [ ] **Step 5: Update compile errors in tests**

Every test constructing `InvokeWorkbenchActionRequest` must add:

```rust
approval_context: None,
```

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p gadgetron-core workbench::approval
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/gadgetron-core/src/workbench/approval.rs crates/gadgetron-core/src/workbench/mod.rs
git commit -m "feat(core): add approval context"
```

## Task 2: Persist Approval Context In Gateway

**Files:**
- Modify: `crates/gadgetron-gateway/src/web/action_service.rs`
- Modify: `crates/gadgetron-gateway/src/handlers.rs`

- [ ] **Step 1: Add action service test**

In `action_service.rs`, near the existing approval-store tests, add:

```rust
#[tokio::test]
async fn invoke_persists_approval_context() {
    use crate::web::approval_store::InMemoryApprovalStore;
    use gadgetron_core::workbench::{
        ApprovalContext, ApprovalStore, WorkbenchActionDescriptor, WorkbenchActionKind,
        WorkbenchActionPlacement,
    };

    let actor = AuthenticatedContext::system();
    let store: Arc<dyn ApprovalStore> = Arc::new(InMemoryApprovalStore::new());
    let descriptor = WorkbenchActionDescriptor {
        id: "needs-approval".into(),
        title: "Needs Approval".into(),
        owner_bundle: "test".into(),
        source_kind: "test".into(),
        source_id: "test".into(),
        gadget_name: Some("wiki.write".into()),
        placement: WorkbenchActionPlacement::Item,
        kind: WorkbenchActionKind::Mutation,
        input_schema: serde_json::json!({"type": "object"}),
        destructive: false,
        requires_approval: true,
        knowledge_hint: "test".into(),
        required_scope: None,
        disabled_reason: None,
    };
    let svc = service_with_descriptor_and_store(descriptor, store.clone());
    let context = ApprovalContext {
        conversation_id: Some("conv-1".into()),
        request_id: None,
        subject_kind: Some("log_finding".into()),
        subject_id: Some("finding-1".into()),
        subject_title: Some("SMART pending sectors".into()),
    };

    let resp = svc.invoke(
        &actor,
        &[],
        "needs-approval",
        InvokeWorkbenchActionRequest {
            args: serde_json::json!({"name": "x"}),
            client_invocation_id: None,
            approval_context: Some(context.clone()),
        },
    ).await.unwrap();

    let approval_id = resp.result.approval_id.unwrap();
    let stored = store.get(approval_id).await.unwrap();
    assert_eq!(stored.context, Some(context));
}
```

Use the existing local helper names in the file. If no helper creates a descriptor catalog with a store, adapt the nearest existing `approval_store` test setup instead of introducing a new abstraction.

- [ ] **Step 2: Run test and confirm it fails**

Run:

```bash
cargo test -p gadgetron-gateway invoke_persists_approval_context
```

Expected: fails because context is not passed into `new_pending`.

- [ ] **Step 3: Persist context**

In `action_service.rs`, replace the approval creation call with:

```rust
let record = ApprovalRequest::new_pending_with_context(
    approval_id,
    actor,
    action_id,
    descriptor.gadget_name.clone(),
    request.args.clone(),
    request.approval_context.clone(),
);
```

- [ ] **Step 4: Parse optional tool invocation headers**

In `handlers.rs`, extend `invoke_tool_handler` arguments with:

```rust
headers: axum::http::HeaderMap,
```

Parse:

```rust
let conversation_id = headers
    .get("x-gadgetron-conversation-id")
    .and_then(|v| v.to_str().ok())
    .filter(|v| !v.is_empty())
    .map(str::to_string);
```

If the dispatcher cannot accept this context yet, leave a narrow comment at the call site and add the header test in the Penny wiring task. Do not invent a parallel dispatch trait in this task.

- [ ] **Step 5: Run gateway tests**

Run:

```bash
cargo test -p gadgetron-gateway approval
cargo test -p gadgetron-gateway invoke_persists_approval_context
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/gadgetron-gateway/src/web/action_service.rs crates/gadgetron-gateway/src/handlers.rs
git commit -m "feat(gateway): persist approval conversation context"
```

## Task 3: Share Approval Card In Web

**Files:**
- Create: `crates/gadgetron-web/web/app/components/shell/approval-card.tsx`
- Modify: `crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx`
- Test: `crates/gadgetron-web/web/__tests__/workbench/EvidencePane.test.tsx`

- [ ] **Step 1: Add side-panel card test with context payload**

Mock `fetch` for `/workbench/approvals/pending` to return:

```json
{
  "approvals": [
    {
      "id": "00000000-0000-0000-0000-000000000001",
      "action_id": "server.bash",
      "gadget_name": "server.bash",
      "args": { "host_id": "host-1", "command": "systemctl restart x" },
      "created_at": "2026-05-04T00:00:00Z",
      "context": { "conversation_id": "conv-1", "subject_title": "SMART pending sectors" }
    }
  ]
}
```

Assert the Actions tab renders `SMART pending sectors` and `server.bash`.

- [ ] **Step 2: Create shared card component**

Create `approval-card.tsx` with props:

```tsx
export interface PendingApproval {
  id: string;
  actionId: string;
  gadgetName: string | null;
  args: unknown;
  createdAt: string;
  context?: {
    conversation_id?: string | null;
    request_id?: string | null;
    subject_kind?: string | null;
    subject_id?: string | null;
    subject_title?: string | null;
  } | null;
}

export function ApprovalCard({
  approval,
  hostMap,
  onApprove,
  onDeny,
  compact = false,
}: {
  approval: PendingApproval;
  hostMap?: Record<string, string>;
  onApprove: (approvalId: string) => Promise<void>;
  onDeny: (approvalId: string) => Promise<void>;
  compact?: boolean;
}) {
  // Move the current card body here, replacing Korean labels with:
  // "Full args", "Collapse", "Approve", "Deny".
}
```

Move existing summary helpers from `evidence-pane.tsx` into this file unless they are still used elsewhere.

- [ ] **Step 3: Wire side panel**

In `evidence-pane.tsx`, import:

```ts
import { ApprovalCard, type PendingApproval } from "./approval-card";
```

Remove the local `PendingApproval` interface and `ApprovalCard` implementation. Keep `usePendingApprovalsFeed` in the side panel for now.

- [ ] **Step 4: Run web tests**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- EvidencePane.test.tsx
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/gadgetron-web/web/app/components/shell/approval-card.tsx crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx crates/gadgetron-web/web/__tests__/workbench/EvidencePane.test.tsx
git commit -m "feat(web): share approval card component"
```

## Task 4: Render Inline Conversation Approvals

**Files:**
- Modify: `crates/gadgetron-web/web/app/(shell)/page.tsx`
- Modify: `crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx`
- Test: `crates/gadgetron-web/web/__tests__/workbench/ChatPageSubject.test.tsx`

- [ ] **Step 1: Extract pending approvals hook**

Create or move to `crates/gadgetron-web/web/app/lib/pending-approvals.ts`:

```ts
import { useCallback, useEffect, useState } from "react";
import type { PendingApproval } from "../components/shell/approval-card";

export function approvalBelongsToConversation(
  approval: PendingApproval,
  activeConversationId: string | null,
): boolean {
  return Boolean(
    activeConversationId &&
      approval.context?.conversation_id === activeConversationId,
  );
}

export function usePendingApprovals(apiKey: string | null): {
  approvals: PendingApproval[];
  refresh: () => Promise<void>;
} {
  const [approvals, setApprovals] = useState<PendingApproval[]>([]);
  const refresh = useCallback(async () => {
    const res = await fetch("/api/v1/web/workbench/approvals/pending", {
      credentials: "include",
      headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
    });
    if (!res.ok) return;
    const body = await res.json();
    setApprovals((body.approvals ?? []).map((r: Record<string, unknown>) => ({
      id: String(r.id),
      actionId: String(r.action_id),
      gadgetName: typeof r.gadget_name === "string" ? r.gadget_name : null,
      args: r.args,
      createdAt: String(r.created_at),
      context: (r.context as PendingApproval["context"]) ?? null,
    })));
  }, [apiKey]);
  useEffect(() => {
    void refresh();
    const timer = setInterval(() => void refresh(), 5000);
    return () => clearInterval(timer);
  }, [refresh]);
  return { approvals, refresh };
}
```

Adjust `getApiBase` handling to match the existing meta-base helper before committing; the snippet shows the target shape, not the final base-url detail.

- [ ] **Step 2: Add filtering unit tests**

In `ChatPageSubject.test.tsx`, test:

```tsx
expect(approvalBelongsToConversation(
  { id: "a", actionId: "x", gadgetName: null, args: {}, createdAt: "", context: { conversation_id: "conv-1" } },
  "conv-1",
)).toBe(true);

expect(approvalBelongsToConversation(
  { id: "a", actionId: "x", gadgetName: null, args: {}, createdAt: "", context: { conversation_id: "conv-2" } },
  "conv-1",
)).toBe(false);
```

- [ ] **Step 3: Render inline cards**

In `page.tsx`, use:

```tsx
const { apiKey } = useAuth();
const { activeConversationId } = useWorkbenchSubject();
const { approvals, refresh } = usePendingApprovals(apiKey ?? null);
const inlineApprovals = approvals.filter((a) =>
  approvalBelongsToConversation(a, activeConversationId),
);
```

Render `inlineApprovals` near the active subject banner, not inside assistant message history:

```tsx
{inlineApprovals.length > 0 && (
  <section data-testid="inline-approvals" className="mx-auto w-full max-w-3xl space-y-2 px-4">
    {inlineApprovals.map((approval) => (
      <ApprovalCard
        key={approval.id}
        approval={approval}
        onApprove={async (id) => {
          await decideApproval(id, true, apiKey ?? null);
          await refresh();
        }}
        onDeny={async (id) => {
          await decideApproval(id, false, apiKey ?? null);
          await refresh();
        }}
        compact
      />
    ))}
  </section>
)}
```

Move `decideApproval` into the same hook module so the side panel and chat use one endpoint path.

- [ ] **Step 4: Run tests**

Run:

```bash
cd crates/gadgetron-web/web
npm run test -- ChatPageSubject.test.tsx EvidencePane.test.tsx
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/gadgetron-web/web/app/'(shell)'/page.tsx crates/gadgetron-web/web/app/lib/pending-approvals.ts crates/gadgetron-web/web/app/components/shell/evidence-pane.tsx crates/gadgetron-web/web/__tests__/workbench/ChatPageSubject.test.tsx
git commit -m "feat(web): render conversation scoped approvals inline"
```

## Final Verification

Run:

```bash
cargo test -p gadgetron-core workbench::approval
cargo test -p gadgetron-gateway approval
cd crates/gadgetron-web/web
npm run test -- EvidencePane.test.tsx ChatPageSubject.test.tsx
npm run build
npm run e2e -- workbench.spec.ts
```

Expected: all pass.
