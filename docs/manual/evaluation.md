# Penny 평가 하네스 (eval)

운영자가 개입하지 않고도 Penny의 주요 경로를 자동 검증할 수 있는 시나리오
기반 평가 도구입니다. 저장소 루트의 [`eval/`](../../eval/) 디렉터리에 삽니다.

Round 2 (2026-04) 기준 8개 시나리오를 추적합니다:

| 시나리오 | 검증 |
|---|---|
| `wiki-roundtrip` | `wiki.write` → `wiki.get` 라운드트립 (디스크 내용까지 검증) |
| `slash-wiki-list` | `/wiki list` 슬래시 커맨드 |
| `wiki-search-finds-seed` | `wiki.search` 키워드 히트 |
| `wiki-rename-moves-page` | `wiki.rename` (원본 사라짐 + 새 경로 존재) |
| `wiki-delete-archives-page` | `wiki.delete` 소프트 아카이브 |
| `web-search-direct-query` | `web.search` MCP 도구 직접 호출 (SearXNG 라운드트립) |
| `manycoresoft-explain` | `web.search` fallback으로 위키 외 주제 설명 |
| `manycoresoft-research-and-save` | `web.search` → `wiki.write` 엔드투엔드 조사+저장 |

---

## 전제 조건

1. `gadgetron serve --no-db --config gadgetron.toml` 이 로컬 `127.0.0.1:8080`에서 실행 중이어야 합니다.
2. `gadgetron.toml` 에 `[agent]` + `[knowledge]` + `[knowledge.search]`가
   모두 구성되어 있어야 합니다. `wiki_path`는 **절대 경로**여야 합니다
   (자세한 이유는 [`configuration.md`](configuration.md) `[knowledge]` 절 참고).
3. `manycoresoft-research-and-save` 시나리오는 로컬 SearXNG 인스턴스에
   의존합니다. 가장 빠른 방법은 Docker:

   ```sh
   mkdir -p ~/gadgetron-searxng
   cat > ~/gadgetron-searxng/settings.yml <<'YAML'
   use_default_settings: true
   server:
     secret_key: "change-me"
     limiter: false
   search:
     formats: [html, json]
     safe_search: 0
   general:
     instance_name: gadgetron-eval
   YAML

   docker run -d --name gadgetron-searxng \
     -p 127.0.0.1:8888:8080 \
     -v ~/gadgetron-searxng/settings.yml:/etc/searxng/settings.yml:ro \
     --restart unless-stopped \
     searxng/searxng:latest
   ```

4. `GADGETRON_API_KEY` 환경변수에 `gadgetron key create`로 발급한 로컬 키가 설정되어야 합니다.
5. Python 3.10+ 와 `requests`, `pyyaml` 패키지가 필요합니다.

---

## 실행

```sh
export GADGETRON_API_KEY=gad_live_...
python3 eval/run_eval.py                          # 전체 시나리오
python3 eval/run_eval.py --scenario wiki-roundtrip  # 단일 시나리오
python3 eval/run_eval.py --server http://other-host:8080
python3 eval/run_eval.py --no-report              # 리포트 파일 생략
```

- 리포트는 `eval/reports/<timestamp>.md` 에 쌓이며 gitignore 됩니다.
- Exit code: regression이 없으면 `0`, `fail` 혹은 `unexpected_pass`가 하나라도 있으면 `1`.
  그대로 CI 게이트로 묶을 수 있습니다.

---

## 결과 코드

- `pass` — 통과해야 할 시나리오가 통과.
- `fail` — 통과해야 할 시나리오가 실패 (triage 필요).
- `expected_fail` (XFAIL) — `expected_status: failing` 으로 silenced 된 실패.
- `unexpected_pass` (XPASS) — silenced 된 시나리오가 되돌아와서 통과. 라벨을 지워야 함.

---

## 새 시나리오 추가

`eval/scenarios.yaml` 에 YAML 블록을 하나 더 적습니다. 지원되는 필드:

- `id`, `description`, `prompt` (단일 user 메시지)
- `timeout_s` (스트림 wall-clock 제한)
- `expected_status: failing` — 알려진 gap 을 silence (unexpected_pass 시 경고).
- `expect`:
  - `finish_reason` — 마지막 chunk의 `finish_reason` 값.
  - `tool_calls_contain` — 나열된 MCP 툴 이름이 모두 스트림에 등장해야 함.
  - `text_contains_any` — 어시스턴트 텍스트에 **한 개라도** 포함되면 통과.
  - `page_path` — `wiki_path` 기준 상대 경로의 md 파일이 생성되어야 함.
  - `page_contains` / `page_contains_any` — 페이지 본문 부분 문자열 검증.
  - `page_path_absent` — `wiki_path` 기준 상대 경로의 md 파일이 **없어야** 통과 (delete / rename 검증용).

