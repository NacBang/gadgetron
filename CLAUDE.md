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
