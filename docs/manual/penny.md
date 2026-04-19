# Penny 런타임 (Phase 2A)

> **상태**: trunk `0.2.0` 기준 구현됨.
> **현재 CLI 계약**: `gadgetron penny ...` 계열 서브커맨드는 없습니다. Penny는 `gadgetron serve`가 `gadgetron.toml`의 `[knowledge]` 섹션을 읽어 등록하고, `gadgetron gadget serve`는 Claude Code가 호출하는 child-side stdio 서버입니다 (`gadgetron mcp serve`는 deprecated alias — ADR-P2A-10).

Penny는 Claude Code CLI를 Gadgetron의 `model = "penny"` 뒤에 붙인 런타임입니다. 사용자가 Penny로 채팅을 보내면 Gadgetron은 Claude Code를 서브프로세스로 띄우고, 로컬 markdown 위키와 선택적 SearXNG 검색을 MCP 도구로 제공합니다. 향후 P2C/P3에서는 같은 런타임이 infra/scheduler/cluster 도구까지 확장되는 방향을 전제합니다.

로컬 canonical operator loop 는 `./demo.sh build|start|status|logs|stop` 이며, 기본 데모 경로는 pgvector-enabled PostgreSQL 을 전제로 합니다. no-db 경로는 빠른 평가용 예외 경로이지 기본 운영 경로가 아닙니다.

---

## 프라이버시 및 보안

### 위키 git 이력은 영구적입니다

Penny가 쓰는 위키는 로컬 git 저장소입니다. 한 번 저장된 내용은 페이지를 수정하거나 삭제해도 git 이력에 남습니다. 비밀값이나 나중에 지우고 싶은 민감한 메모는 위키에 쓰지 마십시오.

Gadgetron은 PEM private key, AWS access key, GCP service-account JSON 같은 일부 패턴을 감지하면 위키 쓰기를 차단하지만, 이것은 마지막 방어선일 뿐입니다.

### 웹 검색은 외부 엔진으로 전달됩니다

`[knowledge.search]`를 설정하면 Penny의 `web.search` 도구가 SearXNG를 통해 외부 검색 엔진에 질의를 보냅니다. Gadgetron 자체는 검색 쿼리를 저장하지 않지만, SearXNG와 그 뒤의 검색 엔진은 쿼리를 볼 수 있습니다. 더 엄격한 프라이버시가 필요하면 `[knowledge.search]`를 설정하지 마십시오.

### 운영 권장사항

- `gadgetron serve`는 비특권 OS 사용자로 실행하십시오.
- 위키 디렉터리와 Claude Code 세션 디렉터리(`~/.claude/`) 접근 권한을 최소화하십시오.
- Phase 2A의 Ask approval flow는 아직 없습니다. Write/Destructive permission UX는 Phase 2B 범위입니다.

### 내장 도구 차단 (`PENNY_DISALLOWED_TOOLS`)

Penny는 Claude Code에 딸려오는 내장 도구(`Bash`, `Read`, `Write`, `Edit`,
`Glob`, `Grep`, `WebSearch`, `WebFetch`, `NotebookEdit`, `Task`,
`TodoWrite`, `Agent`, `ToolSearch`)를 **모두 `--disallowed-tools`로 명시
차단**합니다. Penny는 오직 MCP `knowledge` 서버의 도구(`wiki.*` + 선택적
`web.search`)만 호출해야 하기 때문입니다.

이 차단의 보안/UX 효과:

- **Bash 실행 경로 차단**: 프롬프트 인젝션을 통한 셸 명령 실행 불가.
- **SearXNG 우회 차단**: 내장 `WebSearch`가 ADR-P2A-03의 프라이버시 고지를
  거치지 않고 외부 검색 엔진을 호출하던 경로 봉쇄. 웹 검색은 오직
  `[knowledge.search]`에 명시한 SearXNG 엔드포인트로만 가능합니다.
- **파일시스템 무단 접근 차단**: `Read`/`Write`/`Edit`/`Glob`/`Grep`가
  운영자 홈 디렉터리를 읽거나 쓸 수 없습니다. 모든 파일 작업은 MCP
  `wiki.*` 도구를 통해 `wiki_path` 내부로만 가능합니다.
