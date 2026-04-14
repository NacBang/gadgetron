# Round 1.5 Security Review — `gadgetron-web` Detailed Spec v1

**Reviewer**: security-compliance-lead
**Date**: 2026-04-14
**Scope**: `docs/design/phase2/03-gadgetron-web.md` draft v1 (PM authored 2026-04-14)
**Drives**: ADR-P2A-04 (ACCEPTED stub), D-20260414-02
**Supersedes (partial)**: `docs/design/phase2/00-overview.md §8` "gadgetron-web" row placeholder
**Review basis**: `docs/process/03-review-rubric.md §1.5-A`, OWASP Top 10 (2021), OWASP ASVS L2, OWASP LLM Top 10, CSP Level 3 spec, CWE-22/79/80/829/1021

---

## Verdict

**REVISE**

The v1 design is structurally sound — a threat model section exists, trust boundaries are enumerated, M-W1..M-W6 are named, CSP is owned by the gateway (good), feature opt-out is wired, and `npm ci --ignore-scripts` is specified. Several Round 1.5 blockers are, however, present and must be fixed before TDD begins. Most are concrete and narrow: XSS bypass vectors in the DOMPurify config, a CSP directive gap that enables clickjacking sibling vectors, a wiring gap between `WebConfig.csp_connect_src` and the static `CSP_STRING`, a path-traversal normalization gap, a same-origin assumption that silently breaks under path-rewriting reverse proxies, and missing GDPR mapping for the `localStorage` key decision.

Counts: **7 Blockers** (SEC-W-B1 .. SEC-W-B7), **9 Non-blockers** (SEC-W-NB1 .. SEC-W-NB9), **3 Observations**.

---

## 1. Threat Model Completeness (§21)

### 1.1 Assets — coverage check

The §21 Assets table adds three rows: localStorage API key (High), embedded static assets (Low), CSP header content (Medium). Missing:

- **Gadgetron API key in transit** across the same-origin XHR (`/v1/models`, `/v1/chat/completions`). Not persistent but observable to any in-page script.
- **Assistant response content** before sanitization — high-sensitivity because an attacker-controlled wiki page or tool output may flow through the markdown renderer. This is the M-W1 root asset and deserves an explicit row.
- **`package-lock.json` integrity hashes** — the build-time trust root. Without an explicit row the reviewer has to infer it from M-W3 text.

### 1.2 Trust boundaries — coverage check

B-W1/B-W2/B-W3 are the right three, but the table omits:

- **B-W4: React render tree → DOM innerHTML** via `dangerouslySetInnerHTML` in `MarkdownRenderer`. This is *the* XSS-sensitive boundary; it deserves a row because every assistant message crosses it.
- **B-W5: `build.rs` → filesystem (`web/out/` → `web/dist/` copy)**. If `build.rs` is tricked into copying symlinks out of the intended tree, arbitrary files could land in `include_dir!`. Low practical risk but should be noted as in-scope.

### 1.3 STRIDE row gaps

The §21 STRIDE table has 4 rows covering the two crates (Rust + React), CSP layer, and npm build tree. Gaps:

- **`build.rs` subprocess invocation** (`npm ci`, `npm run build`) is not a STRIDE row. It should be — this is a **Tampering** (T) + **Elevation** (E) surface because it runs unsandboxed with the developer's shell environment at build time. The M-W3 row in §19 is partial mitigation, but the STRIDE row must exist.
- **CSP layer row** lists only "misconfigured directive commit" as the unmitigated risk. Missing: **downgrade attack** (if an operator runs behind a reverse proxy that strips response headers and re-adds its own, the per-subtree scoping breaks silently). Add as a noted risk; the mitigation is `curl -I` verification in the install manual.

**Verdict on §21**: structurally meets §1.5-A, but the omissions above are concrete enough to be blockers for the XSS-sensitive boundary (B-W4). See **SEC-W-B1**.

---

## 2. Blockers

### SEC-W-B1 — DOMPurify config allows style-affecting class bypass; `ALLOWED_ATTR` + `ADD_ATTR` semantics misused

**Location**: §16, `lib/sanitize.ts` (file content lines 778–801)

**Severity**: HIGH

**Issue**: Three concrete defects in the DOMPurify config:

1. **`ADD_ATTR: ['target', 'rel']`** is used to "add" `target`/`rel`, but `ADD_ATTR` in DOMPurify 3.x means "also allow these attributes to survive sanitization in the input", NOT "append these to the output". The link-target enforcement happens via the `afterSanitizeAttributes` hook (which correctly calls `setAttribute`). The `ADD_ATTR` entry is therefore dead code at best, and at worst could let an attacker smuggle a `target="javascript:..."` in Edge/Safari quirk modes where some sanitizers used to treat `target` URI-sensitively. Remove `ADD_ATTR` entirely.

2. **`ALLOWED_ATTR: ['href', 'title', 'alt', 'class', 'lang']`** includes `class` globally. `class` on arbitrary tags (including `<div>`, `<span>`) is *not* itself XSS-exploitable, but combined with **`style-src 'unsafe-inline'`** in the CSP (Appendix B) + a Tailwind-based stylesheet that defines `.bg-[url('javascript:...')]` arbitrary-class utilities, an attacker can at minimum steal focus styles, mimic system UI, and stage UI redressing attacks. More concretely: Tailwind's JIT mode compiles arbitrary class names from source, but the compiled CSS is already fixed at build time, so runtime injected `class="bg-[url('http://evil/')]"` will not match a rule. That reduces the risk. However, `class` on `<img>`, `<a>`, `<td>` is enough to pose as a fake "verified" badge or trick the user into clicking a crafted link. Tighten to `ALLOWED_ATTR: ['href', 'title', 'alt', 'lang']` and move `class` to a per-tag allowlist using `ALLOWED_ATTR` + `ALLOWED_CLASSES` (DOMPurify 3.x supports per-tag class lists), restricting to code-block language classes: `{ 'code': ['language-*', 'hljs', 'hljs-*'], 'pre': ['language-*'] }`.

3. **`USE_PROFILES: { html: true }`** is the "common HTML profile" preset. It already allows MathML and SVG if `svg: true`/`mathMl: true` are set elsewhere, but the interaction with `FORBID_TAGS` is order-dependent: per DOMPurify docs, `FORBID_TAGS` wins over `USE_PROFILES`, so the current spec is correct *in practice*. However, `USE_PROFILES` also implicitly allows `<form>` and `<input>` in older DOMPurify versions (pre 2.4), and the version is not pinned in §9 or §19. **Pin `dompurify` to `>=3.0.9` in `package.json` and document the floor in §19.** CVE-2024-45801 (DOMPurify mXSS via nested template elements) is fixed in 3.1.3; CVE-2024-47875 (prototype pollution) in 3.2.4.

**Additional bypass vectors not covered by the current config**:

