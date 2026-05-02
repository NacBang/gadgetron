# Gadgetron — agent collaboration guide

This file is loaded automatically by Claude Code / Claude Agent SDK when
working inside the Gadgetron repo. It captures team conventions that are
not enforceable by `cargo check` / `cargo clippy` / CI gates alone.

## gstack (recommended)

The Gadgetron team recommends installing and using
[gstack](https://github.com/garrytan/gstack) for any web-browsing / QA
tasks an agent needs to perform while working on this repo.

**Install** (per-machine, one-time):

```sh
git clone --single-branch --depth 1 https://github.com/garrytan/gstack.git \
  ~/.claude/skills/gstack \
  && cd ~/.claude/skills/gstack \
  && ./setup
```

`./setup` requires `bun` — on macOS `brew install oven-sh/bun/bun` is the
preferred install path; other platforms see
<https://bun.sh/install>.

**Usage convention**: once installed, **prefer the `/browse` skill from
gstack** for all headless browser work (navigation, screenshots, DOM
inspection, responsive checks, form / upload / dialog flows). **Avoid**
the `mcp__claude-in-chrome__*` tools — we have standardized on gstack so
that automated QA runs against Gadgetron's future `/web` UI are
reproducible across contributors. If gstack is not installed on your
machine, fall back to minimizing browser automation rather than mixing
toolchains.

### Available gstack slash commands

- `/office-hours`
- `/plan-ceo-review`
- `/plan-eng-review`
- `/plan-design-review`
- `/design-consultation`
- `/design-shotgun`
- `/design-html`
- `/review`
- `/ship`
- `/land-and-deploy`
- `/canary`
- `/benchmark`
- `/browse`
- `/connect-chrome`
- `/qa`
- `/qa-only`
- `/design-review`
- `/setup-browser-cookies`
- `/setup-deploy`
- `/retro`
- `/investigate`
- `/document-release`
- `/codex`
- `/cso`
- `/autoplan`
- `/plan-devex-review`
- `/devex-review`
- `/careful`
- `/freeze`
- `/guard`
- `/unfreeze`
- `/gstack-upgrade`
- `/learn`

The `/review`, `/qa`, `/investigate`, `/benchmark`, and `/document-release`
skills are especially relevant to Gadgetron's workflow — they compose
cleanly with the existing Phase 2A stabilization-sprint review discipline
(see `docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md` and the
chief-advisor persona at `docs/agents/codex-chief-advisor.md`; the
`codex:codex-rescue` gstack skill is the external-opinion entry point).

gstack is **recommended**, not mandatory — contributors who choose not to
install it should document any ad-hoc browser automation they use so the
team can triage drift.

## Routine cycle workflow

Before starting any implementation task, run the following sequence in
order. This keeps the repo state coherent, the AST current, and the PR
gate sharp:

1. **Pull `main`** — `git fetch origin main && git pull --ff-only
   origin main`. Check `git log --oneline HEAD..origin/main` for
   surprises (new ADRs, config schema changes, breaking renames).
2. **Refresh AST** — `graphify update .` (the post-merge hook does
   this automatically on `git pull`; run manually when unsure).
   `graphify-out/GRAPH_REPORT.md` is your first read on "where does
   this live?" questions — see the `## graphify` section.
3. **Audit / polish the harness** — open
   `scripts/e2e-harness/run.sh`, scan for `TODO`, `FIXME`, stale
   gates, hardcoded values that should be env-driven, duplicated
   curl/jq patterns. Improve the harness BEFORE adding new gates —
   a brittle harness produces flaky PR verdicts. Minimum polish
   each cycle: fix one thing you notice.

   Every cycle also re-asks two coverage questions:
   **(a) docs ↔ harness parity** — every endpoint, config key,
   and wire contract documented in `docs/manual/` or
   `docs/design/` should have a harness assertion. Gaps = silent
   drift waiting to happen.
   **(b) implementation coverage** — every code path Gadgetron
   ships (chat Ok/Err, streaming Ok/Err, workbench CRUD, action
   invoke 200/404/403, SSE `[DONE]`, SSE error event, auth 401,
   scope 403, quota 429, admission gates) should have at least
   one gate. If `cargo test` is the only thing that exercises a
   path, consider whether the harness ALSO needs to — integration
   regressions hide in the wire-level gaps unit tests can't see.

   Log gaps as `TODO(harness-coverage-N):` comments in
   `run.sh` and burn them down over subsequent cycles.
4. **Research similar implementations** — before landing any
   non-trivial change, spend a pass on how others solved this. Web
   search, blog posts, GitHub code search, research papers where
   applicable. Capture the key insight in the PR body ("prior art:
   X does Y; we adopt Z because …"). Don't reinvent what a
   well-known pattern already solved; don't copy what doesn't fit.
5. **Team consensus → decide → implement → test → PR → merge**.
   Don't skip 1-4.

Rule: if the cron loop is re-entering a cycle, steps 1-3 are
mandatory every iteration. Step 4 scales with task novelty —
trivial tweaks skip it, architecture changes require it.

## PR gate (E2E harness)

**Every feature PR MUST make `./scripts/e2e-harness/run.sh` green before it
is opened.** No exceptions — if a gate fails, find the root cause and fix
it BEFORE pushing. 통과를 못하면 원인을 파악하여 완전 수정후에 올릴 수
있도록.

The harness boots the full stack (Postgres + wiki + mock OpenAI provider +
`gadgetron serve` + `/web`) and exercises the public API surface with
`curl`, tailing `gadgetron.log` for regressions. See
`scripts/e2e-harness/README.md` for the gate table, artifact layout, and
the "how to add a gate" pattern.

**Recommended workflow**:

```sh
# Before opening a PR:
./scripts/e2e-harness/run.sh           # full run (2-3 min warm)
./scripts/e2e-harness/run.sh --quick   # skip cargo test (~30s)
```

New features SHOULD add a runtime assertion to the harness (a `curl`
against a new endpoint, a grep over `mock-openai.log`, or a new mock
error mode). Keep each gate under 5s of wall time — heavy matrices belong
in `cargo test`, not the smoke layer.

## Skill routing

When the user's request matches an available skill, ALWAYS invoke it using the Skill
tool as your FIRST action. Do NOT answer directly, do NOT use other tools first.
The skill has specialized workflows that produce better results than ad-hoc answers.

Key routing rules:
- Product ideas, "is this worth building", brainstorming → invoke office-hours
- Bugs, errors, "why is this broken", 500 errors → invoke investigate
- Ship, deploy, push, create PR → invoke ship
- QA, test the site, find bugs → invoke qa
- Code review, check my diff → invoke review
- Update docs after shipping → invoke document-release
- Weekly retro → invoke retro
- Design system, brand → invoke design-consultation
- Visual audit, design polish → invoke design-review
- Architecture review → invoke plan-eng-review
- Save progress, checkpoint, resume → invoke checkpoint
- Code quality, health check → invoke health

## graphify

This project maintains a graphify-generated knowledge graph at
`graphify-out/` (community detection + god nodes across 281+
files). It is the fastest way to orient in a codebase this size —
one `GRAPH_REPORT.md` read beats three rounds of speculative grep.

**Hard rules — main agent AND all subagents (Agent-tool spawns
included):**

1. **Before searching for files or referencing a symbol**, open
   `graphify-out/GRAPH_REPORT.md`. Identify the relevant
   *community hub* (e.g. "Auth & Server Core", "Knowledge
   Curation") and *god node* (high-degree symbol inside that
   community). Narrow file lookups to the community's member
   list rather than repo-wide grep.
2. When delegating to a subagent via the `Agent` tool, include in
   the prompt: *"Read `graphify-out/GRAPH_REPORT.md` first to
   find the relevant community + god node, THEN read specific
   files."* This keeps exploration scoped and avoids re-reading
   the corpus.
3. After modifying Rust code in a session, run `graphify
   update .` — the AST fast path keeps the graph current without
   LLM cost. Doc / markdown changes need `/graphify --update`
   (LLM cost).
4. If `graphify-out/wiki/index.md` exists, navigate that instead
   of raw files — the wiki is a curated agent-crawlable surface.

**Hook discipline** — git hooks auto-refresh the graph so
`GRAPH_REPORT.md` stays fresh:

- `post-commit` — AST refresh after each commit (installed by
  `graphify hook install`)
- `post-checkout` — refresh on branch switch
- `post-merge` — refresh on `git pull` / merge (our own hook;
  source at `.githooks/post-merge`)

Contributors who clone fresh should run
`./scripts/install-git-hooks.sh` once. The installer is
idempotent and safe to re-run.

**Fallback** — if the `graphify` CLI is not installed
(`pipx install graphifyy` or `pip install --user graphifyy`),
the hooks and `graphify update` commands silently no-op — never
blocking commits, merges, or pulls. `GRAPH_REPORT.md` is plain
markdown and readable without tooling even when slightly stale.

# Behavioral Guidelines (general LLM coding hygiene)

These four principles apply across any task in this repo, regardless of which IDE / agent surface is loaded. They sit alongside the Gadgetron-specific rules above; when something feels underspecified by either set, prefer the stricter one.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.
