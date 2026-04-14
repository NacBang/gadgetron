# Kairos 개인 비서 (Phase 2A)

> **상태**: Phase 2A MVP 설계 완료, 구현 예정.
> **대상 독자**: Gadgetron 운영자 / 단일 유저 로컬 사용자.
> **설계 출처**: `docs/design/phase2/00-overview.md` v3, `01-knowledge-layer.md` v3, `02-kairos-agent.md` v3, ADR-P2A-01/02/03.

Kairos는 Gadgetron이 Phase 1(다중 프로바이더 LLM 게이트웨이)을 넘어 **지식 레이어 기반의 개인 비서 플랫폼**으로 올라가는 Phase 2의 첫 단계(P2A)입니다. 사용자가 Gadgetron에 내장된 **`gadgetron-web` 웹 UI** (`http://localhost:8080/web`) 에서 `kairos` 모델을 선택해 대화하면, Gadgetron은 Claude Code CLI를 서브프로세스로 띄워 로컬 지식(md 위키)과 웹 검색(SearXNG)을 MCP 도구로 제공합니다. Claude Code 자체가 agent 루프를 돌리고, Rust 코드는 도구와 subprocess 관리만 책임집니다.

- 하방(Phase 1): LLM 게이트웨이 — 외부 API 클라이언트, SDK 유저, 운영자 대상.
- 상방(Phase 2): 개인 비서 플랫폼 — `gadgetron-web` (assistant-ui 기반, 단일 바이너리 embed) 채팅으로 최종 사용자 대상 (D-20260414-02).

같은 Gadgetron 바이너리가 두 레이어를 모두 제공합니다.

---

## 프라이버시 및 보안 경고 (Privacy & Security)

Kairos를 사용하기 전에 **반드시** 다음 두 고지를 읽으십시오. 이 고지들은 ADR-P2A-02 / ADR-P2A-03의 pre-merge gate 요구사항으로, 설계 결정의 일부입니다.

### 고지 1 — 위키 git 이력은 영구적입니다 (Wiki git history is permanent)

> **Permanence note**: Every wiki page you (or Kairos on your behalf) write is committed to a local git repository at `~/.gadgetron/wiki/`. Git history is **permanent**. If you accidentally write a secret (API key, password, private note you later regret) into a wiki page, editing or deleting the page does NOT remove it from git history — the old version remains accessible via `git log`. Removing content from git history requires explicitly rewriting history with `git filter-repo` or BFG Repo-Cleaner, which is destructive and cannot be undone.
>
> **Never write secrets into wiki pages.** Treat the wiki as a permanent, append-only ledger. If you need to record something sensitive that you expect to delete later, store it outside the wiki (e.g., a password manager).

**한국어 요약**: Kairos가 사용자를 대신해 작성하는 모든 위키 페이지는 `~/.gadgetron/wiki/` 로컬 git 저장소에 커밋되며, **git 이력은 영원히 남습니다**. 실수로 API 키·비밀번호·나중에 후회할 개인 메모를 위키에 쓴 경우, 페이지를 편집하거나 삭제해도 이전 버전이 `git log`에 남아 있습니다. 이력을 진짜로 지우려면 `git filter-repo` / BFG Repo-Cleaner로 파괴적 재작성이 필요하며, 이는 되돌릴 수 없습니다. **위키에 비밀을 쓰지 마십시오.** 위키는 영구 append-only 원장으로 취급하고, 비밀번호 매니저 같은 별도 저장소를 사용하십시오.

또한 Kairos는 Gadgetron이 미리 정의한 Credential BLOCK 패턴 (PEM private keys, AWS access keys, GCP service accounts 등)을 포함한 위키 쓰기를 자동으로 거부합니다 (HTTP 422, `wiki_credential_blocked`). 이것은 마지막 방어선일 뿐이며, 사용자의 주의가 가장 중요합니다.

### 고지 2 — 웹 검색 시 쿼리는 외부 검색 엔진으로 전달됩니다 (SearXNG privacy)

> **Privacy note**: Web search via Kairos proxies your queries through SearXNG to the search engines configured in your SearXNG instance (by default: Google, Bing, DuckDuckGo, Brave — but your administrator may have enabled different engines). Queries are anonymized at the SearXNG layer, but the search engines receive the query text. SearXNG may log queries depending on its own configuration. Gadgetron itself does not store your search queries. If you need stricter privacy, disable `web_search` by leaving `searxng_url` unset in your config.

