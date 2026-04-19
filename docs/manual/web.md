# Gadgetron Web UI

> **상태**: Phase 2A 구현됨. 설계 문서는 `docs/design/phase2/03-gadgetron-web.md`.
> **대상 독자**: Gadgetron 운영자 / 개발자 / 전문가 사용자.
> **관련 결정**: D-20260414-02 (OpenWebUI → assistant-ui), ADR-P2A-04.

Gadgetron Web UI (`gadgetron-web`) 는 Gadgetron 바이너리에 embed 된 채팅 UI 입니다. `gadgetron serve` 한 번이면 `http://localhost:8080/web` 에서 바로 열리며, 별도 Docker 컨테이너·sibling 프로세스·외부 DB 가 필요 없습니다. 스택: [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT) + Next.js 14 + Tailwind + shadcn/ui.

로컬 canonical operator loop 는 `./demo.sh build|start|status|logs|stop` 입니다. 아래 절차도 이 루프를 기준으로 설명합니다.

---

## 전제 조건

1. **Gadgetron 바이너리가 `--features web-ui` 로 빌드되어 있어야 합니다.** 기본 빌드는 feature 가 켜져 있으므로 `./demo.sh build` 또는 `cargo build --release -p gadgetron-cli` 로 빌드하면 됩니다. 빌드 시 Node.js 20.19.0 이 PATH 에 있어야 합니다 — `crates/gadgetron-web/web/.nvmrc` 참조.
2. **Gadgetron 서버 설정 파일이 준비되어 있어야 합니다.** Web UI 자체는 `/web`를 서빙하고, `penny` 모델을 쓰려면 `gadgetron.toml`에 `[knowledge]` 섹션이 있어야 합니다. 자세한 설정은 [penny.md](penny.md)를 참조하십시오.
3. **Gadgetron API 키가 있어야 합니다.** 로컬 no-db 경로는 `gadgetron key create`, PostgreSQL 경로는 `gadgetron tenant create --name ...` 후 `gadgetron key create --tenant-id <uuid>`를 사용합니다.

---

## Origin 격리 요구사항 (중요)

> **Gadgetron 은 다른 웹 앱과 origin (scheme + host + port) 을 공유해서는 안 됩니다.**

브라우저의 `localStorage` 는 path 가 아닌 **origin** 기준으로 격리됩니다. 예를 들어 `https://internal.company.com:8080/gadgetron/` 와 `https://internal.company.com:8080/jenkins/` 가 같은 서버에 있다면, Jenkins 에서 XSS 가 발생하면 Gadgetron API 키가 탈취됩니다.

**권장 배포**:
- 단일 유저 로컬: `http://localhost:8080/web` (기본) — 같은 머신에 다른 웹 앱이 없다면 안전
- 팀/on-prem: 전용 서브도메인 (`gadgetron.example.com`) 또는 전용 포트
- SaaS/클라우드: 전용 origin 필수

---

## 보안 헤더

Gadgetron은 `/web/*` 하위의 모든 응답에 4개의 보안 헤더를 붙입니다. 구현은 `crates/gadgetron-gateway/src/web_csp.rs::apply_web_headers`에 중앙 집중되어 있으며, `/v1/*` API 응답에는 적용되지 않습니다 (설계 문서 §8 — API는 브라우저 렌더링 경로가 아님). E2E Gate 11b가 네 헤더가 실제로 응답에 붙는지 live HTTP로 검증합니다.

