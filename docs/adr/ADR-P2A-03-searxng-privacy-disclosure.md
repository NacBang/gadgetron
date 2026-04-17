# ADR-P2A-03 — SearXNG Query Privacy Disclosure

| Field | Value |
|---|---|
| **Status** | ACCEPTED (v3 — Round 2 review addressed) |
| **Date** | 2026-04-13 |
| **Author** | security-compliance-lead |
| **Parent docs** | `docs/design/phase2/00-overview.md` v2 §8 M7; §10 Compliance, Disclosure 2; `docs/design/phase2/01-knowledge-layer.md` v2 §9 M7 |
| **Pre-merge gate** | `docs/manual/penny.md` MUST contain the verbatim disclosure text before any P2A impl PR merges to `main` |

---

## Context

### What SearXNG does with queries

Penny exposes a `web_search` MCP tool backed by a self-hosted SearXNG instance.
When Claude Code calls `web_search`, the `gadgetron-knowledge` crate sends an HTTP
request to the SearXNG instance configured in `[knowledge.search] searxng_url`.

SearXNG is a privacy-respecting metasearch engine. It acts as a proxy: it receives
the query from Gadgetron, fans it out to configured upstream search engines (Google,
Bing, DuckDuckGo, Brave, and others depending on SearXNG configuration), collects
results, and returns them. SearXNG anonymizes request headers (User-Agent, referrer,
cookies) before forwarding to upstream engines.

**However**: the query text itself is forwarded to those upstream engines. A Google
search for "how to treat thyroid cancer" is received by Google regardless of whether
it was sent from a browser or through SearXNG. SearXNG removes identifying headers,
not the query content.

Additionally, SearXNG itself may log queries depending on how the SearXNG instance
is configured. The Gadgetron-bundled `docker-compose.yml` (Appendix C of
`docs/design/phase2/00-overview.md` v2) runs a default SearXNG configuration; the
logging behavior of that default is not controlled by Gadgetron.

### What Gadgetron does NOT store

Gadgetron does not persist the query text. The audit log records only:
- `tools_called: Vec<String>` — contains the string `"web_search"` when the tool
  was called, nothing more (per M6 in `docs/design/phase2/02-penny-agent.md` v2)
- `penny_dispatched: bool`, `subprocess_duration_ms: i32`

The query text is not in the audit log. The query text is not stored in the wiki
(unless the user or model explicitly writes a wiki page containing it). The wiki
commit messages are abstract (`"auto-commit: <page-name> <timestamp>"`) and do not
contain request content per M5 in `docs/design/phase2/01-knowledge-layer.md` v2 §4.4.

### Correction to v1 design doc

`docs/design/phase2/00-overview.md` v1 (rejected) contained an inaccurate claim
that "search history does not flow to any external party." This was incorrect.
The v2 doc explicitly corrects this:

> Correction to v1 doc: earlier draft claimed "search history does not flow to any
> external party" — this was inaccurate. Corrected here.

Source: `docs/design/phase2/00-overview.md` v2 §8 M7.

### Relevance to GDPR

Under GDPR, the user's search queries are personal data (they reveal intent,
preferences, and potentially special category data such as health queries).

For P2A single-user local deployment, the user is simultaneously data subject and
data controller — no Art. 28 Data Processing Agreement is required between the user
and Gadgetron. However, the user must be informed that their queries are transmitted
to third-party search engines so they can make an informed choice.

The upstream search engines (Google, Bing, etc.) become independent data controllers
when they receive the query. Gadgetron cannot govern what those controllers do with
that data.

Source: `docs/design/phase2/00-overview.md` v2 §10 Compliance (GDPR section).

This disclosure obligation was raised by security-compliance-lead in Round 1.5 review
(SEC-7) and addressed in the v2 design docs. The correction from v1's inaccurate claim
is documented as SEC-7 resolution in
`docs/design/phase2/00-overview.md` v2 Appendix D review provenance.

---

## Decision

### Required disclosure in user manual

Privacy disclosure for SearXNG query routing MUST appear in `docs/manual/penny.md`
before any P2A implementation PR merges to `main`.

**Required verbatim text** (canonical source:
`docs/design/phase2/00-overview.md` v2 §10 Disclosure 2):

