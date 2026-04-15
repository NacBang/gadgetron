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
`codex:codex-rescue` chief-advisor persona in `.claude/agents/`).

gstack is **recommended**, not mandatory — contributors who choose not to
install it should document any ad-hoc browser automation they use so the
team can triage drift.