- **SVG `<use href="#...">` DOM-clobbering** — SVG not explicitly forbidden. DOMPurify does sanitize SVG but only when `USE_PROFILES.svg !== false`. Add `FORBID_TAGS: [..., 'svg', 'math']` unless/until the design decides to allow them.
- **`<a href="blob:...">`** — `ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#)/i` correctly excludes `blob:` but not relative-scheme URLs like `//evil.example.com/`. Since the regex requires a scheme prefix, relative URLs fall through and DOMPurify's default URI policy takes over. Confirm behavior with an explicit test (`sanitize: rejects scheme-relative URLs`) — if the default strips them, add a non-regression test; if not, tighten the regex to `/^(?:https?:|mailto:|#)[^\s]/i` and reject scheme-relative outright.
- **mXSS in nested `<template>`** — already fixed in DOMPurify 3.1.3 but only if the version floor is enforced. See pin requirement above.
- **`ALLOW_DATA_ATTR: false`** is correct. Also explicitly set `ALLOW_ARIA_ATTR: false` (for P2A — ARIA attributes are unnecessary in LLM output rendering and ARIA-label-based spoofing of screen-reader content has been reported).

**Required fix** — rewrite `lib/sanitize.ts` config block to:

```ts
// package.json:
//   "dompurify": "3.2.4"   // pinned, see §19

const CONFIG: DOMPurify.Config = Object.freeze({
  USE_PROFILES: { html: true },  // html only; svg/mathML implicitly disabled
  ALLOWED_TAGS: [
    'p', 'br', 'hr', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'strong', 'em', 'b', 'i', 'u', 's', 'code', 'pre',
    'ul', 'ol', 'li',
    'blockquote',
    'a', 'img',
    'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'span', 'div',
  ],
  ALLOWED_ATTR: ['href', 'title', 'alt', 'lang'],  // class removed from global list
  ALLOWED_CLASSES: {
    'code': ['language-*', 'hljs', 'hljs-*'],
    'pre':  ['language-*'],
  },
  FORBID_TAGS: [
    'script', 'style', 'iframe', 'object', 'embed', 'form',
    'input', 'button', 'svg', 'math', 'template', 'base',
    'meta', 'link', 'frame', 'frameset',
  ],
  FORBID_ATTR: [
    // Event handlers — explicit list since ALLOWED_ATTR alone is deny-after-allow
    'onclick', 'onerror', 'onload', 'onmouseover', 'onmouseout',
    'onmouseenter', 'onmouseleave', 'onfocus', 'onblur', 'onchange',
    'onsubmit', 'onkeydown', 'onkeyup', 'onkeypress',
    'onpointerdown', 'onpointerup', 'onpointermove',
    'ontouchstart', 'ontouchend', 'ontouchmove',
    'onanimationstart', 'onanimationend', 'ontransitionend',
    // Navigation / form injection
    'style', 'srcset', 'formaction', 'action', 'xlink:href',
    'background', 'ping',
  ],
  ALLOW_DATA_ATTR: false,
  ALLOW_ARIA_ATTR: false,
  // Explicit scheme allowlist; scheme-relative URLs are stripped
  ALLOWED_URI_REGEXP: /^(?:https?:|mailto:|#)[^\s]/i,
  // Defense in depth: if caller forgets to call sanitize, an exception is thrown
  RETURN_TRUSTED_TYPE: false,
  // Return a string (for dangerouslySetInnerHTML); do not return DocumentFragment
  RETURN_DOM: false,
  RETURN_DOM_FRAGMENT: false,
});
// Remove ADD_ATTR — it's semantically wrong here.
```

Add the following tests to §22 `sanitize.test.ts`:

| Test | Input | Expected |
|---|---|---|
| `sanitize: rejects scheme-relative URL` | `<a href="//evil.example.com">x</a>` | `<a>x</a>` (href stripped) |
| `sanitize: rejects svg` | `<svg><script>alert(1)</script></svg>` | `""` |
| `sanitize: rejects template mXSS` | `<template><img src=x onerror=alert(1)></template>` | `""` or sanitized to `<template></template>` |
| `sanitize: rejects class on non-code tag` | `<div class="bg-red-500">x</div>` | `<div>x</div>` |
| `sanitize: preserves language class on pre/code` | `<pre class="language-rust"><code class="language-rust">fn main(){}</code></pre>` | unchanged |
| `sanitize: rejects aria-label` | `<div aria-label="click me">x</div>` | `<div>x</div>` |
| `sanitize: rejects formaction` | `<input formaction="javascript:alert(1)">` | `""` (tag forbidden anyway; defense in depth) |
| `sanitize: rejects <base href="//evil">` | `<base href="//evil">` | `""` |

**Why blocker**: §21 correctly identifies "XSS escalates to API key exfiltration from localStorage" as High unmitigated risk. M-W1 is the primary control. The current config has real bypass surface (scheme-relative URLs, SVG, missing version floor, dead `ADD_ATTR`) and cannot be signed off in its current form.

---

### SEC-W-B2 — CSP `frame-ancestors` is correct but CSP omits `form-action` scope + `trusted-types` + `require-trusted-types-for` directive; `style-src 'unsafe-inline'` mitigation not justified with data

**Location**: Appendix B (CSP string)

**Severity**: HIGH

**Issue breakdown**:

1. **`style-src 'unsafe-inline'` trade-off is asserted but not measured**. The design says "needed for Tailwind + shadcn's inlined styles" — but Tailwind in build-time compilation mode (`@tailwindcss/cli` or the Next.js plugin) emits static CSS into `_next/static/css/*.css`, NOT inline styles. Inline styles come from: (a) Next.js hydration `<style>` blocks for CSS modules, (b) Radix UI measurement code that sets `style="..."` via JS (which is runtime `element.style` assignment, *not* inline `<style>` in HTML), and (c) the dark-mode CSS variable injection in `<html style="color-scheme: ...">`. None of these require `style-src 'unsafe-inline'` if `nonce`-based CSP is used. The design should either:
   - **(Preferred)** Use `style-src 'self' 'nonce-<random>'` where the gateway generates a per-response nonce (via `tower_http::set_header` is not enough — needs a middleware that inserts a nonce into the response and that the frontend uses). This is non-trivial for a static-export SPA because `index.html` is pre-built.
   - **(Fallback)** Use `style-src 'self' 'unsafe-hashes'` with explicit hashes of all inline style elements that are statically known at build time. Next.js static export can enumerate them.
   - **(Minimum acceptable)** Keep `'unsafe-inline'` but add an **audit trail**: a build-time script that greps `web/out/**/*.html` for `style="..."` and `<style>` occurrences and logs them; security reviewer can then confirm each is not an XSS sink. Commit the audit list as `crates/gadgetron-web/web/inline-styles-audit.md`.

   The minimum acceptable path is OK for P2A, but the design doc must state it and link the audit file. Without it, the "known trade-off" claim is unverifiable.

2. **`script-src 'self'` is tight, but could be tightened further with `'strict-dynamic'`**. CSP Level 3 `'strict-dynamic'` means "trust only scripts that are transitively loaded by already-trusted scripts, ignoring host-based allowlist". For a Next.js static export that ships a single entry chunk, `'strict-dynamic'` + `'nonce-<rand>'` is more robust than `'self'` because it blocks script injection via `<script src="/web/_next/static/evil.js">` (which `'self'` would permit). The trade-off is that `'strict-dynamic'` requires nonce plumbing (same complexity as item 1). For P2A, **keep `script-src 'self'`** but note in Appendix B the rationale for not using `'strict-dynamic'` and the plan to reevaluate in P2B when nonce plumbing exists. This is an honesty requirement, not a block — but since it's missing altogether, flag as a blocker for documentation reasons.

