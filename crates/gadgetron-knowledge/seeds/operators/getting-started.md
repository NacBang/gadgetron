---
tags = ["operators", "getting-started", "installation"]
type = "runbook"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# 운영자 온보딩

Gadgetron을 처음 띄우는 운영자를 위한 문서입니다. 5단계 안에 API 응답을 볼 수 있도록 구성했습니다.

## 사전 요구사항

| 항목 | 설치 확인 |
|---|---|
| Rust toolchain (1.94+) | `cargo --version` |
| Claude Code CLI (2.1.104+) | `claude --version` |
| `claude login` 완료 | `~/.claude/` 디렉토리 존재 |
| Git | `git --version` |
| Node.js + npm (Web UI 빌드에 필요, 선택) | `node --version` |
| PostgreSQL 15+ with pgvector (ADR-P2A-07 이후) | `psql -c 'SELECT 1'` |

## 설치

```sh
git clone https://github.com/NacBang/gadgetron.git
cd gadgetron
cargo build --release -p gadgetron-cli
```

바이너리는 `./target/release/gadgetron`에 생성됩니다.

## 설정

```sh
cp gadgetron.example.toml gadgetron.toml
$EDITOR gadgetron.toml
```

최소 구성 (Penny만 쓰고 LLM provider는 나중):

```toml
[server]
bind = "0.0.0.0:8080"

[agent]
binary = "claude"   # 절대 경로 필요 시 여기서 override
request_timeout_secs = 300
max_concurrent_subprocesses = 4

[agent.brain]
mode = "claude_max"

[knowledge]
wiki_path = "./.gadgetron/wiki"
wiki_autocommit = true
wiki_max_page_bytes = 1_048_576

[web]
enabled = true
```

### Claude binary 절대 경로

spawn에 `env_clear()` 적용 후 고정 PATH(`/usr/local/bin:/usr/bin:/bin`)가 되므로, `claude` 바이너리가 이 경로에 없으면 `agent.binary`에 절대 경로를 주세요:

```toml
[agent]
binary = "/Users/<name>/.local/bin/claude"
```

## 첫 키 발급

```sh
./target/release/gadgetron key create
```

출력:

```
API Key Created
  Key:    gad_live_...
  Save this key — it cannot be retrieved later.
```

복사해두세요. 웹 UI에서 붙여넣습니다.

## 서버 시작

```sh
./target/release/gadgetron serve --config gadgetron.toml --no-db
```

로그에서 다음 확인:

```
provider registered name=<provider>
penny: registered (KnowledgeToolProvider active...)
listening addr=0.0.0.0:8080
```

## 접속

```sh
curl http://localhost:8080/health
# {"status":"ok"}

curl http://localhost:8080/v1/models -H "Authorization: Bearer <키>"
# penny + 등록한 provider들이 보여야 함
```

브라우저에서 `http://localhost:8080/web` 접속 → API 키 붙여넣기 → 대화 시작.

## 문제가 생기면

[`operators/troubleshooting.md`](./troubleshooting.md) 참조. 주요 에러 코드:

- `penny_not_installed` → `agent.binary` 경로 확인
- `penny_agent_error` + `exit_code=1` + stderr "stream-json requires --verbose" → Claude Code 버전이 너무 낮음, 2.1.104+ 업그레이드
- `provider_error` (model=penny 요청인데 vLLM으로 갔음) → Router direct-match가 동작 안 함, v0.3.0-demo 이후 버전 필요
- `401 Unauthorized` → Bearer 키 틀림. `key create` 다시 실행

## 다음 단계

- [`penny/usage.md`](../penny/usage.md) — Penny 사용법
- [`penny/conventions.md`](../penny/conventions.md) — 위키 작명 규칙
- [`operators/troubleshooting.md`](./troubleshooting.md) — 운영 트러블슈팅
