# 16 — Server Metrics Timeseries

> **담당**: PM (Codex)
> **상태**: Reviewed — implementation pending (SRE / Security / Platform / Penny 4-perspective review 2026-04-21 반영)
> **작성일**: 2026-04-21
> **최종 업데이트**: 2026-04-21 (§2.1 tenant_id denormalize + labels allowlist CHECK, §2.2.1 retention tier 재조정, §2.4 응답 `dropped_frames` + `refresh_lag_seconds`, §4 전반 role 분리 + legal hold)
> **관련 크레이트**: `gadgetron-core`, `gadgetron-gateway`, `plugins/plugin-server-monitor`, `gadgetron-web`, new `gadgetron-metrics` (optional split)
> **Phase**: [P2B] primary / [P2C] aggregated query API surface
> **관련 문서**: `docs/design/phase2/07-bundle-server.md`, `docs/design/phase2/12-external-gadget-runtime.md`, `docs/design/phase2/13-penny-shared-surface-loop.md`, `docs/adr/ADR-P2A-05-agent-centric-control-plane.md`, `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`, `docs/process/04-decision-log.md`

---

## 1. 철학 & 컨셉 (Why)

### 1.1 이 문서가 닫는 공백

`plugins/plugin-server-monitor` v0.1 (#310 ~ #314) 가 `server.stats` 1 Hz 폴링으로 **현재 순간 스냅샷**을 수집·표시한다. 그러나:

1. **세션 바깥으로 데이터가 나가지 않는다** — 브라우저를 닫으면 히스토리가 사라진다. incident post-mortem ("3시에 CPU가 왜 튀었지?") 에 쓸 수 없다.
2. **여러 UI/Penny/알림이 같은 히스토리를 공유할 수 없다** — 각 클라이언트가 독립 리플레이를 한다.
3. **gadgetron 재시작 시 데이터 손실** — 단기 관측조차 신뢰 불가.
4. **Penny가 시간 추이를 질의할 수 없다** — "지난 30분간 GPU util 추세" 같은 자연어 질문을 LLM이 답하려면 retrospective store가 필요하다.

이 문서는 `host_metrics` timeseries 테이블 + 자동 downsampling + 읽기 API 를 도입해 위 네 공백을 한 번에 닫는다. **Push 아키텍처 (agent on target) 도입 이전에도**, 현재 pull-path 가 수집한 데이터를 Postgres 로 쏟아 넣는 것만으로도 세션-독립 히스토리가 생긴다. 나중에 push agent 가 들어오면 같은 테이블·같은 쿼리 표면을 재사용한다.

### 1.2 제품 비전과의 연결

- `docs/00-overview.md §1` — "공유 knowledge layer": 호스트 상태 시계열은 **지식의 한 축**이다. 현재의 wiki + 대화 auditing 과 함께 "지난주 incident 맥락" 을 Penny 가 조회할 수 있어야 한다.
- `07-bundle-server.md §1.2` — Server Bundle 의 evidence 원칙과 정렬: append-only, structured, operator 가 후회 없이 근거를 인용할 수 있음.
- `13-penny-shared-surface-loop.md §2.3` — Penny shared-context bootstrap 디지스트에 "최근 1시간 이벤트" 가 포함된다. 시계열 읽기 API 가 이 bootstrap 의 데이터 소스가 된다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 장점 | 채택하지 않은 이유 |
|---|---|---|
| A. 클라이언트 메모리 ring buffer | 의존성 추가 0, 즉시 구현 | 세션·재시작·멀티-클라이언트 3축 모두 fail. Penny 는 이 데이터를 못 본다. |
| B. gadgetron 프로세스 내 ring buffer | state sharing 부분 해결 | 재시작 시 loss. Postgres 이미 있는데 별도 저장 계층 추가 정당성 부족. |
| C. Prometheus / VictoriaMetrics 외부 TSDB | 업계 표준, 성숙도 극단적 | 별도 배포 단위, credential 분리, gadgetron audit 과 분리된 id 공간 — single-pane-of-glass 깨짐. |
| D. Postgres + **TimescaleDB** hypertable (채택) | 기존 Postgres 재사용, continuous aggregates, gadgetron audit / billing 과 같은 credential 공간 | 이미지 교체 1회. pgvector + timescaledb 공존 이미지 검증 필요 (해결 가능). |
| E. Postgres 순정 partitioning (PARTITION BY RANGE) | extension 불필요 | continuous aggregate 직접 materialized view 관리, retention 스크립트 직접 운영. Timescale 대비 공수 2-3배. |

**채택: D — TimescaleDB + pgvector 공존 이미지.**

### 1.4 핵심 설계 원칙과 trade-off

1. **Postgres 는 audit 의 연장선**. 메트릭이 별도 store 로 갈라지는 순간 credential / backup / migrations / recovery 트랙이 두 벌이 된다. 이것을 거부한다.
2. **Narrow row 스키마**. host 별 metric 수는 하드웨어마다 다르다 (GPU 0~8, NIC 1~n). `ALTER TABLE` 없이 신규 metric 을 추가할 수 있어야 한다.
3. **Ingestion 은 hot path 를 블록하지 않는다**. `server.stats` 응답 지연이 DB INSERT 와 커플링되면 폴링이 무너진다. bounded mpsc + bg worker + drop-on-full + counter.
4. **자동 downsampling 이 기본**. 운영자가 수동으로 "1시간 은 5초 해상도" 같은 질의를 쓰지 않아도 UI 가 window 로 tier 를 선택한다.
5. **Retention policy 는 explicit**. tier 별 보존 기간이 config 에 선언돼 있고, 감춰진 추측 없음.
6. **Schema 는 Penny-readable**. metric name 이 계층형 (`cpu.util`, `gpu.0.temp`, `nic.eth0.rx_bps`) 라 LLM 이 직관적으로 쓸 수 있다.
7. **쓰는 쪽도 Penny**. 미래의 agent push 경로가 같은 스키마를 쓴다. pull/push 간 wire shape 차이를 두지 않는다.

Trade-off:
- TimescaleDB extension 의존. pgvector-only 이미지를 쓰는 외부 contributor 는 세팅이 다르다 → custom Docker 이미지를 repo 에 포함.
- Narrow 스키마는 wide 보다 row 수가 많다 (metric 당 1 row). 1 Hz × 50 metric × 10 host = 500 row/s. batching 으로 해소.
- Continuous aggregate 는 refresh 지연 있음 (기본 1~5 min). 실시간 대시보드의 last 5 min 은 raw tier 를 직접 query.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 데이터 모델

```sql
-- 20260421000001_host_metrics_init.sql
CREATE EXTENSION IF NOT EXISTS timescaledb;
CREATE EXTENSION IF NOT EXISTS vector; -- 기존 pgvector 유지

CREATE TABLE host_metrics (
    tenant_id   UUID            NOT NULL,       -- denormalized from hosts.tenant_id
                                                --   (see §4.1: double-gate even when
                                                --    hosts row is corrupted / rolled back)
    host_id     UUID            NOT NULL,
    ts          TIMESTAMPTZ     NOT NULL,
    metric      TEXT            NOT NULL,       -- 'cpu.util', 'gpu.0.temp', 'nic.eth0.rx_bps'
    value       DOUBLE PRECISION NOT NULL,
    unit        TEXT,                            -- 'pct', 'celsius', 'watts', 'bytes_per_sec', 'bytes'
    labels      JSONB           NOT NULL DEFAULT '{}'::jsonb,
                                                -- { "gpu_index": 0, "gpu_name": "A100 80GB",
                                                --   "iface_kind": "ethernet", "source": "dcgm" }
    CONSTRAINT labels_allowlist CHECK (
        labels ?| ARRAY['source','gpu_index','gpu_name','iface_kind','chip','mount']
        OR labels = '{}'::jsonb
    )
);

SELECT create_hypertable(
    'host_metrics',
    'ts',
    chunk_time_interval => INTERVAL '1 hour'
);

-- Tenant-leading composite — the common query is
-- WHERE tenant_id = :t AND host_id = :h AND metric = :m AND ts >= :from.
-- Keeping tenant first means a cross-tenant query without the tenant
-- clause can't even warm-scan a chunk.
CREATE INDEX host_metrics_lookup_idx
    ON host_metrics (tenant_id, host_id, metric, ts DESC);

-- Fleet-wide scans for a single metric (alerting, correlation).
-- Still tenant-leading so cross-tenant stays blocked at the planner.
CREATE INDEX host_metrics_metric_ts_idx
    ON host_metrics (tenant_id, metric, ts DESC);

-- Legal hold — certain host_id / window combinations are exempt from
-- the retention job. Inserts go through a trigger that checks this
-- table; the retention policy also LEFT JOINs against it and skips
-- chunks that still intersect an active hold. See §4.3.
CREATE TABLE host_metrics_retention_hold (
    tenant_id   UUID            NOT NULL,
    host_id     UUID            NOT NULL,
    metric      TEXT,                           -- NULL = all metrics for the host
    hold_from   TIMESTAMPTZ     NOT NULL,
    hold_to     TIMESTAMPTZ     NOT NULL,
    reason      TEXT            NOT NULL,       -- "incident-2026-04-21-ooms"
    created_by  UUID            NOT NULL,
    created_at  TIMESTAMPTZ     NOT NULL DEFAULT NOW(),
    expires_at  TIMESTAMPTZ                     -- auto-release, NULL = indefinite
);
```

#### 2.1.1 Narrow vs wide

채택한 narrow 스키마의 이유는 §1.4.2. Wide snapshot (`cpu_util REAL, mem_used_bytes BIGINT, gpus JSONB, ...`) 은 **하드웨어 shape 변화에 ALTER TABLE 이 필요**해 migration 이 자주 일어난다. narrow 는 metric identity 가 문자열 → 신규 센서 추가가 INSERT 한 줄.

#### 2.1.2 Metric name convention

```
<family>[.<instance>][.<leaf>]
```

예시 (v0.1 수집 표면 기준 전수):

| metric | unit | 비고 |
|---|---|---|
| `cpu.util` | pct | 전체 CPU util (/proc/stat delta) |
| `cpu.load_1m` | - | loadavg |
| `cpu.load_5m` | - | loadavg |
| `mem.used_bytes` | bytes | total - available |
| `mem.available_bytes` | bytes | MemAvailable |
| `mem.swap_used_bytes` | bytes | SwapTotal - SwapFree |
| `gpu.<i>.util` | pct | nvidia-smi / dcgmi |
| `gpu.<i>.mem_used_mib` | mib | nvidia-smi |
| `gpu.<i>.temp` | celsius | nvidia-smi |
| `gpu.<i>.power_w` | watts | nvidia-smi |
| `temp.<chip>.<label>` | celsius | `k10temp-pci-00c3` / `Tctl` 등 |
| `disk.<mount>.used_bytes` | bytes | df |
| `disk.<mount>.total_bytes` | bytes | df (low-frequency 추천) |
| `nic.<iface>.rx_bps` | bytes_per_sec | /proc/net/dev delta (신규) |
| `nic.<iface>.tx_bps` | bytes_per_sec | 신규 |
| `power.psu_watts` | watts | ipmitool dcmi (현재 대부분 unavailable) |
| `power.gpu_watts` | watts | Σ gpu.*.power_w |

`labels` JSONB 는 추가 컨텍스트 — e.g. `{"source": "dcgm"}` 는 dcgm 과 nvidia-smi 병행 상황에서 출처 추적, `{"iface_kind": "wifi"}` 같은 미래 확장 여지.

### 2.2 Continuous aggregates — 자동 downsampling

```sql
-- 5-second bucket
CREATE MATERIALIZED VIEW host_metrics_5s
WITH (timescaledb.continuous) AS
SELECT
    host_id,
    metric,
    time_bucket(INTERVAL '5 seconds', ts) AS bucket,
    AVG(value)                            AS avg,
    MIN(value)                            AS min,
    MAX(value)                            AS max,
    COUNT(*)                              AS samples
FROM host_metrics
GROUP BY host_id, metric, bucket
WITH NO DATA;

SELECT add_continuous_aggregate_policy('host_metrics_5s',
    start_offset => INTERVAL '2 hours',
    end_offset   => INTERVAL '10 seconds',
    schedule_interval => INTERVAL '30 seconds');

-- 1-minute bucket (stacks on top of 5s)
CREATE MATERIALIZED VIEW host_metrics_1m
WITH (timescaledb.continuous) AS
SELECT
    host_id, metric,
    time_bucket(INTERVAL '1 minute', bucket) AS bucket,
    AVG(avg)   AS avg,
    MIN(min)   AS min,
    MAX(max)   AS max,
    SUM(samples) AS samples
FROM host_metrics_5s
GROUP BY host_id, metric, 3;
-- + policy analog

-- 5-minute bucket (stacks on top of 1m)
-- analog
```

#### 2.2.1 Retention policy — tier 별 보존

| Tier | 해상도 | 보존 기간 | 호스트당 row (50 metric 기준) |
|---|---|---|---|
| raw (`host_metrics`) | 1 s | **72 h** | 4.3 M / day → 13 M / 3d |
| 5s avg (`host_metrics_5s`) | 5 s | 7 day | 121 k / day |
| 1m avg (`host_metrics_1m`) | 1 m | 30 day | 72 k / month |
| **5m avg (`host_metrics_5m`)** | 5 m | **90 day** | 1.3 M / 90d |
| **1h avg (`host_metrics_1h`)** | 1 h | **2 year** | 876 k / year |

SRE 리뷰 반영: raw 24h → 72h 로 확장 (사건 발생 후 주말 포함 회고 윈도우 보장). 5m tier 는 1년 → 90일 로 축소, 장기 보존은 1h tier 가 담당 — capacity planning 은 시간 단위로 충분.

```sql
SELECT add_retention_policy('host_metrics',     INTERVAL '72 hours');
SELECT add_retention_policy('host_metrics_5s',  INTERVAL '7 days');
SELECT add_retention_policy('host_metrics_1m',  INTERVAL '30 days');
SELECT add_retention_policy('host_metrics_5m',  INTERVAL '90 days');
SELECT add_retention_policy('host_metrics_1h',  INTERVAL '2 years');
```

이 수치는 config 가능 — `[metrics.retention.raw_hours = 72]` 등. **Legal hold 가드는 §4.3**.

#### 2.2.2 Storage estimate

호스트 10대 × 50 metric × 1 Hz × 24 h = 43.2 M raw rows / day. narrow row ≈ 96 bytes (uuid 16 + ts 8 + text avg 20 + float8 8 + text unit avg 6 + jsonb 32 + header/align 6) → ≈ 4 GB/day raw. TimescaleDB native compression (1h chunks, compress after 2h) 로 10~20x 압축 → **실저장 200~400 MB/day for 10 호스트**. 7일치 5s tier 와 30일치 1m tier 는 raw 대비 1/5 이하. 1년 5m tier 는 5 MB 수준.

### 2.3 Ingestion path

```
┌─────────────────┐     mpsc (bounded 4096)     ┌────────────────────┐
│ collect_stats() │ ──► MetricSample batches ──►│ metrics_writer bg  │─► Postgres
│  (per poll)     │                             │  (tokio spawn)     │    batch INSERT
└─────────────────┘                             └────────────────────┘
```

#### 2.3.1 `MetricSample` 구조

```rust
pub struct MetricSample {
    pub host_id: Uuid,
    pub ts:      DateTime<Utc>,
    pub metric:  String,
    pub value:   f64,
    pub unit:    Option<&'static str>,
    pub labels:  serde_json::Value,
}
```

`collect_stats` 가 반환 타임에 `ServerStats` 를 순회하며 `Vec<MetricSample>` 을 조립해 `metrics_tx.try_send(batch)` 한다. 실패 시 dropped_count += 1, tracing::warn!.

#### 2.3.2 `metrics_writer` — batch INSERT

```rust
async fn run_metrics_writer(
    mut rx: mpsc::Receiver<Vec<MetricSample>>,
    pool: PgPool,
) {
    let mut buf: Vec<MetricSample> = Vec::with_capacity(BATCH_MAX);
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(batch) = maybe else { break; };
                buf.extend(batch);
                if buf.len() >= BATCH_MAX { flush(&pool, &mut buf).await; }
            }
            _ = tick.tick() => {
                if !buf.is_empty() { flush(&pool, &mut buf).await; }
            }
        }
    }
}
```

`flush` 는 unnest-based INSERT:

```sql
INSERT INTO host_metrics (host_id, ts, metric, value, unit, labels)
SELECT * FROM UNNEST($1::uuid[], $2::timestamptz[], $3::text[], $4::float8[], $5::text[], $6::jsonb[]);
```

BATCH_MAX = 500 추천 (1 INSERT ≈ 1~3 ms). 500 ms 주기 tick 으로 지연 한도 보장.

#### 2.3.3 Drop policy

mpsc `try_send` 실패 = 채널 포화. 이 경우:
- sample 버림 (never block hot path — 최우선 규칙).
- `gadget_metrics_dropped_total` 카운터 증가 (노출은 향후 Prometheus 호환 `/metrics` endpoint 로 — 이 문서의 스코프 밖).
- tracing warn! (rate-limited) — 운영자 가시성.

### 2.4 Read API

```
GET /api/v1/web/workbench/servers/{host_id}/metrics
        ?metric=cpu.util
        &from=2026-04-21T00:00:00Z
        &to=2026-04-21T01:00:00Z
        &bucket=auto         # raw | 5s | 1m | 5m | auto
        &agg=avg             # avg | min | max (continuous aggregate 의 컬럼)
```

응답:

```json
{
  "host_id": "c47ff97e-...",
  "metric": "cpu.util",
  "unit": "pct",
  "resolution": "5s",
  "points": [
    {"ts": "2026-04-21T00:00:00Z", "avg": 2.1, "min": 1.8, "max": 2.9},
    {"ts": "2026-04-21T00:00:05Z", "avg": 2.3, "min": 1.9, "max": 3.1}
  ],
  "dropped_frames": 0,
  "refresh_lag_seconds": 18,
  "labels_seen": [
    {"source": "dcgm"}
  ]
}
```

**`dropped_frames`** — 응답 범위 내에서 ingestion mpsc 포화로 드랍된 샘플 수. 0 이 아니면 그래프에 구멍 → UI 가 hatched 영역으로 렌더. 카운터는 `metric_dropped_total (host_id, ts_bucket)` 테이블에서 읽는다 (bg worker drop path 에 이미 기록).

**`refresh_lag_seconds`** — continuous aggregate 의 최신 bucket 이 raw 데이터 대비 얼마나 뒤처졌는지. aggregate tier 를 요청했을 때만 값, raw tier 면 0. UI 는 이 값 > 10 초일 때 "마지막 10초는 raw tier 에서 stitch" 로 대응.

#### 2.4.1 `bucket=auto` 선택 규칙

| 요청 window | 선택 tier |
|---|---|
| ≤ 10 min | raw (`host_metrics`) |
| ≤ 2 hour | 5s (`host_metrics_5s`) |
| ≤ 2 day | 1m (`host_metrics_1m`) |
| > 2 day | 5m (`host_metrics_5m`) |

규칙은 "반환 point 수가 항상 300~2000 사이" 를 유지. 과도 해상도 → 브라우저 render 부담. 과소 해상도 → 스파이크 smoothing 손실.

#### 2.4.2 Scope + 인증

기존 workbench route 들과 동일. `Scope::OpenAiCompat` (workbench 의 기본) + bundle descriptor 의 `required_scope` 확장은 하지 않는다 (메트릭 조회는 읽기 — 기본 scope 로 충분). tenant 경계는 host_id 의 owning tenant 로 판정 (inventory 확장 필요 — v0.1 에는 tenant_id 컬럼 없음. 이 문서에서 추가).

### 2.5 UI 소비

#### 2.5.1 Sparkline on card

각 호스트 카드 하단에 5 분 윈도우 `cpu.util` / `mem.used_bytes` / `gpu.0.util` 스파크라인. `<svg>` 기반 60~120 point 간단 line. fetch 는 호스트당 3개 metric 을 `raw` tier 로 병렬 요청 (5 min × 1 Hz = 300 point).

#### 2.5.2 Detail drawer

카드 클릭 → full-screen drawer:
- 시간 범위 picker (last 5min / 1h / 6h / 24h / 7d / custom)
- 지표 별 큰 그래프 (recharts or d3) — tier 자동
- 여러 metric overlay (CPU + GPU + PSU 같은 축 / 별도 축)

이 부분은 별도 PR 로 나눈다.

### 2.6 Docker image

```dockerfile
# images/gadgetron-pgvector-timescale/Dockerfile
FROM timescale/timescaledb:latest-pg16
RUN apt-get update && apt-get install -y postgresql-16-pgvector \
  && rm -rf /var/lib/apt/lists/*
```

`demo.sh` 의 이미지 참조를 `gadgetron/pgvector-timescale:pg16` 로 교체. CI 에서 image build + push.

검증 쿼리:
```sql
CREATE EXTENSION timescaledb;
CREATE EXTENSION vector;
SELECT extname, extversion FROM pg_extension WHERE extname IN ('timescaledb', 'vector');
```

---

## 3. 마이그레이션 & 롤아웃

### 3.1 순서

1. **이미지 PoC** (task #18) — pgvector + timescaledb 공존 검증 + CI image 빌드
2. **Schema migration** (task #19) — `host_metrics` + continuous aggregates + retention policies
3. **NIC collector** (task #20) — 새 metric family 수집 (timeseries 쓰기 전에 들어와야 ingestion 에서 포함 가능)
4. **Ingestion worker** (task #21) — `run_metrics_writer` + `collect_stats` → `MetricSample` 변환
5. **Read API** (task #22)
6. **UI sparkline** (task #23)
7. **UI detail** (task #24)
8. **E2E + perf** (task #25) — 10-host 시뮬레이션, 지속적 1 Hz, tier 선택 정확성, retention policy firing

### 3.2 Backward compat

`server.stats` gadget 응답 포맷은 변경하지 않음. timeseries 는 **추가 채널** — 기존 pull-path 는 그대로 응답하고 동시에 MetricSample 을 버스에 던진다. 롤백 가능 (ingestion 비활성화해도 stats 는 계속 동작).

### 3.3 Retention 실행 책임

TimescaleDB 의 `add_retention_policy` 가 pg bg worker 로 자동 실행. gadgetron 프로세스는 관여 없음. migration 한 번 실행 후 fire-and-forget.

---

## 4. 보안 / 격리

### 4.1 Tenant 경계 — 이중 게이트

**Security 리뷰 반영: `host_metrics.tenant_id` 를 denormalize 해 직접 컬럼으로 둔다.** inventory 테이블의 `hosts.tenant_id` 하나에만 의존하면 hosts 가 롤백되거나 corrupt 된 상태에서 경계가 무너진다. 대신:

1. **Ingestion 시점**: worker 가 `MetricSample` 마다 `(tenant_id, host_id)` 를 `hosts` 에서 조회해 주입. 미존재 host_id 의 샘플은 drop.
2. **쿼리 시점**: composite index `(tenant_id, host_id, metric, ts DESC)` 가 leading. 핸들러가 WHERE 절에 `tenant_id = ctx.tenant_id` 강제.
3. **host 소속 변경**: 기존 row 의 tenant_id 는 과거 소유를 보존 (historical evidence 성격). 이전 후 신규 샘플만 새 tenant 소속.

### 4.2 Role 분리 — writer 는 INSERT only

현재 `gadgetron` DB role 은 audit / billing / quotas 전부를 담당. metrics writer 까지 같은 role 이면 "writer 가 과거 row 를 지울 수 있는가" 질문에 "YES" 가 된다 — 증거 무결성 위배.

```sql
CREATE ROLE gadgetron_metrics_writer NOLOGIN;
GRANT INSERT ON host_metrics TO gadgetron_metrics_writer;
-- DELETE, UPDATE 는 gadgetron_metrics_admin 만 (migration + retention policy 실행용)
CREATE ROLE gadgetron_metrics_admin NOLOGIN;
GRANT ALL ON host_metrics TO gadgetron_metrics_admin;

-- 애플리케이션 role 이 필요 시 SET ROLE 로 escalate
GRANT gadgetron_metrics_writer TO gadgetron;
```

Ingestion worker 는 항상 `SET ROLE gadgetron_metrics_writer` 후 INSERT. 코드 경로에서 DELETE 가 나오면 런타임 실패 → test 에서 catch.

### 4.3 Legal hold — retention 의 안전장치

사고 조사 중인 host_id 의 데이터가 retention policy 로 자동 drop 되면 evidence 파괴. 대응:

1. **`host_metrics_retention_hold` 테이블** (§2.1) — `(tenant_id, host_id, metric?, hold_from, hold_to, reason, expires_at)`.
2. **Retention policy override**: TimescaleDB 의 `drop_chunks` 는 predicate-based 가 아니므로, **custom retention job** 이 필요. 각 tier 의 기본 retention 대신 `scheduled_background_job` 을 등록 → hold 를 LEFT JOIN 해 교집합이 비어있는 chunk 만 drop.
3. **UI 노출**: `/web/servers/{id}` 카드에 "retention held until: YYYY-MM-DD — incident-2026-04-21-ooms" 배지.
4. **자동 만료**: `expires_at` 이 지난 hold 는 next retention run 이 정리 + audit log.

예시:
```sql
INSERT INTO host_metrics_retention_hold
  (tenant_id, host_id, metric, hold_from, hold_to, reason, created_by, expires_at)
VALUES ($tenant, $host, NULL, '2026-04-20T18:00Z', '2026-04-21T03:00Z',
        'incident-2026-04-21-ooms', $user, NOW() + INTERVAL '30 days');
```

### 4.4 Dropped data — 공격 벡터 관점

bounded mpsc 가 포화되면 sample 이 drop 된다. 악의적 high-frequency 수집으로 다른 호스트의 데이터를 밀어낼 수 있다 → per-host token bucket 추천 (이 문서 스코프 밖, 향후 작업).

### 4.5 PII / 민감 정보 + Labels allowlist

host IP + hostname 은 이미 inventory 에 있다. 메트릭 값 자체는 숫자 — PII 없음. 다만 `labels` JSONB 에 자유 문자열이 들어가면 누수 위험.

**이중 방어**:
- **DB CHECK constraint** (§2.1 스키마에 포함): `labels ?| ARRAY['source','gpu_index','gpu_name','iface_kind','chip','mount']` 만 허용.
- **Ingestion worker** 가 입력 `labels` 를 allowlist 밖 키에 대해 drop, warn.

**Penny-friendly 확장** (AI 리뷰 반영): allowlist 에 `gpu_name` + `iface_kind` + `chip` + `mount` 를 **미리** 포함. LLM 이 "첫 번째 A100 의 util" 같은 자연어 질의를 `gpu.0.util` 로 매핑할 때 `labels.gpu_name = "A100 80GB"` 를 근거로 활용. 이들은 metric 이름과 중복 정보지만 LLM 프롬프트-cost 절감 효과가 큼.

---

## 5. 테스트 전략

### 5.1 Unit

- `MetricSample` JSON shape + round-trip
- `metric_name_for(&ServerStats)` 변환 함수 (ServerStats → Vec<MetricSample>) exhaustive: CPU, mem, per-disk, per-chip temp, per-GPU, per-NIC, power
- `auto_tier(from, to)` 선택 규칙
- `parse_proc_net_dev(&str)` 파서

### 5.2 Integration

- 실제 TimescaleDB 컨테이너 spawn (test harness)
- 1000 row insert → query `last 5 min raw` → 반환 row 수 = 1000
- continuous aggregate manual refresh → 5s tier 조회 → 200 row
- retention policy: 25h 전 데이터 삽입 → policy 수동 트리거 → drop_chunks 호출 → 해당 chunk 제거

### 5.3 E2E (scripts/e2e-harness/run.sh)

새 gate 추가:
- `9a` — `POST server.stats` x5 → `SELECT count(*) FROM host_metrics WHERE host_id = $1` 이 ≥ 5 * metric_count
- `9b` — `GET /servers/{id}/metrics?metric=cpu.util&from=-5m&to=now&bucket=auto` → 200 + points.length > 0
- `9c` — cross-tenant 누출 방지: tenant A 키로 tenant B 의 host metric 조회 → 403

### 5.4 Perf

- 10 호스트 × 1 Hz × 50 metric × 10 분 = 300k row 지속 쓰기 → gadgetron CPU 사용량 < 5%, INSERT p99 < 20 ms
- read API p50 < 50 ms / p99 < 200 ms (last 5 min, raw tier)
- read API (last 7 day, 1m tier) p99 < 500 ms

---

## 6. 오픈 이슈 / 남은 액션 아이템

### 6.1 이번 리뷰에서 결정된 액션 (설계 doc 외 수반 작업)

1. **ADR-METRICS-01 "metric name stability"** — Platform Architect 요구. `cpu.util` → `cpu.total.util` 같은 rename 은 breaking change 로 간주; 신규 metric 만 추가. ADR 초안은 이 doc 의 sibling 으로 추가 (`docs/adr/ADR-METRICS-01-name-stability.md`).
2. **이미지 ownership doc** (`docs/ops/postgres-image.md`) — 누가 timescaledb + pgvector 이미지 tag 를 소유·릴리즈하나. 현재 demo.sh / CI / production 이 일관된 tag 를 쓰도록 단일 소스 확립.
3. **Push agent 수용 ADR** — 향후 agent 가 direct INSERT 가 아닌 gateway API 경유로 쓴다는 결정을 별도 ADR 로 박기. 이번 스키마가 push-neutral 한지 재검토 (narrow + tenant_id + labels 충분).
4. **Penny retrospective gadget** (`host.metric.history`, `host.metric.list(hint?)`) 은 **phase2 doc 17 번** 에서 별도 취급.

### 6.2 남은 리뷰 포인트 (필요 시 v0.2 에서 재논의)

- **Downsampling 시 percentile** — P95/P99 도 보존할지 (avg/min/max 만으로 충분한가). 현재 avg/min/max 만; P99 추가는 storage 2x.
- **Per-host token bucket** — 4.4 에 언급된 ingestion rate limit. 현재 drop-on-full + counter 로 대응, 정교한 QoS 는 v0.3.
- **Ingestion multi-writer** — 지금은 single mpsc. 호스트 100+ 시 단일 채널 병목 우려. 호스트 hash 로 shard 하는 구조는 v0.3.
- **UI auto-tier override** — "마지막 10초는 raw" 규칙에 사용자가 강제 tier 를 요구하면 쿼리 파라미터 `bucket=raw` 로 override 가능. 기본은 auto.

---

## 7. 참조

- TimescaleDB: https://docs.timescale.com/
- Prometheus storage format + retention patterns: https://prometheus.io/docs/prometheus/latest/storage/
- gadgetron audit: `crates/gadgetron-core/src/audit/event.rs`, `crates/gadgetron-xaas/src/audit/`
- 관련 ADR: ADR-P2A-05 (agent-centric control plane), ADR-P2A-06 (approval flow deferred), ADR-P2A-10 (bundle terminology)
- 관련 phase2: 07-bundle-server (host 모델), 12-external-gadget-runtime (외부 runtime 의 evidence 원칙)