**한국어 요약**: Kairos의 웹 검색은 SearXNG를 통해 이루어지며, SearXNG가 설정된 검색 엔진 (기본값: Google, Bing, DuckDuckGo, Brave — 관리자가 변경했을 수 있음) 에 쿼리 텍스트가 전달됩니다. IP와 User-Agent는 SearXNG 계층에서 익명화되지만 **쿼리 본문은 검색 엔진이 받습니다**. SearXNG 자체도 설정에 따라 쿼리를 로그에 남길 수 있습니다. Gadgetron 자체는 검색 쿼리를 저장하지 않습니다. 더 엄격한 프라이버시가 필요하면 `gadgetron.toml`의 `[knowledge.search]` 블록에서 `searxng_url`을 설정하지 마십시오 — 그러면 `web_search` MCP 도구 자체가 Claude Code에 노출되지 않습니다.

### 고지 3 — `--dangerously-skip-permissions` 플래그 사용 (ADR-P2A-02)

Gadgetron은 Claude Code를 `--dangerously-skip-permissions` 플래그와 함께 실행합니다. 이 플래그는 Claude Code의 대화형 권한 확인 프롬프트를 건너뛰어 headless 서버 환경에서 동작하게 합니다. 이 플래그의 위험은 **`--allowed-tools` 화이트리스트가 바이너리 레벨에서 강제되기 때문에** 크게 제한됩니다 — 2026-04-13 `claude 2.1.104`에 대한 행동 테스트로 확인되었습니다 (ADR-P2A-01).

**운영자 책임**:
- `gadgetron serve`는 **비특권 OS 사용자**로 실행하십시오 (root 금지). systemd를 사용하면 `User=gadgetron`, docker-compose면 `user: gadgetron`, Kubernetes면 `securityContext.runAsUser`를 설정하십시오.
- 서버 프로세스의 파일시스템 권한을 `~/.gadgetron/` (위키·설정·감사 로그), `~/.claude/` (OAuth 세션, 가능하면 읽기 전용), `$TMPDIR`로 제한하십시오.
- 단일 유저 로컬 데스크톱 환경에서는 `cargo run` / 직접 실행 사용자가 이미 `~/.gadgetron/` 소유자이므로 이 요건이 자동 충족됩니다.

---

## 전제 조건 (Prerequisites)

1. **Phase 1 Gadgetron이 설치·동작 중**: `docs/manual/quickstart.md`와 `installation.md`를 먼저 완료하십시오.
2. **Claude Code CLI 설치**: `claude` 바이너리가 `PATH`에 있어야 합니다. 공식 설치 가이드는 https://docs.claude.com/en/docs/claude-code 를 참조하십시오.
3. **Claude Code 로그인 완료**: `claude login`을 실행해 `~/.claude/` 아래에 OAuth 자격증명을 저장해야 합니다. 사용자의 Claude Max 구독이 Kairos의 "두뇌"를 담당하므로 API 키 과금이 발생하지 않습니다.
4. **최소 Claude Code 버전**: `2.1.104` 이상. `gadgetron serve`는 시작 시 `claude --version`을 실행해 버전을 확인하고, 미만이면 거부합니다 (ADR-P2A-01 `CLAUDE_CODE_MIN_VERSION`).
5. **(선택) Docker**: SearXNG 를 컨테이너로 띄우려면 Docker 또는 Podman이 편리합니다. 네이티브 SearXNG 설치도 지원됩니다. **Web UI 는 Docker 없이 Gadgetron 바이너리에 이미 embed 되어 있습니다** — `gadgetron serve` 한 번이면 `http://localhost:8080/web` 에서 바로 열립니다 (D-20260414-02, 구 OpenWebUI 계획 폐기).

---

## 빠른 시작 (Quick Start)

Phase 1 Gadgetron이 이미 동작 중이라고 가정합니다.

### 1. Kairos 워크스페이스 초기화

```sh
./target/release/gadgetron kairos init
```

성공 시 아래와 유사한 출력이 나옵니다:

```
[OK] Wiki directory: /Users/you/.gadgetron/wiki (created, git init done)
[OK] Config written: /Users/you/.gadgetron/gadgetron.toml (kairos + knowledge sections added)
[OK] Claude Code CLI detected: /Users/you/.local/bin/claude (2.1.104)
[OK] Ready. Next: run `gadgetron serve --config ~/.gadgetron/gadgetron.toml`
```

실패 경로와 해결 방법은 `docs/design/phase2/01-knowledge-layer.md` §1.1 및 본 매뉴얼 §트러블슈팅을 참조하십시오.

옵션 플래그:
- `--wiki-path <PATH>`: 기본 `~/.gadgetron/wiki` 대신 다른 디렉터리를 사용합니다. `gadgetron.toml`이 이미 `wiki_path`를 지정한 경우 CLI 값이 우선합니다.
- `--docker`: **P2A 에서 미지원** (D-20260414-02, DX-W-B3 Option C). 호출 시 stderr 에 경고 출력 후 exit 0. Web UI 는 `gadgetron serve` 가 자동으로 embed 된 채 서빙하므로 별도 compose 가 필요 없습니다. SearXNG 는 `docker run -d --rm --name searxng -p 127.0.0.1:8888:8080 searxng/searxng` 로 수동 기동하십시오. P2B 에서 SearXNG-only 모드로 재도입될 가능성이 있습니다.

Phase 1 `gadgetron doctor`는 `[ok]` / `[FAIL]` 소문자 스타일을 쓰고, Phase 2 `kairos init`은 `[OK]` / `[WARN]` / `[FAIL]` 대문자를 씁니다. 의도된 차이입니다.

### 2. (선택) SearXNG 기동

웹 검색 MCP 도구를 쓰려면 로컬 SearXNG 가 필요합니다. Docker 예시:

```sh
docker run -d --rm --name searxng -p 127.0.0.1:8888:8080 searxng/searxng
```

네이티브 설치도 가능합니다 (SearXNG 공식 문서 참조). SearXNG 가 없어도 Kairos 는 동작하며, `web_search` MCP 도구만 노출되지 않습니다.

설정: `gadgetron.toml` 의 `[knowledge.search].searxng_url = "http://127.0.0.1:8888"`.

### 3. API 키 생성

`gadgetron-web` 이 Gadgetron 에 인증하려면 API 키가 필요합니다:

```sh
./target/release/gadgetron key create --tenant-id default
```

출력된 키(`gad_live_...`)를 복사해 두십시오. 5 단계에서 브라우저에 붙여넣습니다.

상세: `docs/manual/auth.md`.

### 4. Gadgetron 서버 기동 (단일 바이너리)

```sh
./target/release/gadgetron serve --config ~/.gadgetron/gadgetron.toml
```

이 한 줄이 `:8080/v1/*` (OpenAI-compat API) 와 `:8080/web/*` (`gadgetron-web` — assistant-ui 기반 채팅 UI) 를 모두 서빙합니다. 별도 컨테이너·sibling 프로세스·docker-compose 필요 없음 (D-20260414-02).

**팁**: `kairos init`이 생성한 `gadgetron.toml`은 kairos 전용 라우팅 전략을 설정합니다 (`default_strategy = { type = "fallback", chain = ["kairos"] }`). 다른 프로바이더를 추가할 때는 `round_robin`과 kairos를 같은 풀에 넣지 마십시오 — Claude Code subprocess가 같은 풀의 다른 일반 LLM과 섞이면 혼란스러운 실패가 발생합니다. 자세한 내용은 `docs/design/phase2/02-kairos-agent.md` §11을 보십시오.

### 5. `gadgetron-web` 에서 첫 대화