> **Privacy note**: Web search via Penny proxies your queries through SearXNG to
> the search engines configured in your SearXNG instance (by default: Google, Bing,
> DuckDuckGo, Brave — but your administrator may have enabled different engines).
> Queries are anonymized at the SearXNG layer, but the search engines receive the
> query text. SearXNG may log queries depending on its own configuration. Gadgetron
> itself does not store your search queries. If you need stricter privacy, disable
> `web_search` by leaving `searxng_url` unset in your config.

This text must appear in `docs/manual/penny.md` under a clearly labeled
"Privacy and Security" or equivalent section. It must appear both in the Korean
version and any English version of the manual (per `feedback_manual_before_push.md`,
the user manual is Korean-primary; an English equivalent is acceptable adjacent to
the Korean text).

Placement guidance:
- The disclosure MUST appear before the "Quick start" section, or at minimum be
  visible in a prominently labeled "Privacy" or "Privacy & Security" subsection
- It MUST NOT be buried in an appendix or footnote
- It SHOULD appear alongside Disclosure 1 (wiki git history permanence) for
  consistency, as both are pre-merge requirements

The companion Disclosure 1 text (also required but owned by the wiki data retention
concern, not this ADR) is:

> **Permanence note**: Every wiki page you (or Penny on your behalf) write is
> committed to a local git repository at `~/.gadgetron/wiki/`. Git history is
> **permanent**. [...]

Both disclosures are pre-merge gate requirements per
`docs/design/phase2/00-overview.md` v2 §10 final paragraph.

### Opt-out mechanism

The disclosure text references the opt-out: leave `searxng_url` unset in
`gadgetron.toml`. This must be implemented.

When `knowledge.search.searxng_url` is not set (or empty string), the `web_search`
MCP tool MUST NOT be registered in the MCP server's tool list. Claude Code will
then not see the tool and will not attempt to call it. This behavior is specified
in `docs/design/phase2/01-knowledge-layer.md` v2 §7 configuration schema:

```toml
[knowledge.search]
# searxng_url = "http://localhost:8080"  # unset = web_search disabled
max_results = 5
timeout_secs = 10
```

When `searxng_url` is unset, `KnowledgeConfig.search` is `None`. The MCP server's
`serve_stdio` constructor checks `if let Some(ref search_cfg) = config.search`
before calling `SearxngClient::new(search_cfg)`. If `None`, no `SearxngClient` is
constructed and `web_search` is not registered in the MCP tool list returned to
Claude Code. (Note: `SearxngClient::new(&SearchConfig) -> Result<Self, SearchError>`
takes a non-optional `SearchConfig` and can fail on invalid URL / TLS init; the
`Option` selection is handled one layer up in `KnowledgeConfig`.)
This is the complete opt-out mechanism — no separate `web_search_enabled` flag is
needed.

### Pre-merge gate enforcement

This ADR designates the presence of the disclosure text in `docs/manual/penny.md`
as a **blocking pre-merge gate** for all P2A implementation PRs.

**What "P2A implementation PR" means**: any pull request that merges code from
the `gadgetron-penny` or `gadgetron-knowledge` crates to `main`. This includes:
- The initial core PR (adding `GadgetronError::Wiki` and `GadgetronError::Penny`
  variants to `gadgetron-core`)
- Any PR adding penny session/stream/spawn code
- Any PR adding knowledge wiki/search/MCP server code

**What enforces this gate**: PR reviewers (PM and security-compliance-lead) MUST
check that `docs/manual/penny.md` exists and contains the required verbatim text
before approving any P2A code PR. This is a manual review step, not an automated CI
check, because manual contents are inherently judgment-dependent.

The check is part of the security-compliance-lead Round 1.5 review checklist
(per `docs/process/03-review-rubric.md §1.5`). No P2A code PR may be approved
without sign-off from security-compliance-lead that the disclosure is present.

---

## Consequences

### For the manual

- `docs/manual/penny.md` must be written BEFORE the first P2A code PR is opened
- The disclosure text is verbatim-locked by this ADR; editorial changes require
  reopening this ADR and updating the parent design doc accordingly
- The manual must accurately describe the opt-out (`searxng_url` unset) so users
  have a genuine choice

### For implementation