- **"Not connected" UI 드롭 버그 방지**: 내장 `WebSearch`가 실행 환경에서
  바인딩에 실패할 때 스트림에 흘리던 `❌ _Not connected_` orphan
  tool_result가 Web UI의 답변을 통째로 drop시키던 회귀를 구조적으로
  제거합니다.

차단 목록은 `crates/gadgetron-penny/src/spawn.rs::PENNY_DISALLOWED_TOOLS`
상수에 중앙 집중되어 있습니다. 감사 시 이 상수의 diff로 허용/차단 집합
변화를 추적합니다.

---

## 전제 조건

1. Gadgetron이 빌드되어 있어야 합니다. 설치는 [installation.md](installation.md), 기본 API 경로는 [quickstart.md](quickstart.md)를 참조하십시오.
2. `claude` 바이너리가 `PATH`에 있어야 합니다.
3. `claude login`이 완료되어 있어야 합니다. 기본 brain mode는 사용자의 Claude Max/OAuth 세션을 사용합니다.
4. Claude Code 버전은 `2.1.104` 이상이어야 합니다.
5. `gadgetron.toml`에 `[knowledge]` 섹션이 있어야 합니다. 이 섹션이 없으면 Penny는 `/v1/models`에 등록되지 않습니다.
6. 선택적으로 `web.search`를 쓰려면 SearXNG 인스턴스를 준비하십시오.

---

## 빠른 시작

### 1. 기본 로컬 런타임 준비