3. **Missing directives**:
   - **`require-trusted-types-for 'script'`** — blocks `document.write` / `innerHTML` sinks at the DOM API level. Because the design uses `dangerouslySetInnerHTML` in `MarkdownRenderer`, Trusted Types enforcement would fail at runtime **unless** the sanitizer returns a `TrustedHTML`. DOMPurify supports `RETURN_TRUSTED_TYPE: true` in Chromium browsers. For P2A the recommendation is: **add `require-trusted-types-for 'script'; trusted-types default 'dompurify'`** in the CSP, set `RETURN_TRUSTED_TYPE: true` in the sanitize config, and register a `default` Trusted Types policy that forwards to DOMPurify at app boot. Firefox doesn't support Trusted Types — the directive is silently ignored there, which is fine (progressive enhancement). This is the single strongest defense-in-depth against M-W1 bypass.
   - **`form-action 'self'`** — prevents form-hijacking attacks where an injected `<form action="https://evil">` steals submitted data. Already present (§Appendix B line `form-action 'self'`), good. No change.
   - **`frame-src 'none'`** — not in the directive set. Since no iframes are intended, make it explicit.
   - **`media-src 'self'`** — not set. Defaults to `default-src 'self'` via fallthrough, which is fine, but for audit clarity, list all directives explicitly.
   - **`report-to` / `report-uri`** — no CSP violation reporting endpoint. For P2A local, this is acceptable, but document the gap and plan for a `/v1/csp-report` endpoint in P2B that writes CSP violations to the audit log. Blocker level: Non-blocker (see SEC-W-NB4).

4. **`upgrade-insecure-requests`** is set. This means if an operator serves Gadgetron over plain HTTP on a non-localhost interface, the browser will upgrade all same-origin XHR to HTTPS, which **will fail**. For P2A localhost, no effect. For P2C via reverse proxy, correct. For P2A **non-localhost HTTP**, broken. Add a note that `upgrade-insecure-requests` is appropriate *only* when the gateway is behind TLS, and if an operator sets `bind_addr = "0.0.0.0:8080"` over plain HTTP, they must remove this directive (or better, enable TLS).

**Required fix** — rewrite Appendix B to:

```
default-src 'self';
base-uri 'self';
frame-ancestors 'none';
frame-src 'none';
form-action 'self';
img-src 'self' data:;
font-src 'self';
style-src 'self' 'unsafe-inline';
script-src 'self';
connect-src 'self';
worker-src 'self';
manifest-src 'self';
media-src 'self';
object-src 'none';
require-trusted-types-for 'script';
trusted-types default dompurify;
upgrade-insecure-requests
```

Add sub-section Appendix B.2 titled "Inline-style audit trail" listing every `<style>` and `style="..."` occurrence in `web/out/`, along with the `build.rs` grep step that emits a warning if a new inline style appears without being added to the audit list.

Add §22 test:

```
gateway_web_response_csp_contains_trusted_types:
  Assert the CSP header contains `require-trusted-types-for 'script'` and
  `trusted-types default dompurify` exactly as specified.
```

Add §16 code change:

```ts
// crates/gadgetron-web/web/app/layout.tsx — at module top, before any component
if (typeof window !== 'undefined' && window.trustedTypes && window.trustedTypes.createPolicy) {
  window.trustedTypes.createPolicy('dompurify', {
    createHTML: (input: string) => DOMPurify.sanitize(input, CONFIG),
  });
}
```

and set `RETURN_TRUSTED_TYPE: true` in `lib/sanitize.ts` (guarded by feature detection).

**Why blocker**: CSP is the primary M-W2 control and the design claims it "prevents the attacks it claims to prevent". Without Trusted Types, a single M-W1 bypass anywhere in the pipeline cascades to XSS. With Trusted Types, even a bypass fails at the `innerHTML` sink. This is a 1-line CSP directive + ~15 lines of JS — high value, low cost. Blocker.

---

### SEC-W-B3 — `WebConfig.csp_connect_src` is specified but never wired to `CSP_STRING`; dead config + broken P2C upgrade path

**Location**: §7 (`web_csp_layer` uses `HeaderValue::from_static(CSP_STRING)`) vs. §18 (`WebConfig::csp_connect_src: Vec<String>`)

**Severity**: HIGH

**Issue**: §18 declares the runtime override `GADGETRON_WEB_CSP_CONNECT_SRC` / `[web] csp_connect_src` for operators who proxy Gadgetron behind a CDN or a reverse proxy requiring different origins. §7 wires the CSP via `HeaderValue::from_static(CSP_STRING)` — a compile-time constant. The two do not intersect. An operator who sets `csp_connect_src = ["'self'", "https://auth.company.com"]` will see **no effect** because the `from_static` path ignores it. This is a silent misconfiguration; a correctness + audit-log gap.

**Fix required**: Replace the static CSP with a runtime-built header:

```rust
// crates/gadgetron-gateway/src/web_csp.rs
pub fn web_csp_layer(cfg: &WebConfig) -> /* layer stack */ {
    let csp = build_csp_header(cfg);
    // HeaderValue::from_str is fallible; validate at gateway startup.
    let header = HeaderValue::from_str(&csp)
        .expect("web CSP header validation — check WebConfig::csp_connect_src entries");
    // ... rest of layer stack
}

fn build_csp_header(cfg: &WebConfig) -> String {
    // Start from the Appendix B template (B.1).
    // Replace `connect-src 'self'` with joined allowlist.
    let connect = if cfg.csp_connect_src.is_empty() {
        "'self'".to_string()
    } else {
        cfg.csp_connect_src.join(" ")
    };
    // Sanitize each entry: must match ^[A-Za-z][A-Za-z0-9+.-]*:// or be 'self' | 'none' | 'unsafe-inline'
    // Reject anything containing ; newline < >
    for src in &cfg.csp_connect_src {
        if src.contains(';') || src.contains('\n') || src.contains('<') || src.contains('>') {
            panic!("invalid csp_connect_src entry: {src}");
        }
    }
    format!("default-src 'self'; ... connect-src {connect}; ...")
}
```

Add validation in `gadgetron-core::config::WebConfig`:

```rust
impl WebConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        static CSP_SRC_RE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r"^(?:'self'|'none'|https?://[A-Za-z0-9.\-]+(?::\d+)?)$").unwrap()
        });
        for src in &self.csp_connect_src {
            if !CSP_SRC_RE.is_match(src) {
                return Err(ConfigError::InvalidField {
                    field: "web.csp_connect_src",
                    value: src.clone(),
                    reason: "must be 'self', 'none', or https://host[:port]".into(),
                });
            }
        }
        Ok(())
    }
}
```

Add §22 tests:

| Test | What it asserts |
|---|---|
| `web_config_rejects_csp_connect_src_with_semicolon` | `csp_connect_src = ["'self'; script-src *"]` → validation error (header injection attempt) |
| `web_config_rejects_csp_connect_src_with_newline` | `csp_connect_src = ["'self'\nscript-src *"]` → validation error |
| `gateway_csp_header_reflects_connect_src_override` | Set `csp_connect_src = ["'self'", "https://auth.example.com"]` → GET `/web/` → header contains `connect-src 'self' https://auth.example.com;` |
| `gateway_csp_header_default_is_self_only` | Unset `csp_connect_src` → header contains `connect-src 'self';` |

