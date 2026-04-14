# Gadgetron Web UI

> **상태**: Phase 2A 구현 예정. 설계 문서는 `docs/design/phase2/03-gadgetron-web.md`.
> **대상 독자**: Gadgetron 운영자 / 단일 유저 로컬 사용자.
> **관련 결정**: D-20260414-02 (OpenWebUI → assistant-ui), ADR-P2A-04.

Gadgetron Web UI (`gadgetron-web`) 는 Gadgetron 바이너리에 embed 된 채팅 UI 입니다. `gadgetron serve` 한 번이면 `http://localhost:8080/web` 에서 바로 열리며, 별도 Docker 컨테이너·sibling 프로세스·외부 DB 가 필요 없습니다. 스택: [assistant-ui](https://github.com/assistant-ui/assistant-ui) (MIT) + Next.js 14 + Tailwind + shadcn/ui.

---

## 전제 조건

1. **Gadgetron 바이너리가 `--features web-ui` 로 빌드되어 있어야 합니다.** 기본 빌드는 feature 가 켜져 있으므로 `cargo build --release` 로 빌드하면 됩니다. 빌드 시 Node.js 20.19.0 이 PATH 에 있어야 합니다 — `crates/gadgetron-web/web/.nvmrc` 참조.
2. **Gadgetron 서버가 기동 중이어야 합니다.** `gadgetron serve --config ~/.gadgetron/gadgetron.toml`
3. **Gadgetron API 키가 있어야 합니다.** `gadgetron key create --tenant-id default` 로 생성.

---

## Origin 격리 요구사항 (중요)

> **Gadgetron 은 다른 웹 앱과 origin (scheme + host + port) 을 공유해서는 안 됩니다.**

브라우저의 `localStorage` 는 path 가 아닌 **origin** 기준으로 격리됩니다. 예를 들어 `https://internal.company.com:8080/gadgetron/` 와 `https://internal.company.com:8080/jenkins/` 가 같은 서버에 있다면, Jenkins 에서 XSS 가 발생하면 Gadgetron API 키가 탈취됩니다.

**권장 배포**:
- 단일 유저 로컬: `http://localhost:8080/web` (기본) — 같은 머신에 다른 웹 앱이 없다면 안전
- 팀/on-prem: 전용 서브도메인 (`gadgetron.example.com`) 또는 전용 포트
- SaaS/클라우드: 전용 origin 필수

---

## 빠른 시작

### 1. Gadgetron 기동

```sh
./target/release/gadgetron serve --config ~/.gadgetron/gadgetron.toml
```

단일 프로세스로 `:8080/v1/*` (OpenAI-compat API) 와 `:8080/web/*` (Web UI) 를 모두 서빙합니다.

### 2. API 키 생성

```sh
./target/release/gadgetron key create --tenant-id default
```

출력된 `gad_live_...` 키를 복사하십시오.

### 3. 브라우저 열기

`http://localhost:8080/web` 접속. 첫 방문이면 `/settings` 로 자동 리다이렉트되어 "API 키를 입력하세요" 배너가 표시됩니다.

### 4. 키 붙여넣기 + 저장

설정 페이지에서 API 키 입력란에 `gad_live_...` 붙여넣기 → Save. 키는 브라우저 localStorage 에만 저장되며, 같은 origin (`:8080`) 위에서만 `/v1/*` 호출에 사용됩니다.

### 5. 모델 선택 후 대화 시작

- 드롭다운에서 `kairos` (또는 원하는 프로바이더 모델) 선택
- 메시지 입력 → Enter
- 스트리밍 응답이 2초 이내에 시작됩니다

---

## 수동 QA 체크리스트

P2A 구현 완료 후 아래 항목을 수동으로 확인합니다 (E2E 자동화는 `tests/e2e/web_smoke.sh` 가 커버).

- [ ] `./target/release/gadgetron serve --config ~/.gadgetron/gadgetron.toml` → `http://localhost:8080/web` 접속 → 채팅 UI 가 로드됨
- [ ] Settings 페이지 → 유효한 `gad_live_*` 키 붙여넣기 → "Saved" 확인
- [ ] 잘못된 형식 키 붙여넣기 → "Invalid format. Keys start with gad_live_ or gad_test_." 인라인 에러
- [ ] 채팅으로 돌아가 모델 드롭다운에서 `/v1/models` 결과 확인 (`kairos` 포함)
- [ ] `kairos` 선택 → "ping" 전송 → 스트리밍 토큰 응답 확인
- [ ] 메시지 본문에 `<script>alert(1)</script>` 포함 → alert 미발생, 텍스트로만 렌더됨 (XSS 방어 M-W1 확인)
- [ ] 네트워크 차단 → 오프라인 배너 확인
- [ ] Gadgetron Ctrl-C → 재시작 → 자동 재연결
- [ ] `curl -I http://localhost:8080/web/` → `Content-Security-Policy` 헤더에 `require-trusted-types-for 'script'` 포함 확인
- [ ] `curl -I http://localhost:8080/v1/models -H "Authorization: Bearer $KEY"` → CSP 헤더 **없음** 확인 (API 응답은 CSP 미적용)

---

## 헤드리스 빌드 (Web UI 제외)

Web UI 가 필요 없는 배포 (API-only 서버) 는 헤드리스 빌드를 사용합니다:

```sh
cargo build --release --no-default-features --features headless -p gadgetron-cli
```

자세한 내용은 `docs/manual/installation.md` §Headless build 참조.

검증:

```sh
./target/release/gadgetron serve &
curl -I http://localhost:8080/web/  # 404 Not Found 여야 함
```

---

## 긴급 복구 (키 노출 의심 시)

API 키가 노출되었다고 의심되면 (XSS 사건, 로컬 XSS, 노트북 분실 등):

1. 키 회전:
   ```sh
   gadgetron key create --rotate <old_key_id>
   ```
   이전 키는 Phase 1 `PgKeyValidator` LRU 무효화 경로 (D-20260411-12) 로 < 1초 안에 무효화됩니다.

2. 브라우저 Settings → "Clear" 버튼으로 localStorage 초기화

3. 새 키 붙여넣기

회전 이전의 감사 로그 엔트리는 유효하게 유지됩니다 (`request_id` 상관관계 보존).

---

## 트러블슈팅

### `/web` 404

- `cargo build --features web-ui` 로 빌드했는지 확인. `--no-default-features --features headless` 로 빌드했다면 Web UI 가 컴파일되지 않은 상태입니다.
- `gadgetron.toml` 의 `[web].enabled = true` 인지 확인 (기본값 true)

### 모델 드롭다운이 비어 있음

- `curl -sf -H "Authorization: Bearer $KEY" http://localhost:8080/v1/models | jq '.data'` 로 직접 확인
- 응답이 비어 있으면 `gadgetron.toml` 에 프로바이더 블록이 없음. `[kairos]` 또는 `[providers.*]` 추가.
- 401 이면 키가 잘못됨 — `/settings` 로 돌아가 재입력

### "Gadgetron Web UI not built" 배너 표시

빌드 시 Node.js 가 PATH 에 없어서 fallback UI 가 embed 된 상태입니다. Node 20.19.0 설치 후 `cargo clean -p gadgetron-web && cargo build --features web-ui` 재빌드.

### API 키 잘못됨 (401)

- Settings 페이지의 red 배너에 "Your API key was rejected (401). Please enter a new one." 표시
- 새 키 입력 후 저장. 기존 localStorage 다른 항목 (theme, default model) 은 유지됨 (DX-W-NB1)

### CSP 위반 (개발자 콘솔에 CSP 에러)

- DOMPurify 가 어떤 패턴을 차단했거나, 외부 리소스 (Google Fonts 등) 로드 시도가 있었다는 뜻
- `docs/design/phase2/03-gadgetron-web.md §16` DOMPurify 설정 참조
- 브라우저 콘솔 메시지를 security-compliance-lead 에게 보고

---

## 연관 문서

- `docs/manual/kairos.md` — Kairos 개인 비서 설정·사용
- `docs/manual/installation.md` — Gadgetron 전체 설치 (헤드리스 빌드 포함)
- `docs/manual/auth.md` — API 키 생성·관리
- `docs/manual/configuration.md` — `gadgetron.toml` 전체 필드
- `docs/manual/troubleshooting.md` — 일반 트러블슈팅
- `docs/design/phase2/03-gadgetron-web.md` — 설계 상세
- `docs/adr/ADR-P2A-04-chat-ui-selection.md` — 결정 근거

---

## 변경 이력

- **2026-04-14** (D-20260414-02 + ADR-P2A-04): 신규 문서. OpenWebUI sibling process → `gadgetron-web` embed 로 전환. Origin 격리 요구사항·키 회전·XSS 방어 M-W1~M-W7 명시.
