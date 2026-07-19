"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Bot,
  Boxes,
  CheckCircle2,
  GitBranch,
  ListChecks,
  MessagesSquare,
  Play,
  RefreshCw,
  ShieldCheck,
  Wrench,
} from "lucide-react";
import { toast } from "sonner";

import { useAuth } from "../../lib/auth-context";
import { useI18n } from "../../lib/i18n";
import {
  createLegacyPolicyRevision,
  fetchActivePolicy,
  fetchPolicyDecisions,
  previewPolicy,
  type EvidenceState,
  type LegacyGadgetMode,
  type LegacyGadgetsConfig,
  type OutcomeState,
  type PolicyDecision,
  type PolicyDecisionEvent,
  type PolicyEnforcementCoverage,
  type PolicyEffect,
  type PolicyPreviewResponse,
  type PolicyResponse,
  type PolicyRisk,
  type RollbackState,
} from "../../lib/policy";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { EmptyState, InlineNotice } from "../workbench";

const MODE_DECISION: Record<LegacyGadgetMode, PolicyDecision> = {
  auto: "auto",
  ask: "review",
  never: "deny",
};
const DECISION_MODE: Record<PolicyDecision, LegacyGadgetMode> = {
  auto: "auto",
  review: "ask",
  deny: "never",
};

const DECISION_STYLE: Record<PolicyDecision, string> = {
  auto: "border-emerald-700/50 bg-emerald-950/20 text-emerald-200",
  review: "border-amber-700/50 bg-amber-950/20 text-amber-200",
  deny: "border-red-700/50 bg-red-950/20 text-red-200",
};

