---
tags = ["operators", "troubleshooting", "runbook"]
type = "runbook"
source = "seed"
plugin = "gadgetron-core"
plugin_version = "0.3.0"
---

# 운영 트러블슈팅

자주 발생하는 에러와 대응 절차입니다. 새 이슈를 마주치면 여기에 추가하세요.

## HTTP 에러

### `401 Unauthorized`

**증상**: 모든 요청이 401. `/health`는 200.
**원인**: Bearer 키 틀림 또는 누락.
**대응**:
1. `curl -H "Authorization: Bearer <키>" /v1/models` 로 키 검증
2. 틀리면 `gadgetron key create`로 재발급 후 재시도
3. no-db 모드에서는 `gad_live_*` 형식만 확인, 검증은 느슨함

### `404 The model "penny" does not exist`

**증상**: 서버 로그에 `vLLM stream error 404: The model 'penny' does not exist.`
**원인**: Router가 `penny` 요청을 vLLM provider로 라우팅함 (direct-match 없음).
**대응**: v0.3.0-demo 이후 버전으로 업그레이드. Router의 direct-match (PR #31) 포함되어 있어야 함.

### `422 Unprocessable Entity`

**증상**: 웹 UI에서 메시지 전송 시 422.
**원인**: AI SDK v6 wire format이 OpenAI와 다름.
**대응**: v0.3.0-demo 이후 버전. `OpenAIChatTransport` (app/openai-transport.ts) 포함되어 있어야 함.

### `413 Payload Too Large`

**증상**: 큰 요청 거부.
**원인**: 요청이 4 MiB 초과 (SEC-M2).
**대응**: 대화 히스토리가 너무 길면 새 conversation_id로 시작. 파일 업로드는 P2B.

## Penny 에러

### `penny_not_installed`

**증상**: "The Claude Code CLI (`claude`) was not found on the server."
**원인**: `agent.binary`가 spawn의 고정 PATH에서 찾아지지 않음.
**대응**: `gadgetron.toml`에서 절대 경로로:

```toml
[agent]
binary = "/Users/<name>/.local/bin/claude"
```

### `penny_agent_error` + exit_code=1

**서브케이스 1**: stderr에 `--output-format=stream-json requires --verbose`
- **원인**: Claude Code 2.0.x (구버전). P2A는 2.1.104+ 전제.
- **대응**: `claude --version` 확인, 필요 시 업그레이드.

**서브케이스 2**: stderr에 `Not logged in · Please run /login`
- **원인**: spawn env allowlist에 `USER`/`SHELL`이 없어서 macOS 키체인 접근 실패. v0.3.0-demo 이전 버전.
- **대응**: 업그레이드.

**서브케이스 3**: stderr 비어있음 + exit_code=1
- **원인**: 원인 불명. stderr가 redact된 뒤 빈 문자열로 남을 수도.
- **대응**: `RUST_LOG=debug`로 서버 재시작 후 재현. raw stderr는 서버 로그에 기록됨.

### `penny_timeout`

**증상**: 504 timeout after 300s.
**원인**: Claude Code subprocess가 제한시간 초과.
**대응**: `gadgetron.toml`의 `[agent].request_timeout_secs` 상향. 단, 길수록 subprocess 점유 증가 → `max_concurrent_subprocesses` 체크.

## 위키 에러

### `wiki_conflict`

**증상**: "A wiki page could not be saved because it was modified by another process"
**원인**: git index 충돌. 보통 외부에서 수동 수정 + Penny 동시 쓰기.
**대응**:
```sh
cd <wiki_path>
git status
git diff
# 충돌 해결 후
git add . && git commit -m "manual resolve"
```

### `wiki_page_too_large`

**증상**: 413. "exceeds the maximum size (...)"
**원인**: `wiki_max_page_bytes` (기본 1 MiB) 초과.
**대응**: 페이지를 여러 개로 분할하거나 `gadgetron.toml`에서 상향.

### `wiki_invalid_path`

**증상**: "The requested wiki page path is invalid"
**원인**: 경로에 `..`, 절대 경로, 특수문자 포함.
**대응**: [`penny/conventions.md`](../penny/conventions.md) 이름 규칙 준수.

### `wiki_credential_blocked`

**증상**: 422. "blocked because it contains a credential pattern"
**원인**: M5 탐지 — PEM/AWS/GCP 키 패턴이 페이지 내용에 포함됨.
**대응**: 크레덴셜 제거 후 재저장. 의도적으로 예시를 넣어야 한다면 fake 값으로 대체.

## 서버 시작 실패

### `Address already in use`

**증상**: `Error: failed to bind to 0.0.0.0:8080`
**대응**:
```sh
lsof -ti :8080 | xargs kill   # 점유 프로세스 종료
# 또는 gadgetron.toml의 [server].bind 포트 변경
```

### Penny 등록 안 됨

**증상**: `/v1/models`에 `penny`가 없음.
**원인**: `[agent]` 또는 `[knowledge]` 섹션이 config에 없거나 잘못됨.
**대응**: `gadgetron.example.toml` 대비해서 섹션 존재 확인.

## 관측 팁

```sh
# 디버그 로그로 서버 시작
RUST_LOG=debug,gadgetron_penny=trace ./target/release/gadgetron serve --config gadgetron.toml --no-db 2>&1 | tee /tmp/gadgetron.log

# 위키 변경 이력
(cd <wiki_path> && git log --oneline -20)

# 활성 subprocess 확인
ps aux | grep claude
```

## 추가 기여

이 페이지에 없는 이슈를 마주쳤다면:

1. 해결 후 "이 이슈 트러블슈팅에 추가해줘" Penny에게 요청
2. Penny가 템플릿에 맞춰 `operators/troubleshooting.md` 또는 별도 `operators/<문제>-<대응>.md`로 저장

## 관련

- [`operators/getting-started.md`](./getting-started.md) — 초기 설정
- [`penny/usage.md`](../penny/usage.md) — Penny 자체