**Why blocker**: Silent mis-config is worse than a missing feature. §21 references §18 as the P2C upgrade path; if that path is non-functional, P2C upgrade silently breaks CSP for operators who think they've configured it. Also: without input validation on `csp_connect_src`, an operator typo like `"'self'; script-src *"` becomes an **actual CSP header injection** — an attacker with `gadgetron.toml` write access (already root-level, but includes ops misconfiguration) can neuter the entire CSP. Validation is cheap; add it.

---

### SEC-W-B4 — `serve_asset` path-traversal check is insufficient against percent-encoded + unicode normalization bypass; proptest corpus under-specified

**Location**: §5 `serve_asset` handler lines 374–384; §22 `proptest_path_inputs_never_panic`

**Severity**: HIGH

**Issue**: The current check:

```rust
if path.contains("..") || path.starts_with('/') {
    return (StatusCode::BAD_REQUEST, "invalid path").into_response();
}
```

This rejects literal `..` and leading `/`. Known bypasses:

1. **Percent-encoded dot-dot**: `%2e%2e`, `%2E%2E`, `.%2e`, `%2e.`
   - axum's `Path<String>` extractor uses the matched route segment from `matchit`, which operates on the URL path *after* Hyper's `http` percent-decoding? **No** — `http::Uri::path()` returns the raw encoded path. `matchit` does not decode. `axum::extract::Path<String>` when extracting a single `*path` capture returns the encoded string with the leading `/` already stripped. So `%2e%2e` will arrive as `%2e%2e` — the current `contains("..")` check **misses** it.
2. **Double-encoded dot-dot**: `%252e%252e` (the `%25` decodes to `%`, then `%2e%2e` would require a second decode). Most handlers decode once; this is safe unless a middleware chain double-decodes. Document that no middleware in the chain double-decodes.
3. **Unicode normalization**: `..` is two ASCII `0x2E`. NFKC/NFKD does not normalize anything else into `.` in any standard Unicode table. Verified via UCD. **Not a realistic vector** for `..` specifically, but for backslash on Windows, or for fullwidth `．．` (U+FF0E), the latter would pass the current check. Since `include_dir!` stores forward-slash paths and rejects backslashes, Windows-style `..\\..\\` is effectively blocked upstream. Fullwidth dots are also not in `include_dir`'s lookup table — the `get_file` call would 404 and fall back to index.html, which is acceptable but the proptest should assert it.
4. **Backslash**: `\\..\\etc\\passwd` — not checked. On Linux `\\` is a valid filename character, so `include_dir` treats it as part of the filename and lookup fails → SPA fallback. Not exploitable but worth asserting.
5. **Null byte**: `foo%00.html` — Rust `String` is UTF-8 and null bytes are legal, so `contains("..")` sees it and the null byte passes through. `include_dir::get_file` would fail lookup → SPA fallback. Not exploitable but worth rejecting explicitly for defense in depth.
6. **Absolute path via double-slash**: `//etc/passwd` would be extracted by axum as `/etc/passwd` (route match strips one leading slash) → current `starts_with('/')` catches it. Good.
7. **URL with fragment/query leaking**: `foo?../bar` — query is stripped by `http::Uri::path()`. Good.

**Fix required**: Replace the guard with a more thorough one, and add percent-decode-then-recheck:

```rust
async fn serve_asset(Path(raw): Path<String>) -> Response {
    match validate_and_decode(&raw) {
        Ok(decoded) => match WEB_DIST.get_file(&decoded) {
            Some(file) => render_file(&decoded, file.contents()),
            None => render_index(),
        },
        Err(()) => (StatusCode::BAD_REQUEST, "invalid path").into_response(),
    }
}

/// Decode once, reject traversal. Returns Err on any suspicious input.
fn validate_and_decode(raw: &str) -> Result<String, ()> {
    // 1. Reject null bytes
    if raw.contains('\0') { return Err(()); }
    // 2. Reject backslashes (not valid in our embed tree; sign of bypass attempt)
    if raw.contains('\\') { return Err(()); }
    // 3. Percent-decode once. urlencoding::decode or manual 2-hex-digit loop.
    let decoded = percent_decode_once(raw).map_err(|_| ())?;
    // 4. Re-check for traversal after decode
    if decoded.contains("..") || decoded.starts_with('/') { return Err(()); }
    // 5. Reject non-ASCII control chars (0x00-0x1F, 0x7F)
    if decoded.bytes().any(|b| b < 0x20 || b == 0x7F) { return Err(()); }
    // 6. Reject any segment that begins with a dot (hidden files like .env)
    if decoded.split('/').any(|s| s.starts_with('.')) { return Err(()); }
    Ok(decoded)
}
```

Add dependency: `percent-encoding = "2"` (already in the workspace via `reqwest`; confirm with `cargo tree`). If not, add it to workspace deps.

Expand §22 proptest to explicitly cover the 8 categories:

```rust
#[test]
fn proptest_path_inputs_never_panic() {
    use proptest::prelude::*;
    proptest!(|(input in path_strategy())| {
        // validate_and_decode must not panic on arbitrary input
        let _ = validate_and_decode(&input);
    });
}

fn path_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        // 1. Raw `..`
        Just("../etc/passwd".to_string()),
        Just("foo/../bar".to_string()),
        // 2. Percent-encoded `..`
        Just("%2e%2e/etc/passwd".to_string()),
        Just("%2E%2E/etc/passwd".to_string()),
        Just(".%2e/etc/passwd".to_string()),
        // 3. Double-encoded (must not be decoded twice)
        Just("%252e%252e/etc/passwd".to_string()),
        // 4. Null byte
        Just("foo%00.html".to_string()),
        Just("foo\0bar".to_string()),
        // 5. Absolute path
        Just("/etc/passwd".to_string()),
        Just("//etc/passwd".to_string()),
        // 6. Backslash
        Just("..\\..\\etc\\passwd".to_string()),
        // 7. Hidden file
        Just(".env".to_string()),
        Just("foo/.git/config".to_string()),
        // 8. Fullwidth unicode dots
        Just("\u{FF0E}\u{FF0E}/etc/passwd".to_string()),
        // 9. Control chars
        any::<String>().prop_filter("non-empty", |s| !s.is_empty()),
    ]
}

#[test]
fn positive_paths_accepted() {
    for ok in [
        "index.html",
        "settings/index.html",
        "_next/static/abc123.js",
        "_next/static/def456.css",
        "favicon.ico",
    ] {
        assert!(validate_and_decode(ok).is_ok(), "should accept: {ok}");
    }
}

#[test]
fn traversal_variants_all_rejected() {
    for bad in [
        "../Cargo.toml",
        "%2e%2e/Cargo.toml",
        "%2E%2E/Cargo.toml",
        "/etc/passwd",
        "//etc/passwd",
        "..\\windows\\system32",
        "foo\0bar",
        ".env",
        "foo/.git/HEAD",
        "\u{FF0E}\u{FF0E}/etc/passwd",
    ] {
        assert!(validate_and_decode(bad).is_err(), "should reject: {bad}");
    }
}
```

**Why blocker**: Even though none of the current bypass vectors lead to a realistic disclosure (all files live in `WEB_DIST` which contains only the SPA bundle — nothing sensitive), the "fail closed" guarantee is the whole point of the custom handler vs. `ServeDir`. The current check is advisory. The spec explicitly says "We fail closed" in §5 comment, so the implementation must actually fail closed. Cheap fix, add it.