function title(value: string): string {
  return value
    .split(/[_-]/)
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

function StatusCard({ label, value, tone = "neutral" }: {
  label: string;
  value: string;
  tone?: "neutral" | "warn";
}) {
  return (
    <div className={`rounded border p-4 ${tone === "warn" ? "border-amber-800/60 bg-amber-950/15" : "border-zinc-800 bg-zinc-950/50"}`}>
      <div className="text-xs font-medium uppercase tracking-[0.12em] text-zinc-500">{label}</div>
      <div className={`mt-2 text-xl font-semibold ${tone === "warn" ? "text-amber-200" : "text-zinc-100"}`}>{value}</div>
    </div>
  );
}

const COVERAGE_PATHS: Array<{
  key: keyof Omit<PolicyEnforcementCoverage, "overall">;
  label: string;
  icon: typeof Wrench;
}> = [
  { key: "tool_calls", label: "Tool calls", icon: Wrench },
  { key: "background_jobs", label: "Background jobs", icon: Bot },
  { key: "bundle_gadgets", label: "Bundle Gadgets", icon: Boxes },
  { key: "review_resume", label: "Review resume", icon: MessagesSquare },
];

function EnforcementCoverage({ coverage }: { coverage: PolicyEnforcementCoverage }) {
  return (
    <section className="rounded border border-zinc-800 bg-zinc-950/40" aria-labelledby="enforcement-coverage-heading">
      <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-3">
        <h2 id="enforcement-coverage-heading" className="text-sm font-semibold text-zinc-100">Enforcement coverage</h2>
        <span className={`rounded-full border px-2.5 py-1 text-xs font-semibold ${
          coverage.overall === "enforced"
            ? "border-emerald-700/50 bg-emerald-950/20 text-emerald-200"
            : "border-amber-700/50 bg-amber-950/20 text-amber-200"
        }`}>
          {coverage.overall === "enforced" ? "4 / 4 Enforced" : `${COVERAGE_PATHS.filter(({ key }) => coverage[key] === "enforced").length} / 4 Enforced`}
        </span>
      </div>
      <div className="grid sm:grid-cols-2 xl:grid-cols-4">
        {COVERAGE_PATHS.map(({ key, label, icon: Icon }) => {
          const enforced = coverage[key] === "enforced";
          return (
            <div key={key} className="flex items-center gap-3 border-b border-zinc-800 px-4 py-4 sm:border-r xl:border-b-0 last:border-r-0">
              <span className={`grid size-9 place-items-center rounded ${enforced ? "bg-emerald-950/30 text-emerald-300" : "bg-amber-950/30 text-amber-300"}`}>
                <Icon className="size-4" aria-hidden />
              </span>
              <div>
                <div className="text-sm font-medium text-zinc-200">{label}</div>
                <div className={`mt-0.5 text-xs font-semibold ${enforced ? "text-emerald-300" : "text-amber-300"}`}>
                  {enforced ? "Enforced" : "Unavailable"}
                </div>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function authorizationLabel(event: PolicyDecisionEvent): string {
  switch (event.authorization) {
    case "pending_review": return "Review · Pending";
    case "approved_review": return "Review · Approved";
    case "denied": return "Deny";
    case "auto": return "Auto";
    default: return title(event.decision);
  }
}

function DecisionLedger({ decisions, refreshing, unavailable, onRefresh }: {
  decisions: PolicyDecisionEvent[];
  refreshing: boolean;
  unavailable: boolean;
  onRefresh: () => void;
}) {
  return (
    <section className="overflow-hidden rounded border border-zinc-800 bg-zinc-950/40" aria-labelledby="decision-ledger-heading">
      <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-3">
        <h2 id="decision-ledger-heading" className="text-sm font-semibold text-zinc-100">Recent decisions</h2>
        <Button variant="outline" size="sm" onClick={onRefresh} disabled={refreshing} aria-label="Refresh recent decisions">
          <RefreshCw className={refreshing ? "animate-spin" : ""} aria-hidden />
          Refresh
        </Button>
      </div>
      {unavailable ? (
        <div className="grid h-28 place-items-center text-sm font-semibold text-amber-300">Decision ledger unavailable</div>
      ) : decisions.length === 0 ? (
        <div className="grid h-28 place-items-center text-sm font-medium text-zinc-500">No decisions yet</div>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full min-w-[720px] text-left text-sm">
            <thead className="border-b border-zinc-800 bg-zinc-900/50 text-xs uppercase tracking-[0.08em] text-zinc-500">
              <tr>
                <th className="px-4 py-2.5 font-medium">Decision</th>
                <th className="px-4 py-2.5 font-medium">Action</th>
                <th className="px-4 py-2.5 font-medium">Path</th>
                <th className="px-4 py-2.5 font-medium">Revision</th>
                <th className="px-4 py-2.5 font-medium">Time</th>
              </tr>
            </thead>
            <tbody>
              {decisions.map((event) => (
                <tr key={event.event_id} className="border-b border-zinc-800/80 last:border-b-0">
                  <td className="px-4 py-3 font-semibold text-zinc-100">{authorizationLabel(event)}</td>
                  <td className="max-w-[280px] px-4 py-3">
                    <details>
                      <summary className="cursor-pointer truncate font-medium text-zinc-200">{event.input.action_id}</summary>
                      <dl className="mt-3 grid gap-1.5 text-xs text-zinc-500">
                        <div><dt className="inline">Reason: </dt><dd className="inline text-zinc-400">{event.trace.reason}</dd></div>
                        <div className="font-mono"><dt className="inline">Input: </dt><dd className="inline break-all">{event.input_hash}</dd></div>
                        <div className="font-mono"><dt className="inline">Trace: </dt><dd className="inline break-all">{event.trace_hash}</dd></div>
                      </dl>
                    </details>
                  </td>
                  <td className="px-4 py-3 text-zinc-400">{title(event.enforcement_path)}</td>
                  <td className="px-4 py-3 font-mono text-zinc-400">{event.policy.revision}</td>
                  <td className="whitespace-nowrap px-4 py-3 text-zinc-400">{new Date(event.created_at).toLocaleString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function DecisionControl({
  label,
  value,
  disabled,
  allowed = ["auto", "review", "deny"],
  onChange,
}: {
  label: string;
  value: PolicyDecision;
  disabled?: boolean;
  allowed?: readonly PolicyDecision[];
  onChange: (value: PolicyDecision) => void;
}) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-zinc-800 px-4 py-3 last:border-b-0">
      <span className="text-sm font-medium text-zinc-200">{label}</span>
      <div className="grid grid-cols-3 overflow-hidden rounded border border-zinc-700" role="group" aria-label={`${label} decision`}>
        {(["auto", "review", "deny"] as const).map((decision) => (
          <button
            key={decision}
            type="button"
            aria-label={`${label}: ${title(decision)}`}
            aria-pressed={value === decision}
            disabled={disabled || !allowed.includes(decision)}
            onClick={() => onChange(decision)}
            className={`border-r border-zinc-700 px-3 py-1.5 text-xs font-medium last:border-r-0 disabled:cursor-not-allowed disabled:opacity-40 ${
              value === decision ? DECISION_STYLE[decision] : "bg-zinc-950 text-zinc-500 hover:bg-zinc-900"
            }`}
          >
            {title(decision)}
          </button>
        ))}
      </div>
    </div>
  );
}

function LegacyEditor({ policy, draft, saving, onChange, onSave }: {
  policy: PolicyResponse;
  draft: LegacyGadgetsConfig;
  saving: boolean;
  onChange: (next: LegacyGadgetsConfig) => void;
  onSave: () => void;
}) {
  const source = policy.policy.legacy_modes;
  const dirty = source ? JSON.stringify(source) !== JSON.stringify(draft) : false;
  const rows = [
    ["Write default", "default_mode"],
    ["Knowledge write", "wiki_write"],
    ["Infrastructure write", "infra_write"],
    ["Scheduler write", "scheduler_write"],
    ["Provider changes", "provider_mutate"],
  ] as const;
  const updateWrite = (key: typeof rows[number][1], decision: PolicyDecision) => {
    onChange({
      ...draft,
      write: { ...draft.write, [key]: DECISION_MODE[decision] },
    });
  };

  return (
    <section className="overflow-hidden rounded border border-zinc-800 bg-zinc-950/40" aria-labelledby="compatibility-policy-heading">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-zinc-800 px-4 py-3">
        <div className="flex items-center gap-2">
          <GitBranch className="size-4 text-[#D89B5A]" aria-hidden />
          <h2 id="compatibility-policy-heading" className="text-sm font-semibold text-zinc-100">Compatibility decisions</h2>
        </div>
        <Button disabled={!dirty || saving} size="sm" onClick={onSave} className="bg-[#B87333] text-white hover:bg-[#9f622b]">
          {saving ? <RefreshCw className="animate-spin" aria-hidden /> : <GitBranch aria-hidden />}
          Create revision
        </Button>
      </div>
      <DecisionControl label="Read" value="auto" disabled onChange={() => undefined} />
      {rows.map(([label, key]) => (
        <DecisionControl
          key={key}
          label={label}
          value={MODE_DECISION[draft.write[key]]}
          disabled={saving}
          onChange={(decision) => updateWrite(key, decision)}
        />
      ))}
      {Object.entries(draft.write.namespace_modes ?? {}).sort(([a], [b]) => a.localeCompare(b)).map(([namespace, mode]) => (
        <DecisionControl
          key={namespace}
          label={`${title(namespace)} namespace`}
          value={MODE_DECISION[mode]}
          disabled={saving}
          onChange={(decision) => onChange({
            ...draft,
            write: {
              ...draft.write,
              namespace_modes: { ...(draft.write.namespace_modes ?? {}), [namespace]: DECISION_MODE[decision] },
            },
          })}
        />
      ))}
      <DecisionControl
        label="Destructive"
        value={draft.destructive.enabled ? "review" : "deny"}
        disabled={saving}
        allowed={["review", "deny"]}
        onChange={(decision) => {
          if (decision === "auto") return;
          onChange({ ...draft, destructive: { ...draft.destructive, enabled: decision === "review" } });
        }}
      />
    </section>
  );
}

function FieldSelect<T extends string>({ label, value, values, onChange }: {
  label: string;
  value: T;
  values: readonly T[];
  onChange: (value: T) => void;
}) {
  return (
    <label className="rounded border border-zinc-800 bg-zinc-950/40 p-3">
      <span className="block text-xs font-medium uppercase tracking-[0.1em] text-zinc-500">{label}</span>
      <select
        aria-label={label}
        value={value}
        onChange={(event) => onChange(event.target.value as T)}
        className="mt-2 h-9 w-full rounded border border-zinc-700 bg-zinc-950 px-2 text-sm font-medium text-zinc-100 outline-none focus:border-[#B87333]"
      >
        {values.map((option) => <option key={option} value={option}>{title(option)}</option>)}
      </select>
    </label>
  );
}

function DecisionPreview({ result }: { result: PolicyPreviewResponse }) {
  const trace = result.trace;
  return (
    <section className={`rounded border p-4 ${DECISION_STYLE[trace.decision]}`} aria-live="polite" data-testid="policy-preview-result">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <div className="text-xs font-medium uppercase tracking-[0.12em] opacity-70">Decision</div>
          <div className="mt-1 text-2xl font-semibold">{title(trace.decision)}</div>
        </div>
        <div className="rounded border border-current/30 px-3 py-1.5 text-sm">Revision {trace.policy.revision}</div>
      </div>
      <p className="mt-3 text-sm font-medium">{trace.reason}</p>
      <ol className="mt-4 grid gap-2 md:grid-cols-2">
        {trace.steps.map((step, index) => (
          <li key={`${step.stage}-${step.rule_id ?? index}`} className="rounded border border-current/20 bg-black/10 p-3">
            <div className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.08em]">
              {step.matched ? <CheckCircle2 className="size-4" aria-hidden /> : <ListChecks className="size-4 opacity-60" aria-hidden />}
              {step.rule_id ?? title(step.stage)}
            </div>
            <div className="mt-1 text-xs opacity-80">{step.matched ? step.reason : `Skipped · ${step.failed_predicates.join(", ")}`}</div>
          </li>
        ))}
      </ol>
      <details className="mt-4 border-t border-current/20 pt-3 text-xs">
        <summary className="cursor-pointer font-medium">Technical details</summary>
        <dl className="mt-3 grid gap-2 font-mono opacity-75">
          <div><dt className="inline">Policy hash: </dt><dd className="inline break-all">{trace.policy.document_hash}</dd></div>
          <div><dt className="inline">Input hash: </dt><dd className="inline break-all">{trace.input_hash}</dd></div>
          <div><dt className="inline">Trace hash: </dt><dd className="inline break-all">{result.trace_hash}</dd></div>
        </dl>
      </details>
    </section>
  );
}

export function PolicyWorkspace() {
  const { apiKey } = useAuth();
  const { labels } = useI18n();
  const [policy, setPolicy] = useState<PolicyResponse | null>(null);
  const [draft, setDraft] = useState<LegacyGadgetsConfig | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [result, setResult] = useState<PolicyPreviewResponse | null>(null);
  const [decisions, setDecisions] = useState<PolicyDecisionEvent[]>([]);
  const [decisionsUnavailable, setDecisionsUnavailable] = useState(false);
  const [refreshingDecisions, setRefreshingDecisions] = useState(false);
  const [actionId, setActionId] = useState("wiki.write");
  const [namespace, setNamespace] = useState("wiki");
  const [effect, setEffect] = useState<PolicyEffect>("write");
  const [risk, setRisk] = useState<PolicyRisk>("low");
  const [evidence, setEvidence] = useState<EvidenceState>("sufficient");
  const [outcome, setOutcome] = useState<OutcomeState>("verifiable");
  const [rollback, setRollback] = useState<RollbackState>("available");
  const [hasScope, setHasScope] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [next, ledger] = await Promise.all([
        fetchActivePolicy(apiKey),
        fetchPolicyDecisions(apiKey).then(
          (value) => ({ value, unavailable: false }),
          () => ({ value: { decisions: [], count: 0 }, unavailable: true }),
        ),
      ]);
      setPolicy(next);
      setDraft(next.policy.legacy_modes ?? null);
      setDecisions(ledger.value.decisions);
      setDecisionsUnavailable(ledger.unavailable);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Policy could not be loaded.");
    } finally {
      setLoading(false);
    }
  }, [apiKey]);

  useEffect(() => { void load(); }, [load]);

  const refreshDecisions = useCallback(async () => {
    setRefreshingDecisions(true);
    try {
      const ledger = await fetchPolicyDecisions(apiKey);
      setDecisions(ledger.decisions);
      setDecisionsUnavailable(false);
    } catch (caught) {
      setDecisionsUnavailable(true);
      setError(caught instanceof Error ? caught.message : "Decisions could not be refreshed.");
    } finally {
      setRefreshingDecisions(false);
    }
  }, [apiKey]);

  const save = useCallback(async () => {
    if (!policy || !draft) return;
    setSaving(true);
    setError(null);
    try {
      const next = await createLegacyPolicyRevision(apiKey, policy.policy.revision, draft);
      setPolicy(next);
      setDraft(next.policy.legacy_modes ?? null);
      setResult(null);
      toast.success(`Policy revision ${next.policy.revision} created`);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Policy revision was not created.");
    } finally {
      setSaving(false);
    }
  }, [apiKey, draft, policy]);

  const runPreview = useCallback(async () => {
    setPreviewing(true);
    setError(null);
    try {
      const next = await previewPolicy(apiKey, {
        action_id: actionId,
        gadget_name: actionId,
        namespace,
        effect,
        risk,
        requested_scopes: ["management"],
        actor_scopes: hasScope ? ["management"] : [],
        evidence: { state: evidence, references: evidence === "missing" ? [] : ["preview:evidence"] },
        outcome: { state: outcome, ...(outcome === "verifiable" ? { predicate_ref: "preview-outcome-v1" } : {}) },
        rollback: { state: rollback, ...(rollback === "available" ? { compensating_action: `${namespace}.rollback` } : {}) },
      });
      setResult(next);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Decision preview failed.");
    } finally {
      setPreviewing(false);
    }
  }, [actionId, apiKey, effect, evidence, hasScope, namespace, outcome, risk, rollback]);

  const ruleCount = useMemo(
    () => policy?.policy.document.rules.filter((rule) => rule.enabled).length ?? 0,
    [policy],
  );

  if (loading && !policy) return <div className="h-52 animate-pulse rounded border border-zinc-800 bg-zinc-900/50" />;
  if (!policy) return <InlineNotice tone="error" title="Policy unavailable">{error ?? "No policy revision is available."}</InlineNotice>;

  return (
    <div className="space-y-4" data-testid="policy-workspace">
      {error && <InlineNotice tone="error" title="Policy action failed">{error}</InlineNotice>}
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <StatusCard label="Active revision" value={`Revision ${policy.policy.revision}`} />
        <StatusCard label="Default decision" value={title(policy.policy.document.default_decision)} />
        <StatusCard label="Enabled rules" value={String(ruleCount)} />
        <StatusCard
          label="Enforcement coverage"
          value={policy.enforcement_coverage.overall === "enforced" ? "Enforced" : "Unavailable"}
          tone={policy.enforcement_coverage.overall === "enforced" ? "neutral" : "warn"}
        />
      </div>

      <EnforcementCoverage coverage={policy.enforcement_coverage} />

      {draft ? (
        <LegacyEditor policy={policy} draft={draft} saving={saving} onChange={setDraft} onSave={() => void save()} />
      ) : (
        <InlineNotice tone="info" title="Typed policy revision">This revision has no legacy compatibility snapshot.</InlineNotice>
      )}

      <section className="rounded border border-zinc-800 bg-zinc-950/40 p-4" aria-labelledby="decision-preview-heading">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <ShieldCheck className="size-4 text-[#D89B5A]" aria-hidden />
            <h2 id="decision-preview-heading" className="text-sm font-semibold text-zinc-100">Decision preview</h2>
          </div>
          <Button onClick={() => void runPreview()} disabled={previewing} className="bg-[#B87333] text-white hover:bg-[#9f622b]">
            {previewing ? <RefreshCw className="animate-spin" aria-hidden /> : <Play aria-hidden />}
            Evaluate decision
          </Button>
        </div>
        <div className="mt-4 grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <label className="rounded border border-zinc-800 bg-zinc-950/40 p-3">
            <span className="block text-xs font-medium uppercase tracking-[0.1em] text-zinc-500">Action</span>
            <Input aria-label="Action" value={actionId} onChange={(event) => setActionId(event.target.value)} className="mt-2" />
          </label>
          <label className="rounded border border-zinc-800 bg-zinc-950/40 p-3">
            <span className="block text-xs font-medium uppercase tracking-[0.1em] text-zinc-500">Namespace</span>
            <Input aria-label="Namespace" value={namespace} onChange={(event) => setNamespace(event.target.value)} className="mt-2" />
          </label>
          <FieldSelect label="Effect" value={effect} values={["read", "write", "destructive"]} onChange={setEffect} />
          <FieldSelect label="Risk" value={risk} values={["unrated", "low", "medium", "high", "critical"]} onChange={setRisk} />
          <FieldSelect label="Evidence" value={evidence} values={["missing", "sufficient", "stale", "contradictory"]} onChange={setEvidence} />
          <FieldSelect label="Outcome" value={outcome} values={["missing", "verifiable"]} onChange={setOutcome} />
          <FieldSelect label="Rollback" value={rollback} values={["unknown", "unavailable", "available"]} onChange={setRollback} />
          <button
            type="button"
            aria-pressed={hasScope}
            onClick={() => setHasScope((current) => !current)}
            className={`rounded border p-3 text-left ${hasScope ? "border-emerald-700/50 bg-emerald-950/20" : "border-red-700/50 bg-red-950/20"}`}
          >
            <span className="block text-xs font-medium uppercase tracking-[0.1em] text-zinc-500">Management scope</span>
            <span className={`mt-2 block text-sm font-semibold ${hasScope ? "text-emerald-200" : "text-red-200"}`}>{hasScope ? "Present" : "Missing"}</span>
          </button>
        </div>
      </section>

      {result ? (
        <DecisionPreview result={result} />
      ) : (
        <EmptyState
          title={labels.emptyStates.policyPreviewTitle}
          description={labels.emptyStates.policyPreviewDescription}
        />
      )}

      <DecisionLedger
        decisions={decisions}
        refreshing={refreshingDecisions}
        unavailable={decisionsUnavailable}
        onRefresh={() => void refreshDecisions()}
      />
    </div>
  );
}