예시:

```yaml
- id: my-new-scenario
  description: Something Penny must do.
  timeout_s: 60
  prompt: |
    wiki.write 로 예시 페이지를 만들어줘.
  expect:
    tool_calls_contain:
      - mcp__knowledge__wiki_write
    page_path: example.md
    finish_reason: stop
```

---

## 알려진 제약

- 시나리오 순서는 선언 순서대로 실행됩니다. `rename` / `delete` 시나리오는
  직전의 `roundtrip` 시나리오가 만든 페이지에 의존하므로 순서를 바꾸면
  실패합니다.
- `--scenario <id>` 로 단일 실행할 때는 선행 시나리오가 남긴 상태가
  전제됩니다 (디스크에 파일이 있어야 rename / delete가 성공). 단일 실행이
  실패하면 `wiki-roundtrip` 부터 다시 돌리십시오.
- 평가는 실제 Claude Code 서브프로세스를 띄우므로 Penny 전체 파이프라인
  (MCP stdio, SearXNG 라운드트립 포함)의 진짜 latency를 측정합니다.
  8 시나리오 전체가 로컬 맥북에서 1분대에 끝나는 것이 일반적입니다 (정확한 wall-clock은 네트워크·SearXNG 응답 시간에 따라 변동).

---

## CI 연동

`run_eval.py` 는 regression 발생 시 exit code `1` 로 종료하므로 CI 게이트에 그대로 꽂을 수 있습니다. 아래 recipes 는 자주 쓰는 CI 플랫폼별 최소 구성입니다 — 각자의 secret 저장소 + runner 환경에 맞춰 조정하십시오.

### GitHub Actions

```yaml
# .github/workflows/penny-eval.yml
name: Penny Eval

on:
  pull_request:
    paths:
      - 'crates/gadgetron-penny/**'
      - 'crates/gadgetron-knowledge/**'
      - 'eval/**'
  workflow_dispatch:

jobs:
  eval:
    runs-on: ubuntu-22.04
    timeout-minutes: 15
    services:
      searxng:
        image: searxng/searxng:latest
        ports: ['8888:8080']
        env:
          # minimal settings baked into the public image; adequate
          # for the `web.search` scenarios.
          INSTANCE_NAME: gadgetron-ci
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with: { toolchain: '1.80' }

      - name: Build gadgetron
        run: ./demo.sh build

      - name: Install Claude Code
        run: |
          curl -fsSL https://claude.ai/install.sh | sh
          claude --version

      - name: Provision Claude Code credentials
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
        run: |
          # CI-safe: switch [agent.brain] to external_anthropic so the
          # harness doesn't need interactive `claude login`. This trades
          # the default `claude_max` OAuth session (per-developer,
          # interactive) for a flat API key that the secret manager
          # supplies.
          cat > gadgetron.toml <<'TOML'
          [server]
          bind = "127.0.0.1:8080"
          [agent]
          binary = "claude"
          request_timeout_secs = 300
          [agent.brain]
          mode = "external_anthropic"
          [knowledge]
          wiki_path = "./.gadgetron/wiki"
          wiki_autocommit = true
          [knowledge.search]
          searxng_url = "http://127.0.0.1:8888"
          timeout_secs = 10
          TOML
          mkdir -p .gadgetron

      - name: Start gadgetron
        run: |
          ./target/release/gadgetron serve --no-db \
            --config gadgetron.toml > /tmp/gadgetron.log 2>&1 &
          echo $! > /tmp/gadgetron.pid
          for i in $(seq 1 30); do
            curl -fsS http://127.0.0.1:8080/health && break
            sleep 1
          done

      - name: Mint eval key
        run: |
          KEY=$(./target/release/gadgetron key create | grep -oE 'gad_live_[a-f0-9]+')
          echo "GADGETRON_API_KEY=$KEY" >> $GITHUB_ENV

      - name: Run eval
        run: python3 eval/run_eval.py

      - name: Capture logs on failure
        if: failure()
        uses: actions/upload-artifact@v4
        with:
          name: gadgetron-logs
          path: |
            /tmp/gadgetron.log
            eval/reports/*.md

      - name: Stop gadgetron
        if: always()
        run: kill $(cat /tmp/gadgetron.pid) 2>/dev/null || true
```