- `gadgetron-knowledge/src/search/searxng.rs` — The `Option<SearxngClient>` selection is handled at the `KnowledgeConfig` layer — `SearxngClient::new()` itself takes a non-optional `&SearchConfig` and returns `Result<Self, SearchError>`. The caller (`serve_stdio` in `01-knowledge-layer.md §6.1`) checks `if let Some(ref search_cfg) = config.search { SearxngClient::new(search_cfg)? }` before calling. The MCP `web_search` tool is registered only if the client construction succeeds.
- `gadgetron-knowledge/src/mcp/tools.rs` — tool list is conditional on
  `SearxngClient` presence; `web_search` is absent from the MCP tool list when
  the client is None
- The `SearxngConfig` struct validation in `gadgetron-knowledge/src/config.rs`
  accepts a missing `searxng_url` as valid (opt-out path)
- Integration tests that exercise `web_search` must use the mock SearXNG fixture
  and must NOT make real network requests to Google/Bing/DDG

### For privacy posture

- Gadgetron does not store search queries — this holds by design (audit log records
  tool names, not arguments, per M6)
- Gadgetron cannot govern SearXNG logging behavior — users must configure their own
  SearXNG instance to disable logging if they need stronger guarantees
- Bundled SearXNG `docker-compose.yml` should document how to disable SearXNG query
  logging (a comment in the compose file is sufficient for P2A)
- For P2C, a formal Privacy Impact Assessment is required before enabling shared
  web search (multiple users' queries going through a shared SearXNG instance raises
  the risk profile significantly)

### For P2C

`[P2C-SECURITY-REOPEN]`

In a multi-user deployment, all users' search queries transit the same SearXNG
instance. Depending on SearXNG configuration, queries from different tenants may
appear in the same SearXNG log. This is a data segregation concern. Before P2C
enables `web_search`, the following must be addressed:

- Per-tenant SearXNG instances, OR
- SearXNG query logging explicitly disabled at the infrastructure level, AND
- A privacy disclosure in per-user terms of service (not just an admin-facing manual)

---

## Related threat: prompt injection via search results (out of scope for THIS ADR)

SearXNG search results include `url` and `snippet` fields populated by upstream
engines (Google, Bing, DuckDuckGo, Brave — depending on your SearXNG
configuration). These fields may contain adversarial content designed to
manipulate an LLM (prompt injection). This is a distinct threat from privacy
disclosure.

**Privacy ADR (this document)**: covers what data leaves the operator's
machine to external search engines.

**Prompt injection defense**: covered in `docs/design/phase2/00-overview.md §8`
STRIDE table (SearXNG row, I category = High). Mitigation is M8 — risk
acceptance for P2A (Claude Code is the reasoning agent and provides partial
defense by its own prompt injection resistance). P2C+ may add content
sanitization.

These two concerns compose cleanly: PRIVACY (what leaves) is addressed here;
ADVERSARIAL CONTENT (what comes back) is addressed in the STRIDE table.

---

## References

| Document | Section | Relevance |
|---|---|---|
| `docs/design/phase2/00-overview.md` v2 | §8 M7 | SearXNG risk definition |
| `docs/design/phase2/00-overview.md` v2 | §10 Disclosure 2 | Verbatim disclosure text (canonical) |
| `docs/design/phase2/00-overview.md` v2 | §10 GDPR section | Legal basis for disclosure requirement |
| `docs/design/phase2/00-overview.md` v2 | §8 STRIDE table, SearXNG row | I (disclose) = High |
| `docs/design/phase2/00-overview.md` v2 | Appendix D, SEC-7 entry | History of this disclosure requirement |
| `docs/design/phase2/01-knowledge-layer.md` v2 | §9 M7 | Implementation-level disclosure gate |
| `docs/design/phase2/01-knowledge-layer.md` v2 | §4.4 M5 | Abstract commit messages (no query content in git) |
| `docs/design/phase2/01-knowledge-layer.md` v2 | §7 config schema | `searxng_url` opt-out mechanism |
| `docs/design/phase2/02-penny-agent.md` v2 | §16 ADR table | Listed as impl blocker |
| `docs/process/03-review-rubric.md` | §1.5 | Security review gate including this check |
| GDPR | Art. 13, Art. 14 | Right to be informed; basis for user disclosure |

---

## Changelog

- **2026-04-13 — Round 2**: added prompt-injection cross-reference section (out-of-scope clarification pointing to STRIDE table); updated disclosure text to "depending on your SearXNG instance" wording; reconciled `SearxngClient::new()` signature (non-optional `&SearchConfig`, `Result<Self>`) with `01-knowledge-layer.md §6.1`.