---

### SEC-W-B5 — Same-origin assumption silently breaks under path-rewriting reverse proxy (e.g. `/api/v1/*` rewrite); no operator-facing guidance or config

**Location**: §8 B-W2 (`User browser JS → :8080/v1/*`) + §14 `fetch('/v1/models', ...)` + §15 `fetch('/v1/chat/completions', ...)`

**Severity**: HIGH (correctness + operator misconfiguration surface)

**Issue**: The frontend hardcodes `/v1/models` and `/v1/chat/completions` as fetch targets. This works when the gateway is mounted at the root of the origin. Common P2C / reverse-proxy setups violate this:

1. **Operator serves Gadgetron at `/gadgetron/v1/*` and `/gadgetron/web/*`** (single nginx proxying multiple apps on one domain). The `basePath: '/web'` setting in Next.js handles the UI side, but the hardcoded `/v1/*` fetch calls now miss the prefix and hit a different app. Same-origin is preserved but the path is wrong.
2. **Operator rewrites `/api/v1/*` → `/v1/*`** on the reverse proxy. The frontend fetches `/v1/...` which is not exposed externally; the reverse proxy returns 404.
3. **Operator uses a different origin for the API** (subdomain split). This breaks same-origin entirely and would require CORS. Out of scope for P2A, but the design should say so explicitly and give operators guidance.

The `connect-src 'self'` CSP correctly prevents key exfiltration to *external* hosts, but it does not prevent an operator from accidentally breaking the UI in case 1 or 2 above.

**Fix required**: Add a runtime-discoverable API base path. Two options:

**Option A (recommended for P2A)**: Emit a `<meta>` tag in `index.html` at build time, containing the API base path, and read it at runtime:

```ts
// crates/gadgetron-web/web/app/layout.tsx
export default function RootLayout({ children }) {
  return (
    <html>
      <head>
        {/* Baked at build time, overridable at runtime */}
        <meta name="gadgetron-api-base" content="/v1" />
      </head>
      <body>{children}</body>
    </html>
  );
}
```

```ts
// crates/gadgetron-web/web/lib/api-client.ts
function apiBase(): string {
  if (typeof document === 'undefined') return '/v1';
  const meta = document.querySelector<HTMLMetaElement>('meta[name="gadgetron-api-base"]');
  return meta?.content || '/v1';
}

export async function getModels(apiKey: string) {
  const res = await fetch(`${apiBase()}/models`, { ... });
  // ...
}
```

Add `[web] api_base_path` to `gadgetron.toml` (§18):

```toml
# The URL path prefix where /v1/* is mounted, as visible to the browser.
# Default "/v1". Change this only if a reverse proxy rewrites the path.
# The frontend reads this from a <meta> tag injected by gadgetron-gateway at response time.
# env: GADGETRON_WEB_API_BASE_PATH
api_base_path = "/v1"
```

For this to work, the gateway must rewrite `index.html` on the fly OR the build must bake the default and a middleware replaces the meta content on each response. The middleware path is simpler: use a `SetResponseHeaderLayer`-equivalent for HTML body rewriting, or pre-process the HTML at gateway startup. For P2A, **pre-process at gateway startup**:

```rust
// gadgetron_web::service() reads WEB_DIST["index.html"], replaces
// `content="/v1"` with the configured value, and serves the mutated bytes.
// Load-once, serve-many.
pub fn service(cfg: &WebConfig) -> Router {
    let index_html = WEB_DIST.get_file("index.html")
        .map(|f| std::str::from_utf8(f.contents()).unwrap_or(""))
        .unwrap_or("")
        .replace(
            r#"<meta name="gadgetron-api-base" content="/v1">"#,
            &format!(r#"<meta name="gadgetron-api-base" content="{}">"#, cfg.api_base_path),
        );
    // Serve the mutated index_html via a closure; other assets served unchanged.
    ...
}
```

**Option B (simpler but less flexible)**: Document the constraint. The §18 `[web]` block currently has `base_path = "/web"`; add `api_base_path = "/v1"` as a sibling and a note that **both** must match the reverse proxy's routing. Include a §20 "Reverse proxy deployment" sub-section with concrete nginx config examples.

**Required change**: Pick Option A and implement it, OR pick Option B and write the operator docs. Either resolves the blocker; Option A is more robust.

Add §22 tests:

| Test | What it asserts |
|---|---|
| `index_html_contains_api_base_meta` | `WEB_DIST["index.html"]` bytes contain `<meta name="gadgetron-api-base"` |
| `service_replaces_api_base_in_index` | Configure `api_base_path = "/prefix/v1"` → GET `/web/` → response body contains `content="/prefix/v1"` |

**Why blocker**: P2A is advertised as single-binary-localhost, but §18 already hints at P2C reverse-proxy use via `csp_connect_src`. Shipping the frontend with hardcoded `/v1` creates a hidden coupling that operators will trip over, and the error mode is confusing (network errors in browser console, CORS-style symptoms, nothing pointing to the cause). Add config + docs now, before the frontend gets built into test artifacts.

---

### SEC-W-B6 — `localStorage` origin-isolation is per-origin not per-path; operator path-sharing deployments leak the API key across apps

**Location**: §13 localStorage schema; §8 M-W2 in ADR-P2A-04

**Severity**: HIGH

**Issue**: The design claims the API key is "keyed on `:8080/web`" (§M-W2 in ADR-P2A-04). **This is incorrect**. `localStorage` is scoped to **origin** (`scheme://host:port`), NOT to path. A Gadgetron deployment at `http://example.com:8080/web` shares localStorage with any other app at `http://example.com:8080/other-app`, `http://example.com:8080/internal/admin`, etc. If an operator shares an origin between Gadgetron and another web app (common for internal tools on a company domain), **any script from the other app can read `gadgetron_web_api_key` from localStorage**. This is not hypothetical — it's the standard reason SaaS vendors push authentication into HttpOnly cookies.

The §13 "XSS defense" paragraph (3) explicitly says "localStorage is equally exposed to same-origin scripts as a cookie would be" — this is true for cookies without HttpOnly, but **`HttpOnly` cookies are NOT accessible to JavaScript**, which the justification glosses over. The deliberate choice to use localStorage over HttpOnly cookies is defensible for a single-origin-single-app deployment, but not for a shared-origin deployment.

**Fix required**:

1. **Add a deployment constraint** to §18 and to the install manual:
   > **Origin isolation requirement**: Gadgetron MUST be deployed on an origin (scheme + host + port) that is not shared with any other web application. If another app on the same origin is compromised, it can read the Gadgetron API key from browser localStorage. Use a dedicated subdomain (e.g. `gadgetron.example.com`) or a dedicated port for deployments that share a host with other apps.

2. **Add a runtime detection warning**: on `/settings` page load, check `document.domain`, `location.port`, and `location.pathname`. If the path is NOT `/web` (i.e., the app is mounted somewhere else — a sign of shared-origin path-partitioning), show a console warning:
   ```ts
   useEffect(() => {
     if (location.pathname.indexOf('/web') !== 0) {
       console.warn(
         'Gadgetron: this deployment does not use the default /web base path. ' +
         'localStorage is shared across all apps on this origin — ensure this ' +
         'origin is dedicated to Gadgetron. See docs/manual/deployment.md §Origin-isolation.'
       );
     }
   }, []);
   ```
   This is not a security control (attacker is scripted; warnings are ignored), but it flags honest operators.