**키 지점**:
- `timeout-minutes: 15` — 로컬 1분대 대비 cold-start + CI runner 지연 여유. Claude Code 가 API 키 기반으로 요청을 보내기 시작하기까지 ~30초 관찰됨.
- `external_anthropic` brain mode 로 전환 — CI runner 에는 `claude login` interactive OAuth 가 없으므로 `ANTHROPIC_API_KEY` 를 직접 주입하는 경로가 필요.
- `--no-db` 는 eval 이 `[knowledge]` 만 요구하고 quota/audit 영속성은 요구하지 않기 때문. Production smoke 는 [installation.md §8 Post-deploy acceptance smoke test](installation.md) 이 담당.
- `paths:` 필터로 PR 이 Penny/knowledge/eval 쪽을 건드릴 때만 실행 — 워커 분 절약.
- 실패 시 `gadgetron-logs` 아티팩트로 `/tmp/gadgetron.log` + `eval/reports/*.md` 를 업로드해서 로컬 재현 없이 triage.

### GitLab CI

```yaml
# .gitlab-ci.yml
stages:
  - eval

penny-eval:
  stage: eval
  image: rust:1.80
  timeout: 15 minutes
  services:
    - name: searxng/searxng:latest
      alias: searxng
  variables:
    SEARXNG_URL: 'http://searxng:8080'
  script:
    - ./demo.sh build
    - curl -fsSL https://claude.ai/install.sh | sh
    - |
      cat > gadgetron.toml <<TOML
      [server]
      bind = "127.0.0.1:8080"
      [agent]
      binary = "claude"
      [agent.brain]
      mode = "external_anthropic"
      [knowledge]
      wiki_path = "./.gadgetron/wiki"
      wiki_autocommit = true
      [knowledge.search]
      searxng_url = "$SEARXNG_URL"
      TOML
      mkdir -p .gadgetron
    - ./target/release/gadgetron serve --no-db --config gadgetron.toml > /tmp/gadgetron.log 2>&1 &
    - until curl -fsS http://127.0.0.1:8080/health; do sleep 1; done
    - export GADGETRON_API_KEY=$(./target/release/gadgetron key create | grep -oE 'gad_live_[a-f0-9]+')
    - python3 eval/run_eval.py
  artifacts:
    when: on_failure
    paths:
      - /tmp/gadgetron.log
      - eval/reports/
  only:
    changes:
      - 'crates/gadgetron-penny/**/*'
      - 'crates/gadgetron-knowledge/**/*'
      - 'eval/**/*'
```

### Providing `ANTHROPIC_API_KEY` from secrets

- **GitHub Actions**: Repository → Settings → Secrets and variables → Actions → add `ANTHROPIC_API_KEY`. Referenced as `${{ secrets.ANTHROPIC_API_KEY }}` above.
- **GitLab CI**: Settings → CI/CD → Variables → `ANTHROPIC_API_KEY` (masked, protected if using protected branches).
- **Self-hosted runners**: hand the key via the runner's environment; treat as any other service-account credential. Rotate per `manual/auth.md §Secret + key rotation cadence`.

### Debugging a failing eval run

When the CI run is red, reproduce locally first:

```sh
# 1. Start the same services (pgvector optional for --no-db eval):
docker run -d --name searxng -p 127.0.0.1:8888:8080 searxng/searxng:latest

# 2. Match the CI config (external_anthropic, not claude_max):
export ANTHROPIC_API_KEY="sk-ant-..."
# edit gadgetron.toml to [agent.brain] mode = "external_anthropic"

# 3. Start gadgetron with verbose Penny logging:
RUST_LOG=info,penny_subprocess=debug,penny_stream=debug,wiki_search=debug \
  ./target/release/gadgetron serve --no-db --config gadgetron.toml

# 4. Re-run the specific scenario that failed:
python3 eval/run_eval.py --scenario <failing-scenario-id>
```

See [troubleshooting.md §`RUST_LOG` cheat sheet](troubleshooting.md) for the full target catalog and production-safe filter combinations. The `eval/reports/<timestamp>.md` report that `run_eval.py` emits includes per-scenario captured output — grep for `fail` in the CI artifact to get the same context without re-running.

### Keeping eval healthy

- **Scenarios drift when the wiki seed changes.** The harness expects specific seed pages (see `scripts/e2e-harness/fixtures/`). When those fixtures change, `wiki-search-finds-seed` or similar keyword scenarios may shift `expected_status`. Re-baseline by running locally, inspect the new text-contains-any hits, and update `eval/scenarios.yaml`.
- **`expected_status: failing`** is a short-lived label. Review monthly — unexpected passes (`xpass`) that sit there for weeks indicate a fixed bug that no one removed the silence marker for.
- **CI cost control**: each eval run burns roughly `8 × (2–4K input + 300 output)` tokens against the Anthropic API with `external_anthropic`. Approximate ~$0.15–$0.35 per run at current pricing. Scope the trigger (`paths:` filter or manual dispatch) if the eval runs on every commit.