1. 브라우저에서 `http://localhost:8080/web` 접속. `gadgetron-web` 의 채팅 UI 가 로드됩니다 (assistant-ui 컴포넌트 기반, shadcn + Tailwind 스타일).
2. 설정 아이콘 → API 키 입력란에 3 단계에서 복사한 `gad_live_...` 를 붙여넣고 저장. 키는 브라우저 localStorage 에만 저장되며, 같은 origin(`:8080`) 위에서만 `/v1/*` 호출에 사용됩니다.
3. 모델 드롭다운이 `/v1/models` 를 통해 Gadgetron 의 모델 목록을 가져옵니다 — 기존 `vllm/...`, `sglang/...` 등과 함께 **`kairos`** 가 보여야 합니다.
4. 모델로 `kairos` 선택 후 "내가 어제 뭐 했는지 위키에서 찾아줘" 같은 프롬프트를 보내보십시오.
5. 내부 흐름: `gadgetron-web` 브라우저 → `POST /v1/chat/completions (model=kairos, stream=true)` (같은 origin, localStorage 의 Bearer 토큰) → `gadgetron-gateway` → `gadgetron-router` → `gadgetron-kairos::provider` → `claude -p` subprocess → MCP tools (`wiki_search`, `wiki_get`, 필요 시 `web_search`) → stream-json 이벤트를 SSE 로 변환해 `gadgetron-web` 에 스트리밍.

첫 실행에서 Claude Code가 몇 초 지연될 수 있습니다 (모델 콜드 스타트). 두 번째 실행부터는 1초 안쪽이 일반적입니다.

---

## 설정 참조 (`[kairos]` / `[knowledge]` / `[knowledge.search]`)

`gadgetron.toml`에 다음 블록을 추가합니다 (`kairos init`이 자동 생성):

```toml
[knowledge]
# 위키 루트. 없으면 kairos init이 생성하고 git init 실행.
# env: GADGETRON_KNOWLEDGE_WIKI_PATH
wiki_path = "/home/you/.gadgetron/wiki"

# 위키 쓰기마다 자동 git 커밋 여부.
# env: GADGETRON_KNOWLEDGE_WIKI_AUTOCOMMIT
wiki_autocommit = true

# 위키 페이지 최대 바이트. [1, 100 MiB]. 기본 1 MiB.
# env: GADGETRON_KNOWLEDGE_WIKI_MAX_PAGE_BYTES
wiki_max_page_bytes = 1_048_576

# 선택: 커스텀 git author. 미설정 시 git config의 global author 사용.
# env: GADGETRON_KNOWLEDGE_WIKI_GIT_AUTHOR
# wiki_git_author = "Your Name <you@example.com>"

[knowledge.search]
# SearXNG 인스턴스 URL. 미설정 시 web_search MCP 도구 자체가 노출되지 않음 (프라이버시 opt-out).
# env: GADGETRON_KNOWLEDGE_SEARXNG_URL
searxng_url = "http://127.0.0.1:8888"

# 쿼리당 타임아웃 초. [1, 60]. 기본 10.
# env: GADGETRON_KNOWLEDGE_SEARCH_TIMEOUT_SECS
timeout_secs = 10

# 쿼리당 최대 결과 수. [1, 50]. 기본 5.
# env: GADGETRON_KNOWLEDGE_SEARCH_MAX_RESULTS
max_results = 5

[kairos]
# Claude Code 바이너리. 절대 경로 또는 basename (PATH 검색).
# 검증 규칙: 셸 메타문자 금지, `-` 접두사 금지, 상대 경로 금지.
# env: GADGETRON_KAIROS_CLAUDE_BINARY
claude_binary = "claude"

# 선택: ANTHROPIC_BASE_URL 오버라이드. None = 기본 Claude Max 세션.
# env: GADGETRON_KAIROS_CLAUDE_BASE_URL
# claude_base_url = "http://127.0.0.1:4000"

# 선택: Claude 모델 오버라이드 (`--model` flag). None = Claude Code 기본값.
# env: GADGETRON_KAIROS_CLAUDE_MODEL
# claude_model = "claude-opus-4-6"

# 요청당 subprocess wallclock 상한 초. [10, 3600]. 기본 300.
# env: GADGETRON_KAIROS_REQUEST_TIMEOUT_SECS
request_timeout_secs = 300

# 동시 Claude Code subprocess 최대 수. [1, 32]. 기본 4 (P2A 데스크톱).
# 초과 시 큐잉 또는 HTTP 503.
# env: GADGETRON_KAIROS_MAX_CONCURRENT_SUBPROCESSES
max_concurrent_subprocesses = 4
```

전체 필드 설명은 `docs/design/phase2/02-kairos-agent.md` §10과 `01-knowledge-layer.md` §7을 참조하십시오.