3. **Document key rotation**: add a §13.1 "Compromise recovery" paragraph:
   > If you suspect your API key has been exposed (including: XSS incident, localStorage leak to a co-hosted app, key pasted into an untrusted page, laptop lost/stolen), rotate immediately:
   > ```sh
   > gadgetron key create --rotate <old_key_id>
   > ```
   > Then clear the browser's `gadgetron_web_*` localStorage entries via the `/settings` → "Clear" button, and paste the new key. The old key is invalidated within <1s via the Phase 1 PgKeyValidator LRU invalidation path (D-20260411-12). Audit log entries before rotation remain valid (`request_id` correlation intact).

4. **Mention the `sessionStorage` alternative and why it was rejected**: `sessionStorage` is also per-origin, so it does not solve the shared-origin problem, but it IS per-tab and cleared on tab close. This is a defensible trade-off if the design wants to force re-paste on every tab. The v1 doc doesn't mention this; the audit trail should. Add one sentence to §13 explaining that `sessionStorage` was considered and rejected because users would need to paste the key on every page reload.

5. **Optional — IndexedDB with `chrome.storage.local`-like isolation**: not available without a service worker + Origin Private File System (OPFS). Out of scope for P2A; mention as P2B consideration.

**Why blocker**: The ADR and the design doc both make a claim ("keyed on `:8080/web`") that is false at the browser API level. Correct the claim, document the real constraint, and add the operator-facing guidance. Without this, an operator who deploys Gadgetron at `https://internal-tools.company.com/gadgetron/` alongside `https://internal-tools.company.com/jenkins/` will silently leak keys to Jenkins XSS.

---

### SEC-W-B7 — `build.rs` invokes `npm` from inherited PATH + inherited env; no integrity verification beyond lockfile hash; no reproducibility guarantee

**Location**: §4 `build.rs` pseudocode lines 246–263

**Severity**: HIGH

**Issue**: Three related gaps:

1. **PATH inheritance**: `which_npm()` walks `env::var_os("PATH")` and picks the first `npm` found. In CI, this is fine (managed by `actions/setup-node`). On a developer machine, an attacker with write access to a directory earlier on `PATH` than the legitimate Node install can substitute a malicious `npm` that runs arbitrary code at build time with the developer's full UID (and reads `~/.ssh`, `~/.gitconfig`, `~/.aws`, etc.). This is a post-account-compromise vector but matters for the blast-radius argument in the ADR.
2. **Full environment inherited**: `Command::new(&npm)` inherits the parent process env, including `NPM_TOKEN`, `GITHUB_TOKEN`, `AWS_*`, `NODE_AUTH_TOKEN`, and any other secrets. A malicious transitive dep's *build step* (we use `--ignore-scripts` — good, but `npm ci --ignore-scripts` doesn't block `postinstall` execution in all npm versions; verify against the pinned npm version, see below) or a compromised `package.json` `build` script could exfiltrate these.
3. **No integrity verification beyond `package-lock.json`**: `package-lock.json` contains `"integrity": "sha512-..."` hashes for each package, which npm verifies during `ci`. This is strong, but only as strong as the first-time generation of the lockfile (TOCTOU). If an attacker MITMs the very first `npm install` done by the original developer, the lockfile bakes in the attacker's hashes and subsequent `ci` calls happily verify them. Mitigation: cross-check lockfile integrity against a committed `web/npm-shrinkwrap.json.sha256` OR use `npm audit signatures` (npm ≥ 9.5) which verifies packages against the npm registry's Sigstore-backed signatures.
4. **`--ignore-scripts` verification missing for the chosen deps**: §19 line "verify no deps need install scripts to function; assistant-ui, Next, marked, DOMPurify, shiki do not" — this is an assertion, not a verification. The verification step needs to be a CI gate: `npm ls --all --parseable | xargs -I{} node -e 'const p=require("{}/package.json"); if (p.scripts && (p.scripts.install || p.scripts.postinstall || p.scripts.preinstall)) process.exit(1)'` (rough sketch). For P2A, a simpler check: `npm ci --ignore-scripts --dry-run` in CI, then grep for warnings about skipped scripts, and fail the build if any are reported. As of Next.js 14.x, `next` has no install script — safe. `assistant-ui` (verify). `marked`, `DOMPurify`, `shiki`, `tailwindcss`, `postcss` — verify.

**Fix required**:

1. **Sanitize PATH in `build.rs`** — before invoking npm:
   ```rust
   // Prefer a known-good PATH for npm resolution, falling back to inherited only
   // when GADGETRON_WEB_TRUST_PATH=1 is set (for developer escape hatch).
   let trusted_path = if env::var("GADGETRON_WEB_TRUST_PATH").ok().as_deref() == Some("1") {
       env::var_os("PATH").unwrap_or_default()
   } else {
       // Hardcoded minimal PATH — matches typical Node installs on macOS/Linux CI
       OsString::from("/usr/local/bin:/usr/bin:/bin:/opt/homebrew/bin")
   };
   ```
   This is aggressive — many developers have `nvm` in `~/.nvm/versions/node/*/bin`. Alternative: add `~/.nvm/...` via an env-var-driven allowlist, document it in README. Minimum acceptable: emit a `cargo:warning=` with the resolved `npm` absolute path so developers can verify the binary before trusting the build.

2. **Scrub env for the npm subprocess** — don't fully `env_clear()` (npm needs `HOME`, `PATH`, possibly `NODE_OPTIONS`), but explicitly remove known-secret env vars:
   ```rust
   for var in &[
       "NPM_TOKEN", "GITHUB_TOKEN", "AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY",
       "NODE_AUTH_TOKEN", "GH_TOKEN", "CARGO_REGISTRY_TOKEN", "ANTHROPIC_API_KEY",
       "OPENAI_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS", "SSH_AUTH_SOCK",
   ] {
       cmd.env_remove(var);
   }
   ```

3. **Add `npm audit signatures` to CI** (npm ≥ 9.5 required; Node 20 ships npm 10):
   ```yaml
   - run: npm audit signatures
     working-directory: crates/gadgetron-web/web
   ```
   This verifies Sigstore signatures on all registry packages. Gracefully-handle the case where some legacy packages are unsigned (accept with warning, fail on high-severity unsigned packages).

4. **Verify `--ignore-scripts` really blocks postinstall**: add a CI step:
   ```yaml
   - run: |
       rm -rf node_modules
       npm ci --ignore-scripts 2>&1 | tee install.log
       if grep -qi "skipped.*script" install.log; then
         echo "Scripts were skipped — re-evaluate which deps need them"
         grep -i "skipped.*script" install.log
       fi
     working-directory: crates/gadgetron-web/web
   ```
   Fail if any dependency required a script for functional build. Escalate to a design decision (can we use an alternative?).