| 헤더 | 값 | 목적 | 표준 참조 |
|---|---|---|---|
| `Content-Security-Policy` | `default-src 'self'; base-uri 'self'; frame-ancestors 'none'; frame-src 'none'; form-action 'self'; img-src 'self' data:; font-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; connect-src 'self'; worker-src 'self' blob:; manifest-src 'self'; media-src 'self'; object-src 'none'; upgrade-insecure-requests` | XSS 공격면 축소, 프레임 내 로드 차단, 외부 오리진으로의 네트워크 연결 차단. `'unsafe-inline'`/`'unsafe-eval'`는 Next.js 하이드레이션 요구사항이며 제거 시 UI가 렌더되지 않습니다 (`csp_allows_nextjs_inline_scripts` 테스트로 잠금). | [W3C CSP Level 3](https://www.w3.org/TR/CSP3/) |
| `X-Content-Type-Options` | `nosniff` | 브라우저가 MIME 타입을 추측해서 `.txt`를 `text/html`로 오인하는 경로 차단. | [Fetch Standard §5.2](https://fetch.spec.whatwg.org/#x-content-type-options-header) |
| `Referrer-Policy` | `no-referrer` | `/web`에서 외부 링크를 클릭할 때 Gadgetron URL을 Referer 헤더로 노출하지 않음 — URL 경로·쿼리로 누출되는 세션 메타데이터 방지. | [W3C Referrer Policy](https://www.w3.org/TR/referrer-policy/) |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=()` | 카메라·마이크·위치 API를 Gadgetron 문서에서 원천 차단. 미래에 XSS가 발생해도 이들 민감 API를 탈취 경로로 쓸 수 없음. | [W3C Permissions Policy](https://www.w3.org/TR/permissions-policy/) |

검증:

```sh
curl -fsSL -D - http://localhost:8080/web/ -o /dev/null | grep -iE 'content-security-policy|x-content-type-options|referrer-policy|permissions-policy'
```

이 네 헤더가 모두 출력되어야 합니다. `/v1/*` 엔드포인트를 같은 명령으로 확인하면 모두 **없어야** 합니다.

운영 주의사항:

- **역방향 프록시에서 덮어쓰지 마십시오.** Nginx/Envoy 등이 동일한 헤더를 다른 값으로 다시 붙이면 브라우저는 첫 번째 값을 사용하지만, 일부 WAF는 이를 상충하는 정책으로 해석해 응답을 차단할 수 있습니다. 프록시는 이 헤더들을 건드리지 말고 그대로 통과시키는 것이 안전합니다.
- **CSP 완화는 설계 결정 대상입니다.** `script-src 'self' 'unsafe-inline' 'unsafe-eval'` 는 Next.js 제약 때문에 유지되고 있으며, 제거하려면 Next.js 런타임을 정적 번들 + Trusted Types로 전환하는 설계 결정이 필요합니다.

---

## 빠른 시작

### 1. Gadgetron 기동

```sh
./demo.sh start
./demo.sh status
```

단일 프로세스로 `:8080/v1/*` (OpenAI-compat API) 와 `:8080/web/*` (Web UI) 를 모두 서빙합니다. 추가 로그가 필요하면 다른 터미널에서 `./demo.sh logs -f` 를 사용하십시오.

### 2. API 키 생성

```sh
./target/release/gadgetron tenant create --name "web-demo"
./target/release/gadgetron key create --tenant-id <tenant_uuid>
```

출력된 `gad_live_...` 키를 복사하십시오. `--no-db` 같은 예외 평가 경로에서만 tenant 없이 `key create` 를 사용하십시오. 현재 canonical local demo path 는 PostgreSQL-backed 경로입니다.

### 3. 브라우저 열기

`http://localhost:8080/web` 접속. 첫 방문이면 `/settings` 로 자동 리다이렉트되어 "API 키를 입력하세요" 배너가 표시됩니다.

### 4. 키 붙여넣기 + 저장

설정 페이지에서 API 키 입력란에 `gad_live_...` 붙여넣기 → Save. 키는 브라우저 localStorage 에만 저장되며, 같은 origin (`:8080`) 위에서만 `/v1/*` 호출에 사용됩니다.

### 5. 모델 선택 후 대화 시작

- 드롭다운에서 `penny` (또는 원하는 프로바이더 모델) 선택
- 메시지 입력 → Enter
- 스트리밍 응답이 2초 이내에 시작됩니다

---

## `/web/wiki` — 브라우저 워크벤치 (wiki CRUD)

트렁크 0.2.0부터 `http://localhost:8080/web/wiki` 에서 단독 페이지로 접근할 수 있는 워크벤치 UI 입니다. PR #175/#176/#177/#179 로 쉘·디스패처·카탈로그·E2E 게이트가 함께 상자에 들어오며, 0.2.2 (ISSUE A.1) 에서 markdown 렌더가, 0.2.3 (ISSUE A.2) 에서 메인 쉘 좌측 레일 통합이 추가되었습니다.

### 접근 경로

두 경로 모두 동일한 페이지입니다 — 같은 `localStorage` API 키를 쓰고, 같은 Origin 격리 규칙을 따릅니다 (§Origin 격리 요구사항 참조).

1. **단독 URL**: `http://localhost:8080/web/wiki` — 타 UI 없이 wiki 만 쓰는 빠른 경로. 첫 방문 시 `/settings` 로 리다이렉트되어 API 키 입력을 요구합니다.
2. **메인 쉘 좌측 레일**: `http://localhost:8080/web` 접속 → 좌측 레일의 **"Wiki"** 탭 클릭. 탭은 내부적으로 `/wiki` 로 이동하여 동일한 페이지를 렌더합니다 (`LeftRailTab::Wiki` at `crates/gadgetron-web/web/app/components/shell/left-rail.tsx`). Chat 으로 돌아가려면 쉘의 "Chat" 탭을 누르거나 브라우저 뒤로가기를 누르십시오.

### 기능 (구현 완료)

페이지는 다섯 개의 워크벤치 direct-action 을 `POST /api/v1/web/workbench/actions/{id}` 로 호출합니다 (api-reference.md §POST /actions 참조). 각 버튼이 어떤 action 을 호출하는지:

| UI 조작 | Action id | Gadget | 결과 |
|---|---|---|---|
| 검색창에 쿼리 입력 → "Search" 버튼 (또는 Enter) | `knowledge-search` | `wiki.search` | 매칭 페이지 + 스니펫 목록 |
| 페이지 로드 시 자동, 또는 "Refresh" 버튼 | `wiki-list` | `wiki.list` | 왼쪽 패널의 모든 페이지 이름 목록 |
| 왼쪽 목록의 페이지 이름 클릭 | `wiki-read` | `wiki.get` | 선택한 페이지의 markdown 렌더 (`<pre>` fallback 포함) |
| "+ New page" 또는 기존 페이지 편집 → "Save" 버튼 | `wiki-write` | `wiki.write` | 새 페이지 생성 또는 덮어쓰기 |
| 페이지 컨텍스트 메뉴 "Delete" (→ 승인 흐름) | `wiki-delete` | `wiki.delete` | ISSUE 3 / v0.2.6 에 추가된 승인 게이트 액션. `pending_approval` 응답 → 승인 후 소프트 삭제. §승인 흐름 참조. |

Markdown 렌더는 `react-markdown` + `remark-gfm` (GitHub-flavoured) 입니다. 파싱이 실패하면 `<pre>` raw 블록으로 fallback — 사용자는 내용을 여전히 읽을 수 있습니다.

### 승인 흐름 (destructive action lifecycle, ISSUE 3 / v0.2.6)

`wiki-delete` 는 현재 카탈로그에서 유일한 approval-gated action 입니다 (`destructive: true`). `POST /actions/wiki-delete` 는 dispatch 하지 않고 다음 shape 을 반환합니다:

```json
{ "result": { "status": "pending_approval", "approval_id": "<uuid>", "audit_event_id": "<uuid>", ... } }
```

승인 / 거부는 전용 엔드포인트 두 개로 처리합니다 (api-reference.md §Approvals):

- **승인 → 자동 dispatch**: `POST /api/v1/web/workbench/approvals/{approval_id}/approve` (body `{}`) → 서버가 approval 을 `Approved` 로 마킹하고 저장된 args 로 `resume_approval` 진입 → 최종 `status: "ok"` + gadget payload 를 반환.
- **거부**: `POST /api/v1/web/workbench/approvals/{approval_id}/deny` (body `{"reason": "..."}` 또는 `{}`) → dispatch 하지 않고 `state: "denied"` 응답.
- 같은 `approval_id` 로 두 번째 approve/deny → HTTP 409 `workbench_approval_already_resolved`.
- 다른 tenant 의 key 로 시도 → HTTP 403 `forbidden` (원본 레코드는 변경되지 않음).

모든 terminal 경로는 `action_audit_events` 에 로그되며 `GET /api/v1/web/workbench/audit/events` 로 조회 가능합니다. E2E Gate 7h.7 이 invoke → approve → ok → 이중-approve 409 를 검증하고, Gate 7h.8 이 audit rows 를 조회합니다 (`scripts/e2e-harness/run.sh`).

## `/web/dashboard` — operator observability (ISSUE 4 / v0.2.7)

PR #194 shipped `/web/dashboard` as a sibling of `/web` 와 `/web/wiki`: tenant-scoped live tiles driven by `GET /usage/summary` (24-hour rollup over chat / direct-action / tool-call planes) plus a WebSocket feed from `GET /events/ws` that streams `ActivityEvent` frames as they publish. LeftRail adds a "Dashboard" tab next to Chat / Wiki.

**"tools" plane scope.** `/usage/summary.tools.total` aggregates `tool_audit_events` regardless of caller origin — Penny-originated tool calls (audit owner_id NULL in P2A) AND external MCP client calls via `POST /v1/tools/{name}/invoke` (audit owner_id = api_key_id, ISSUE 7 TASK 7.3 / PR #207) land on the same plane and count together. Operators querying `/audit/tool-events?tool_name=...` can still separate the two populations client-side with `owner_id` presence.

**Shipped `ActivityEvent` publishers:** `ChatCompleted` (ISSUE 4 / PR #194 — `StreamEndGuard` Drop 경로), `ToolCallCompleted` (ISSUE 5 / PR #199 — `run_gadget_audit_writer` fan-out). ISSUE 6 / PR #201 also fans out Penny tool calls to `CapturedActivityEvent` (for `/workbench/activity`) but those flow through a separate coordinator capture path — they do NOT appear as `/events/ws` frames. The `ActivityBus` broadcast channel is the live-tiles signal; the coordinator is the durable activity-feed signal.

**Auth flow — browser-specific.** The WebSocket upgrade path cannot carry an `Authorization` header from browser JavaScript, so the page uses the query-token fallback: `wss://…/events/ws?token=gad_live_…`. The auth middleware checks `?token=` **only** for this route and strips it from logs before the request-id line lands on disk (`crates/gadgetron-gateway/src/middleware/auth.rs`). Non-browser clients should keep using the standard `Authorization: Bearer …` header.

**Lag handling.** When the broadcast channel backs up (subscriber slower than publishers), the server sends a structured `{"type":"lag", "missed":N, …}` frame then closes. The page MUST reconnect and re-sync via `GET /usage/summary` — silent drops would mask a real load problem.

**E2E gates.** `scripts/e2e-harness/run.sh` Gate 7k.3 pins the `/usage/summary` response shape (all three sub-objects present, zero-state layout stable). Gate 11f renders `/web/dashboard` with an authenticated key and asserts both the pre-auth `dashboard-auth-gate` and the post-auth `dashboard` testid are addressable.

**Save / error 토스트** (ISSUE 2, 0.2.4 via PR #184 — `sonner` 라이브러리): 저장에 성공하면 우측 하단에 "Saved <page>" 토스트가, 실패하면 "Save failed" 에러 토스트가 설명과 함께 표시됩니다. 페이지 열기 실패(`wiki.get` 오류) 시에도 "Failed to open <page>" 에러 토스트가 나타납니다. DOM 의 `<section data-sonner-toaster>` 엘리먼트로 확인 가능하며, Gate 11d Playwright E2E 에서 이 DOM 을 assert 해 회귀를 막습니다.

### 인증

`/web/wiki` 는 `/web` 채팅 페이지와 **동일한** `localStorage` 키 슬롯을 씁니다. 채팅 페이지에서 API 키를 저장했다면 `/web/wiki` 는 별도 로그인 없이 바로 동작합니다. 반대로 `/web/wiki` 에서 키를 저장하면 `/web` 채팅도 해당 키로 동작합니다 — 같은 Origin 을 공유하기 때문입니다. 이 사실은 §Origin 격리 요구사항의 공격 모델과 정확히 같은 전제 위에서 성립합니다: 이 port 에 다른 웹 앱이 없어야 localStorage 키가 안전합니다.

### E2E 게이트

Gate 11d (`scripts/e2e-harness/run.sh`) 가 Playwright 로 진짜 Chromium 브라우저를 띄우고, 키 입력·페이지 생성·읽기·수정·저장·검색을 전부 자동화해서 검증합니다. Gate 11d 가 초록이면 `/web/wiki` 의 CRUD 루프 전체가 작동한다는 뜻입니다.

---

## 수동 QA 체크리스트

현재 trunk 기준 수동 확인 항목입니다 (E2E 자동화는 `tests/e2e/web_smoke.sh` 가 커버).

- [ ] `./demo.sh start` + `./demo.sh status` → `http://localhost:8080/web` 접속 → 채팅 UI 가 로드됨
- [ ] Settings 페이지 → 유효한 `gad_live_*` 키 붙여넣기 → "Saved" 확인
- [ ] 잘못된 형식 키 붙여넣기 → "Invalid format. Keys start with gad_live_ or gad_test_." 인라인 에러
- [ ] 채팅으로 돌아가 모델 드롭다운에서 `/v1/models` 결과 확인 (`penny` 포함)
- [ ] `penny` 선택 → "ping" 전송 → 스트리밍 토큰 응답 확인
- [ ] 메시지 본문에 `<script>alert(1)</script>` 포함 → alert 미발생, 텍스트로만 렌더됨 (XSS 방어 M-W1 확인)
- [ ] 네트워크 차단 → 오프라인 배너 확인
- [ ] `./demo.sh stop` 후 `./demo.sh start` → 브라우저 자동 재연결
- [ ] `curl -I http://localhost:8080/web/` → 아래 §보안 헤더의 4개 헤더가 모두 있는지 확인
- [ ] `curl -I http://localhost:8080/v1/models -H "Authorization: Bearer $KEY"` → CSP / nosniff / Referrer-Policy / Permissions-Policy **모두 없음** 확인. `/v1/*` API 응답은 `apply_web_headers` 레이어를 타지 않습니다 (설계 문서 §8).

---

## 헤드리스 빌드 (Web UI 제외)

API-only 서버를 원하면 default feature 를 끄고 빌드합니다:

```sh
cargo build --release --no-default-features -p gadgetron-cli
```

feature 토폴로지·Node.js 요구사항·`GADGETRON_SKIP_WEB_BUILD=1` fallback·검증 절차는 [installation.md §Headless build (no Web UI)](installation.md#headless-build-no-web-ui) 가 canonical 레퍼런스입니다.

---

## 긴급 복구 (키 노출 의심 시)

API 키가 노출되었다고 의심되면 (XSS 사건, 로컬 XSS, 노트북 분실 등):

1. 데이터베이스 기반 키라면 기존 키를 revoke 하고 새 키를 발급:
   ```sh
   gadgetron key revoke --key-id <old_key_id>
   gadgetron key create --tenant-id <tenant_uuid>
   ```
   현재 구현은 서버 측 validator 캐시 TTL 때문에 revoke 직후에도 최대 10분까지 이전 키가 살아 있을 수 있습니다. 즉시 무효화가 필요하면 Gadgetron 서버를 재시작하십시오.

2. no-db 모드 키였다면 저장소에 남는 키 레코드가 없으므로 revoke할 대상이 없습니다. 서버를 재시작하고 새 로컬 키를 발급하십시오.

3. 브라우저 Settings → "Clear" 버튼으로 localStorage 초기화 후 새 키를 붙여넣기

회전 이전의 감사 로그 엔트리는 유효하게 유지됩니다 (`request_id` 상관관계 보존).

---

## `[web]` 설정

```toml
[web]
enabled = true           # false = /web 라우트 미등록 (기본 true)
api_base_path = "/v1"    # 브라우저가 보는 API 경로 prefix (기본 "/v1")
```

- `enabled`: `false` 로 설정하면 `/web/*` 라우트가 등록되지 않고 404 를 반환합니다. 헤드리스 빌드와 결과가 동일합니다 (DX-W-NB4).
- `api_base_path`: 브라우저에서 본 `/v1/*` 의 경로 prefix. 기본 `/v1`. 역방향 프록시가 경로를 rewrite 하는 경우에만 변경합니다. 시작 시 `gadgetron-web` 이 embed 된 `index.html` 의 `<meta name="gadgetron-api-base">` 를 이 값으로 치환합니다 (SEC-W-B5). 반드시 `/` 로 시작해야 하며 `;`, `\n`, `\r`, `<`, `>`, `"`, `'`, backtick 을 포함할 수 없습니다 (SEC-W-B3 / SEC-W-B9). 위반 시 서버 시작 오류가 발생합니다.

---

## 트러블슈팅

### `/web` 404

- `cargo build --features web-ui` 로 빌드했는지 확인. `--no-default-features` 로 빌드했다면 Web UI 가 컴파일되지 않은 상태입니다.
- `./demo.sh build` 또는 `./demo.sh start` 를 다시 실행해 release binary 를 재생성하십시오. `start` 는 tracked source 가 더 새로우면 자동으로 rebuild 합니다.
- `gadgetron.toml` 의 `[web].enabled = true` 인지 확인 (기본값 true)

### 모델 드롭다운이 비어 있음

- `curl -sf -H "Authorization: Bearer $KEY" http://localhost:8080/v1/models | jq '.data'` 로 직접 확인
- 응답이 비어 있으면 `gadgetron.toml` 에 일반 프로바이더 블록이 없고, `penny`를 원한다면 `[knowledge]` 섹션도 빠져 있을 가능성이 큽니다. `[knowledge]` 또는 `[providers.*]`를 추가하십시오.
- 401 이면 키가 잘못됨 — `/settings` 로 돌아가 재입력

### "Gadgetron Web UI not built" 배너 표시

빌드 시 Node.js 가 PATH 에 없어서 fallback UI 가 embed 된 상태입니다. Node 20.19.0 설치 후 `cargo clean -p gadgetron-web && ./demo.sh build` 로 release binary 를 재빌드하십시오.

### API 키 잘못됨 (401)

- Settings 페이지의 red 배너에 "Your API key was rejected (401). Please enter a new one." 표시
- 새 키 입력 후 저장. 기존 localStorage 다른 항목 (theme, default model) 은 유지됨 (DX-W-NB1)

### CSP 위반 (개발자 콘솔에 CSP 에러)

- DOMPurify 가 어떤 패턴을 차단했거나, 외부 리소스 (Google Fonts 등) 로드 시도가 있었다는 뜻
- `docs/design/phase2/03-gadgetron-web.md §16` DOMPurify 설정 참조
- 브라우저 콘솔 메시지를 security-compliance-lead 에게 보고

---

## 연관 문서

- `docs/manual/penny.md` — Penny 런타임 설정·사용
- `docs/manual/installation.md` — Gadgetron 전체 설치 (헤드리스 빌드 포함)
- `docs/manual/auth.md` — API 키 생성·관리
- `docs/manual/configuration.md` — `gadgetron.toml` 전체 필드
- `docs/manual/troubleshooting.md` — 일반 트러블슈팅
- `docs/design/phase2/03-gadgetron-web.md` — 설계 상세
- `docs/adr/ADR-P2A-04-chat-ui-selection.md` — 결정 근거

---

## 변경 이력

- **2026-04-14** (D-20260414-02 + ADR-P2A-04): 신규 문서. OpenWebUI sibling process → `gadgetron-web` embed 로 전환. Origin 격리 요구사항·키 회전·XSS 방어 M-W1~M-W7 명시.
- **2026-04-19 — ISSUE 3 / v0.2.6 (PR #188) — 승인 흐름 섹션 추가**: `/web/wiki` 테이블에 `wiki-delete` (destructive, approval-gated) 행 추가 + §승인 흐름 (destructive action lifecycle) 섹션 신설 — invoke → pending_approval → approve/deny 요청 shape 와 409/403 에러 케이스, Gate 7h.7 / 7h.8 참조.
- **2026-04-19 — ISSUE 4 / v0.2.7 (PR #194) — `/web/dashboard` 페이지 섹션 추가**: Chat / Wiki / Dashboard 삼형제 레일 탭, 브라우저용 `?token=` 쿼리-토큰 auth fallback, `/events/ws` 랙 프레임 프로토콜, Gate 7k.3 / 11f 참조. ISSUE 5 / v0.2.8 (PR #199) 후속으로 shipped `ActivityBus` 퍼블리셔 (`ChatCompleted` + `ToolCallCompleted`) 열거 + ISSUE 6 / v0.2.9 (PR #201) `CapturedActivityEvent` 갈림길 명시 (broadcast bus 가 아닌 coordinator 경유).