---

## 트러블슈팅 (Troubleshooting)

아래 표는 사용자가 마주칠 가능성이 높은 에러와 대응 방법입니다. 더 상세한 표는 `docs/design/phase2/00-overview.md` §12에 있습니다.

| HTTP | `error.code` | 의미 | 대응 |
|---|---|---|---|
| 503 | `kairos_not_installed` | `claude` 바이너리가 PATH에서 발견되지 않음 | Claude Code를 설치하고 `claude login` 실행. `which claude` 로 PATH 확인. |
| 503 | `kairos_spawn_failed` | 바이너리는 있으나 subprocess 기동 실패 | `gadgetron serve`를 `RUST_LOG=gadgetron_kairos=debug` 로 재실행해 진단 로그 확인, `journalctl -u gadgetron` 또는 docker logs도 확인. |
| 500 | `kairos_agent_error` | Claude Code가 오류 종료 | 재시도. 반복되면 `~/.claude/` 세션 만료 가능성 — `claude login` 재실행. |
| 504 | `kairos_timeout` | `request_timeout_secs` 초과 | 프롬프트를 단순화하거나 `request_timeout_secs` 값을 늘리십시오 (최대 3600). |
| 422 | `wiki_credential_blocked` | 위키 쓰기 본문에 credential 패턴 (PEM / AWS / GCP) 검출 | 해당 비밀 문자열을 제거하고 재시도. 프라이버시 고지 1 참조. |
| 413 | `wiki_page_too_large` | `wiki_max_page_bytes` 초과 | 페이지를 여러 개로 분할하거나 `wiki_max_page_bytes` 값을 늘리십시오 (최대 100 MiB). |
| 409 | `wiki_conflict` | 동시 쓰기로 git merge conflict 발생 | 위키 디렉터리에서 `git status` 실행, conflict 해결 후 재시도. |
| 400 | `wiki_invalid_path` | 위키 경로에 `..`, 절대 경로, 또는 특수 문자 포함 | 위키 페이지 이름은 단순 `foo/bar.md` 형태만 사용하십시오. |
| 503 | `wiki_git_corrupted` | 위키 git 저장소 상태가 일관되지 않음 (locked index / detached HEAD / missing objects 등). Disk full 가능성도 포함 | 위키 디렉터리에서 `git status` 실행 또는 디스크 공간·파일시스템 권한 확인. 필요 시 수동 복구. |

**Claude Code 버전 에러**: 서버 시작 시 `"claude CLI version X is below the minimum 2.1.104 required for --allowed-tools enforcement per ADR-P2A-01"` 가 나오면 Claude Code를 업그레이드하십시오 (`claude update` 또는 공식 설치 스크립트 재실행).

**SearXNG 차단**: 방화벽/프록시가 SearXNG 포트(기본 `127.0.0.1:8888`)를 막으면 `web_search` MCP 도구가 실패합니다. `curl http://127.0.0.1:8888/search?q=test&format=json` 로 직접 확인하십시오.

**`gadgetron-web` 이 `kairos` 모델을 못 봄**: 브라우저가 `/v1/models` 엔드포인트를 호출해 모델 목록을 가져오는지 확인하십시오. Gadgetron 이 기동되지 않았거나 `gadgetron.toml` 의 `[kairos]` 블록이 비어 있으면 모델이 나타나지 않습니다. `curl -H "Authorization: Bearer $KEY" http://localhost:8080/v1/models | jq '.data[] | .id'` 로 직접 확인. 브라우저 DevTools → Network 탭에서 `/v1/models` 요청의 응답을 확인하는 것도 도움이 됩니다. `/web` 이 404 면 `gadgetron-gateway` 가 `web-ui` Cargo feature 없이 빌드된 것이므로 `cargo build --features web-ui` 로 재빌드하십시오.

---

## FAQ

**Q. Kairos가 Claude API 과금을 발생시키나요?**
아니요. Kairos는 사용자의 `~/.claude/` OAuth 세션(Claude Max 구독)을 사용합니다. Rust 코드는 subprocess 관리와 MCP 도구만 제공하며, Anthropic API 직접 호출은 없습니다.