5. **Add `cargo-deny` config entries** explicitly for `include_dir = 0.7` and `mime_guess = 2`:
   ```toml
   # deny.toml (workspace root)
   [advisories]
   vulnerability = "deny"
   unmaintained = "warn"
   yanked = "deny"

   [licenses]
   allow = [
       "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause",
       "ISC", "MPL-2.0", "CC0-1.0", "Unicode-DFS-2016", "Unlicense",
   ]
   # include_dir 0.7 uses MIT, mime_guess 2 uses MIT, both compatible.

   [bans]
   deny = []
   skip = []
   skip-tree = []
   multiple-versions = "warn"
   ```
   Confirm `include_dir 0.7` transitively pulls `proc-macro2`, `quote`, `syn` (build-time only) — already in workspace, no new advisory surface. `mime_guess 2` pulls `unicase` — verify MIT. These are already in the Phase 1 cargo-deny allow list; add an assertion test that `cargo deny check` passes with the updated `Cargo.lock`.

6. **SBOM generation**: §19 doesn't mention it. For each release artifact, generate a CycloneDX SBOM covering both Rust and npm trees:
   ```yaml
   - run: cargo cyclonedx --format json > sbom-rust.json
   - run: npx @cyclonedx/cyclonedx-npm --output-file sbom-npm.json
     working-directory: crates/gadgetron-web/web
   - run: jq -s '.[0] * .[1]' sbom-rust.json sbom-npm.json > sbom-combined.json
   ```
   Attach `sbom-combined.json` to every release. This is already a platform-wide security-lead responsibility (role doc §Core responsibilities #4); the design doc must acknowledge that `gadgetron-web` contributes to the SBOM.

**Why blocker**: `build.rs` is the only code path that runs arbitrary network + subprocess at build time; it's the biggest supply-chain surface in the workspace. §19 lists mitigations as bullet points without code. Code owns the guarantee; add the code.

---

## 3. Non-blockers

### SEC-W-NB1 — `marked` configured with `breaks: true` can cause spurious `<br>` injection; `async: false` cast to `string` is fragile

**Location**: §16 `MarkdownRenderer`

`marked.parse(content, { async: false, gfm: true, breaks: true }) as string` — the `as string` is an unchecked type assertion. In marked 12.x, `parse()` returns `string | Promise<string>`; `async: false` guarantees synchronous return, but a future marked major bump could remove the sync path and silently break. Use `marked.parse(content, opts)` then `await` it and make the renderer async (React supports async render via Suspense). For P2A, the simpler fix is: `const rawHtml = marked.parse(content, opts); if (typeof rawHtml !== 'string') throw new Error('unexpected async');`. Pin `marked` to a specific minor version (e.g., `"12.0.2"`) in §19.

`breaks: true` converts single newlines to `<br>` — this is a usability preference, not a security concern, but it interacts with DOMPurify: `<br>` is in `ALLOWED_TAGS`, so safe. Non-blocker but document the choice.

### SEC-W-NB2 — No Subresource Integrity (SRI) for any script

Since all scripts are same-origin (`script-src 'self'`) and embedded in the binary, SRI is not strictly needed. However, if P2B ever adds a CDN script (unlikely but possible for analytics), the CSP must be updated and SRI added then. Note as a P2B concern in §24 Open items.

### SEC-W-NB3 — `GADGETRON_WEB_BASE_PATH` env var is specified but Next.js `basePath` is baked at build time

§18 says `GADGETRON_WEB_BASE_PATH` and `base_path = "/web"` but Next.js `basePath` must be set at *build time* (in `next.config.mjs`). Changing it at runtime without rebuilding the frontend breaks all internal asset URLs. Either (a) remove the runtime field and document that changing base path requires `GADGETRON_WEB_BASE_PATH=... cargo build`, or (b) implement the same index.html rewriting trick as SEC-W-B5 for `basePath` too. For P2A, pick (a) and remove the env var to avoid confusion.

### SEC-W-NB4 — CSP violation reporting endpoint missing

Add `report-to csp-endpoint; report-uri /v1/csp-report` to the CSP and implement a minimal `/v1/csp-report` endpoint that logs violations to the audit log (structured field `csp_violation: CspViolationReport`). P2B scope, but open the issue now so security-lead can track.

### SEC-W-NB5 — CI license allow-list missing `0BSD`, `BlueOak-1.0.0`, `Python-2.0`

The §23 CI invocation uses `--onlyAllow 'MIT;Apache-2.0;BSD-2-Clause;BSD-3-Clause;ISC;MPL-2.0;CC0-1.0;Unlicense'`. Transitive npm deps increasingly use:
- `0BSD` (very permissive, effectively public domain) — add
- `BlueOak-1.0.0` (used by some webpack-related packages) — evaluate; probably add
- `Python-2.0` (pulled in via `readable-stream` variants sometimes) — add

Run `npx license-checker --production --summary` on the actual resolved dep tree before pinning the list. Non-blocker because the CI job will fail early if encountered, giving a chance to evaluate.

### SEC-W-NB6 — `shiki` ships a large amount of TextMate grammar JSON; bundle size not a security issue per se, but larger bundles = larger blast radius for XSS

Non-blocker but worth tracking: `shiki` grammars are JSON data, not executable JS, so they're not an XSS vector. §24 Open item #2 already tracks bundle size. Confirm the `common` language subset does not include any Turing-complete pattern-matching grammars (none do — TextMate grammars are declarative).

### SEC-W-NB7 — Cache policy for `index.html` correctly set to `no-cache`, but `404.html` / `500.html` not called out

If Next.js static export emits `404.html`, the cache policy branch in §5 `render_file` treats it as "not in `_next/static/`" and applies `no-cache, no-store, must-revalidate`. Good. Add a test: `test_serve_404_html_has_no_cache`.

### SEC-W-NB8 — `[web] enabled = true` runtime toggle duplicates `--no-default-features` compile-time toggle

§18 says `enabled` is "a runtime gate in addition to the Cargo feature". Two toggles is fine (compile-out for security-critical headless, runtime-disable for operator convenience), but document the interaction: runtime `enabled = false` with a `web-ui`-compiled binary means the `/web/*` routes still exist but return 404. Test: `test_web_routes_return_404_when_enabled_false`.

### SEC-W-NB9 — No audit-log entry for `/web/*` access

`/v1/*` endpoints emit audit log entries (Phase 1). `/web/*` static-asset GETs do not. This is fine for P2A — serving static assets is not auditworthy — but if an operator wants to audit "who accessed the chat UI", they have to use HTTP access logs. Document in §21 that static-asset access is NOT in the audit log scope.

---

## 4. GDPR / SOC2 Mapping Gaps

The design doc has **no compliance mapping section**. §10 of `00-overview.md` covers Phase 2A single-user compliance at the Kairos layer, but `gadgetron-web` introduces new controls that must be mapped:

### 4.1 GDPR obligations (new in v1)

| Obligation | Status | Required action |
|---|---|---|
| **Art 13 — information to data subject at point of collection** | Gap | The `/settings` page captures the API key (not personal data in strict sense), but any chat input IS personal data in a P2C multi-user deployment. For P2A single-user, the user = data subject = controller (same as §10). For P2C, reopen-tag needed. Add a `[P2C-SECURITY-REOPEN]` tag to §21. |
| **Art 25 — privacy by design / default** | Partial | The `no cookies` decision + same-origin + localStorage is defensible; document it in §13 as an Art 25 decision record. |
| **Art 32 — appropriate technical measures** | Partial | CSP + DOMPurify + npm audit constitute "state of the art" technical measures. Cite in §21 as the Art 32 fulfillment. |
| **Art 33 — breach notification** | N/A in P2A | No data processed on behalf of another data subject. P2C reopen. |

### 4.2 SOC2 controls (new in v1)

| Control | Status | Required action |
|---|---|---|
| **CC6.1 (logical access — wiki write)** | Inherits from §10 | No change. |
| **CC6.6 (logical access over external connections)** | Gap | The Web UI introduces a new external connection surface (browser → gateway). CSP + same-origin + HTTPS (P2C) are the controls. Add explicit CC6.6 mapping line in §21 referencing the CSP and the TLS requirement. |
| **CC6.7 (transmission of data)** | Gap | API key transmitted in `Authorization: Bearer` header over same-origin. Over plain HTTP (P2A local default), the header is in cleartext; mitigated by loopback-only binding. For P2C, TLS is required. Add a note in §18 `[web] enabled = true` that setting `bind_addr` to a non-loopback interface without TLS is a CC6.7 violation. |
| **CC7.2 (anomaly detection)** | Gap | CSP violation reports (SEC-W-NB4) would feed CC7.2. For P2A, no reports endpoint → no CSP-layer anomaly detection → acceptable but gap-flagged. |
| **CC9.2 (Vendor risk — new npm tree)** | Gap | §19 supply chain controls are the mitigation, but the **vendor risk assessment** itself is missing. Add a §19.1 sub-section "Vendor risk assessment — frontend dep tree" that lists each direct dep, its maintainer, its last release date, its known CVE history, and the risk decision. Template in `docs/process/02-document-template.md`. Required for CC9.2 closure. |

### 4.3 Required action

Add a new §25 to `03-gadgetron-web.md` titled "Compliance mapping (GDPR + SOC2)" that mirrors the structure of `00-overview.md §10`. Include:

- GDPR Art 25 decision record for localStorage-over-HttpOnly-cookies
- SOC2 CC6.6 + CC6.7 mappings
- SOC2 CC9.2 vendor risk assessment table (one row per direct npm dep)
- `[P2C-SECURITY-REOPEN]` tags for Art 13, Art 33
- Reference to §10 of `00-overview.md` for shared Phase 2A controls

Non-blocker because the design can ship without §25 for P2A single-user, BUT the TDD gate must include the vendor risk table. Flag as non-blocker SEC-W-NB1-GDPR (tracked separately so it doesn't fall through the cracks).

---

## 5. Observations (informative; not review-gating)

### OBS-1 — PM authorship

The design doc is authored by PM (not ux-interface-lead, who is the nominal owner per ADR-P2A-04 §Consequences). This is fine for a first pass but the Round 3 chief-architect review should verify that ux-interface-lead has reviewed §9 stack choices and §21 frontend STRIDE rows. Not a security concern; noted for process completeness.

### OBS-2 — `assistant-ui` version pinning

§24 Open item #6 defers the assistant-ui version pin. From a security standpoint, the pin must happen before first code PR, and the pinned version must have its own cargo-audit-equivalent review. `@assistant-ui/react` has a history of ~weekly releases; pin to a specific SHA or immutable tag, not a version range.

### OBS-3 — `localhost:8080` binding and cross-browser localStorage partitioning

Modern browsers (Chrome 115+, Firefox 103+) have started partitioning storage by top-level site in some contexts (State Partitioning / Total Cookie Protection). This is primarily about third-party contexts and does NOT affect first-party localStorage at `http://localhost:8080`. No action needed, but the test matrix should include Safari Technology Preview (which has its own ITP rules).

---

## 6. Summary Table

| ID | Title | Severity | Section |
|---|---|---|---|
| SEC-W-B1 | DOMPurify config bypass vectors + missing version floor | HIGH | §16 |
| SEC-W-B2 | CSP missing Trusted Types + inline-style audit trail | HIGH | §8, App B |
| SEC-W-B3 | `csp_connect_src` config dead-wired to static CSP string | HIGH | §7, §18 |
| SEC-W-B4 | Path traversal bypass via percent encoding; proptest insufficient | HIGH | §5, §22 |
| SEC-W-B5 | Same-origin hardcoded `/v1/*` breaks under reverse proxy | HIGH | §8 B-W2, §14, §15 |
| SEC-W-B6 | localStorage per-origin not per-path; shared-origin leak | HIGH | §13 |
| SEC-W-B7 | `build.rs` PATH+env inheritance; no sig verification | HIGH | §4, §19 |
| SEC-W-NB1 | `marked` async cast + version pin | MED | §16 |
| SEC-W-NB2 | No SRI (OK for same-origin) | LOW | §8 |
| SEC-W-NB3 | `GADGETRON_WEB_BASE_PATH` runtime flag conflicts with build-time basePath | MED | §18 |
| SEC-W-NB4 | No CSP report endpoint | MED | §8 |
| SEC-W-NB5 | License allow-list missing `0BSD`/`BlueOak-1.0.0` | LOW | §23 |
| SEC-W-NB6 | shiki bundle size (tracking only) | LOW | §24 |
| SEC-W-NB7 | 404.html cache policy not tested | LOW | §22 |
| SEC-W-NB8 | runtime `enabled = false` + compiled `web-ui` interaction not tested | LOW | §22 |
| SEC-W-NB9 | `/web/*` not audit-logged | LOW (documented) | §21 |

---

## 7. Pre-merge gate (Round 1.5 closure requirements)

Before this doc can pass Round 1.5 security review:

1. Resolve **SEC-W-B1..SEC-W-B7** in the doc text. Each fix must land as concrete spec lines (config literals, code snippets, test names) — NOT as prose promises.
2. Add §25 Compliance mapping sub-section (SEC-W-NB-GDPR).
3. Commit an updated CSP string in Appendix B with the Trusted Types directives.
4. Commit the updated `sanitize.ts` config with the pinned DOMPurify version and the tightened allow/forbid lists.
5. Commit the updated `build.rs` pseudocode with env scrubbing and PATH sanitization.
6. Commit the updated `validate_and_decode` path traversal helper in §5 and the expanded proptest in §22.
7. Commit the `api_base_path` config + the gateway `service(cfg)` signature change in §7 and §18.
8. Commit the `csp_connect_src` validation + test in §18.

Once all 8 items are addressed, re-submit for Round 1.5. Round 2 (qa-test-architect) should then review the expanded test matrix as a follow-up pass.

---

## 8. Cross-references

- ADR-P2A-04 `§Mitigations` — M-W1..M-W4 referenced throughout
- `docs/process/04-decision-log.md` D-20260414-02 — OpenWebUI→assistant-ui decision
- `docs/design/phase2/00-overview.md §8` — STRIDE table; `gadgetron-web` row placeholder superseded by §21 of this spec
- `docs/design/phase2/00-overview.md §10` — GDPR/SOC2 Phase 2A mapping; this doc must add §25 as a sibling
- `docs/reviews/phase2/round2-security-compliance-lead.md` — Round 2 review of v2 (historical; no conflict)
- OWASP Cheat Sheet: CSP, DOMPurify, Trusted Types
- CWE-22 (path traversal), CWE-79 (XSS), CWE-80 (HTML injection), CWE-829 (untrusted dep), CWE-1021 (UI redressing / clickjacking)

---

**Reviewer signature**: security-compliance-lead
**Next action**: PM (or designated author) resolves SEC-W-B1..B7 → this reviewer re-checks → Round 2 (qa-test-architect) review of the expanded test matrix → Round 3 (chief-architect + ux-interface-lead) design + UX review → TDD gate.