pgvector-enabled PostgreSQL 기동 절차는 [installation.md §Step 4 PostgreSQL setup](installation.md#step-4-postgresql-setup) (Ubuntu) 또는 [installation.md §Step 5 PostgreSQL setup](installation.md#step-5-postgresql-setup) (macOS) 을 canonical path 로 따르십시오. 컨테이너가 올라오면 Gadgetron 빌드 + baseline 설정 생성만 실행합니다:

```sh
./demo.sh build
./target/release/gadgetron init --yes
```

### 2. 설정 파일 준비

`gadgetron init`은 아직 `[agent]` / `[knowledge]` 블록을 자동으로 생성하지 않습니다. 따라서 `init`이 만든 baseline `gadgetron.toml`에 아래 블록을 **추가**해야 합니다.

```sh
mkdir -p .gadgetron
cat > gadgetron.toml <<'TOML'
[server]
bind = "127.0.0.1:8080"

[agent]
binary = "claude"
claude_code_min_version = "2.1.104"
request_timeout_secs = 300
max_concurrent_subprocesses = 4

[agent.brain]
mode = "claude_max"

[knowledge]
wiki_path = "./.gadgetron/wiki"        # config 파일 디렉터리 기준 상대 경로 지원
wiki_autocommit = true
wiki_max_page_bytes = 1048576

# [knowledge.search]
# searxng_url = "http://127.0.0.1:8888"
# timeout_secs = 10
# max_results = 10
TOML
```

`wiki_path`의 부모 디렉터리는 미리 존재해야 합니다. 위 예시에서는 `mkdir -p .gadgetron`이 그 역할을 합니다.

### 3. API 키 준비

Canonical PostgreSQL-backed 경로:

```sh
./target/release/gadgetron tenant create --name "my-team"
./target/release/gadgetron key create --tenant-id <tenant-uuid>
```

빠른 평가용 no-db 예외 경로에서는 `./target/release/gadgetron key create` 만으로도 키를 만들 수 있지만, 이 문서의 기본 경로는 PostgreSQL-backed 경로입니다.

출력된 `gad_live_...` 키를 보관하십시오.

### 4. 서버 기동

```sh
export GADGETRON_DATABASE_URL="postgres://gadgetron:secret@127.0.0.1:5432/gadgetron_demo"
./demo.sh start
./demo.sh status
```

정상 등록 시 로그에 `penny: registered`가 나타나고, `/v1/models`에 `penny`가 포함됩니다. 상세 로그가 필요하면 `./demo.sh logs -f` 를 사용하십시오.

### 5. 첫 요청 보내기

```sh
curl -sN http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer gad_live_<your_key>" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "penny",
    "stream": true,
    "messages": [
      {"role":"user","content":"wiki에 오늘 회의 내용을 저장해줘"}
    ]
  }'
```

브라우저를 선호하면 [web.md](web.md)의 `/web` UI를 사용하면 됩니다.

---

## `gadgetron gadget serve`

`gadgetron gadget serve`는 일반 운영자가 직접 쓰는 주 명령은 아닙니다. Claude Code가 Penny 요청마다 child process로 호출하는 stdio JSON-RPC 서버입니다. 다만 수동 진단에는 유용합니다.

> **참고**: `gadgetron mcp serve`는 deprecated alias입니다 (ADR-P2A-10). v0.3–v0.4에서는 동작하지만 v0.5에서 제거될 예정입니다. 스크립트·systemd unit·MCP config에서 이 명령을 사용하고 있다면 `gadgetron gadget serve`로 업데이트하십시오.

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | ./target/debug/gadgetron gadget serve --config gadgetron.toml
```

이 명령은 `gadgetron.toml`이 존재하고 `[knowledge]` 섹션이 유효해야만 동작합니다. 현재 CLI에는 `gadgetron penny init`이 없으므로, 설정 파일은 직접 준비해야 합니다.

---

## 설정 요약

Penny 런타임이 읽는 설정 블록은 아래와 같습니다. 각 필드의 타입·범위·기본값·validation 규칙은 [configuration.md](configuration.md)가 canonical 레퍼런스입니다. 본 섹션은 Penny 관점에서 어떤 블록이 필수/선택인지, 누락 시 어떤 동작이 되는지만 요약합니다.

| 블록 | 필수 여부 | Penny에 미치는 영향 | 상세 |
|---|---|---|---|
| `[agent]` | 권장 | Claude Code subprocess 런타임 한도 (`binary`, `request_timeout_secs`, `max_concurrent_subprocesses`, session 필드). 누락 시 모두 기본값 사용 | [configuration.md §`[agent]`](configuration.md#agent) |
| `[agent.brain]` | 권장 | Penny brain 모드 (`claude_max` 기본 / `external_anthropic` / `external_proxy` / `gadgetron_local`은 P2A에서 startup error) | [configuration.md §`[agent.brain]`](configuration.md#agentbrain) |
| `[knowledge]` | **필수** | 이 블록이 없으면 `/v1/models`에 `penny`가 등록되지 않습니다. `wiki_path` 부모 디렉터리는 미리 존재해야 합니다 | [configuration.md §`[knowledge]`](configuration.md#knowledge) |
| `[knowledge.search]` | 선택 | 있을 때만 `web.search` MCP 도구가 노출됩니다 (SearXNG 라운드트립) | [configuration.md §`[knowledge.search]`](configuration.md#knowledgesearch) |
| `[knowledge.embedding]` + `[knowledge.reindex]` | 선택 | 있으면 pgvector 기반 시맨틱+키워드 하이브리드 검색, 없으면 keyword-only | [configuration.md §`[knowledge.embedding]`](configuration.md#knowledgeembedding-semantic-search-setup) |

---

## 트러블슈팅

> **연산자용 와이어 레벨 참조**: 아래 표는 증상→원인→대응 매핑입니다. 실제 HTTP 응답 바디의 정확한 JSON 형태(OpenAI-shaped envelope, `message` / `type` / `code` 필드, 각 코드별 HTTP status)는 [api-reference.md §Penny / Wiki error bodies](api-reference.md#penny--wiki-error-bodies-examples)에서 예시와 함께 확인할 수 있습니다. 클라이언트 SDK를 구현하거나 자동화된 에러 매칭을 작성한다면 그쪽을 먼저 보십시오.

| 증상 | 원인 | 대응 |
|---|---|---|
| `/v1/models`에 `penny`가 없음 | `[knowledge]` 섹션이 없거나 검증에 실패함 | `gadgetron.toml`에 `[knowledge]`를 추가하고, `wiki_path` 부모 디렉터리가 존재하는지 확인한 뒤 서버 로그에서 `penny: registered`를 확인 |
| `config file not found ... Create a gadgetron.toml with a [knowledge] section` | `gadgetron gadget serve`가 설정 파일을 찾지 못함 | `--config`로 올바른 경로를 넘기거나, 현재 디렉터리에 `gadgetron.toml`을 두십시오 |
| ``[knowledge] section is missing`` | 설정 파일은 있지만 knowledge layer가 비활성 | `[knowledge]` 블록을 추가하십시오 |
| `penny_not_installed` / spawn failure | `claude` 바이너리가 없거나 실행할 수 없음 | Claude Code를 설치하고 `claude login`을 완료한 뒤 `claude --version`을 확인 |
| `penny_timeout` | `request_timeout_secs` 초과 | 프롬프트를 단순화하거나 timeout을 늘리십시오 |
| `wiki_credential_blocked` | 위키 본문에 차단된 credential 패턴 포함 | 해당 비밀 문자열을 제거하십시오 |
| `wiki_page_too_large` | `wiki_max_page_bytes` 초과 | 페이지를 분리하거나 제한을 늘리십시오 |
| `wiki_conflict` | 위키 git 충돌 | 위키 저장소에서 `git status`와 수동 충돌 해결 후 재시도 |

추가 HTTP 에러 설명은 [troubleshooting.md](troubleshooting.md)를 참조하십시오.

---

## FAQ

**Q. Penny가 Anthropic API 과금을 직접 발생시키나요?**  
기본 `claude_max` 모드에서는 사용자의 Claude Code OAuth 세션을 사용합니다. Rust 코드는 subprocess와 MCP 도구를 관리합니다.

**Q. 다른 provider와 같이 쓸 수 있나요?**  
가능합니다. `penny`는 일반 `LlmProvider`처럼 `/v1/models`에 등록됩니다. 다만 Penny와 일반 모델을 같은 라우팅 풀에 무분별하게 섞는 구성은 피하는 편이 안전합니다.

**Q. `gadgetron init`만으로 Penny 설정이 끝나나요?**  
아직 아닙니다. `init`이 생성한 파일에 `[agent]` / `[knowledge]` 블록을 수동으로 추가해야 합니다.

---

## 연관 문서

| 문서 | 내용 |
|---|---|
| `docs/design/phase2/00-overview.md` | Phase 2 전체 설계 개요, STRIDE 위협 모델, GDPR/SOC2 매핑, 에러 테이블 |
| `docs/design/phase2/01-knowledge-layer.md` | `gadgetron-knowledge` crate 상세 (wiki/MCP/search) |
| `docs/design/phase2/02-penny-agent.md` | `gadgetron-penny` crate 상세 (provider/session/stream) |
| `docs/adr/ADR-P2A-01-allowed-tools-enforcement.md` | `--allowed-tools` 강제 검증 결과 (PASS on 2.1.104) |
| `docs/adr/ADR-P2A-02-dangerously-skip-permissions-risk-acceptance.md` | `--dangerously-skip-permissions` risk acceptance + non-root 전제 |
| `docs/adr/ADR-P2A-03-searxng-privacy-disclosure.md` | SearXNG 프라이버시 고지 ADR |
| `docs/manual/auth.md` | API 키 / 테넌트 / scope 시스템 |
| `docs/manual/quickstart.md` | 5분 로컬 빠른 시작 (pgvector + demo.sh 경로) |
| `docs/manual/troubleshooting.md` | 공통 에러 해결 · 로그 경로 · `gadgetron doctor` |
| `docs/manual/web.md` | Gadgetron Web UI (`/web`) 설정, Origin 격리, 키 회전, 헤드리스 빌드 — D-20260414-02 이후 신규 |
| `docs/design/phase2/03-gadgetron-web.md` | `gadgetron-web` crate 설계 (assistant-ui + Next.js embed) |
| `docs/adr/ADR-P2A-04-chat-ui-selection.md` | OpenWebUI → assistant-ui 전환 근거 |

---

## 변경 이력

- **2026-04-13 — v3 매뉴얼 초안**: Round 2 크로스리뷰 사이클 + ADR-P2A-01 M4 behavioral verification (PASS on claude 2.1.104) + 프라이버시 고지 2종 + 트러블슈팅 표. 구현 PR 머지 전 pre-merge gate 요건 충족.
- **2026-04-14 — D-20260414-02 / ADR-P2A-04**: OpenWebUI sibling process 제거, `gadgetron-web` (assistant-ui + Next.js embed) 으로 전환. §2 Docker 전제 조건 갱신 (Web UI 는 Docker 불필요), §4 단일 바이너리 서버 기동, §5 `gadgetron-web` 기반 첫 대화 단계로 전면 재작성. `--docker` 플래그 P2A 미지원 처리 (graceful deprecation). `web.md` 및 `03-gadgetron-web.md` 연관 문서 추가.