**Q. Kairos가 위키에 뭘 쓸지 어떻게 결정하나요?**
Claude Code agent 루프가 결정합니다. 사용자가 "이 아이디어를 기억해줘"라고 말하면 Claude Code가 `wiki_write` MCP 도구를 호출합니다. Gadgetron은 경로 검증 (M3) + credential 패턴 차단 (M5) + 크기 제한 (M1) 만 강제합니다. 자세한 내용은 `docs/design/phase2/01-knowledge-layer.md` §4.4.

**Q. 다른 LLM 프로바이더(OpenAI, vLLM 등)와 동시에 쓸 수 있나요?**
예. kairos는 `gadgetron-router`의 provider map에 `"kairos"` 이름으로 등록된 일반 LlmProvider 구현입니다. 기존 프로바이더와 공존합니다. 단, routing 전략으로 `round_robin` + kairos 조합은 피하십시오 (설정 §4 팁).

**Q. 멀티 유저 / 클라우드 배포는 가능한가요?**
P2A는 **단일 유저 로컬**만 공식 지원합니다. 멀티 유저(Per-tenant 격리), 클라우드 오브젝트 스토리지(S3/GCS), 팀 공유 위키는 P2C+ 로드맵입니다. `docs/design/phase2/00-overview.md` §13 로드맵 참조.

**Q. 위키를 외부 git 호스트에 백업할 수 있나요?**
직접 지원은 P2A에 없지만, 위키가 로컬 git 저장소이므로 사용자가 직접 `git remote add` + `git push`로 GitHub/Gitea 등에 푸시할 수 있습니다. 단, **고지 1**의 영구성 경고를 다시 읽어주십시오 — git 이력이 공개 저장소로 푸시되면 되돌릴 수 없습니다.

---

## 연관 문서

| 문서 | 내용 |
|---|---|
| `docs/design/phase2/00-overview.md` | Phase 2 전체 설계 개요, STRIDE 위협 모델, GDPR/SOC2 매핑, 에러 테이블 |
| `docs/design/phase2/01-knowledge-layer.md` | `gadgetron-knowledge` crate 상세 (wiki/MCP/search) |
| `docs/design/phase2/02-kairos-agent.md` | `gadgetron-kairos` crate 상세 (provider/session/stream) |
| `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md` | `--allowed-tools` 강제 검증 결과 (PASS on 2.1.104) |
| `docs/adr/ADR-P2A-02-dangerously-skip-permissions-risk-acceptance.md` | `--dangerously-skip-permissions` risk acceptance + non-root 전제 |
| `docs/adr/ADR-P2A-03-searxng-privacy-disclosure.md` | SearXNG 프라이버시 고지 ADR |
| `docs/manual/auth.md` | API 키 / 테넌트 / scope 시스템 |
| `docs/manual/quickstart.md` | Phase 1 5분 빠른 시작 |
| `docs/manual/troubleshooting.md` | Phase 1 공통 에러 해결 |
| `docs/manual/web.md` | Gadgetron Web UI (`/web`) 설정, Origin 격리, 키 회전, 헤드리스 빌드 — D-20260414-02 이후 신규 |
| `docs/design/phase2/03-gadgetron-web.md` | `gadgetron-web` crate 설계 (assistant-ui + Next.js embed) |
| `docs/adr/ADR-P2A-04-chat-ui-selection.md` | OpenWebUI → assistant-ui 전환 근거 |

---

## 변경 이력

- **2026-04-13 — v3 매뉴얼 초안**: Round 2 크로스리뷰 사이클 + ADR-P2A-01 M4 behavioral verification (PASS on claude 2.1.104) + 프라이버시 고지 2종 + 트러블슈팅 표. 구현 PR 머지 전 pre-merge gate 요건 충족.
- **2026-04-14 — D-20260414-02 / ADR-P2A-04**: OpenWebUI sibling process 제거, `gadgetron-web` (assistant-ui + Next.js embed) 으로 전환. §2 Docker 전제 조건 갱신 (Web UI 는 Docker 불필요), §4 단일 바이너리 서버 기동, §5 `gadgetron-web` 기반 첫 대화 단계로 전면 재작성. `--docker` 플래그 P2A 미지원 처리 (graceful deprecation). `web.md` 및 `03-gadgetron-web.md` 연관 문서 추가.
