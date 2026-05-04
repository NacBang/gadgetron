# Gadgetini Server Monitor Integration

> **담당**: @devops-sre-lead + @ux-interface-lead
> **상태**: Draft — user-approved concept, implementation pending
> **작성일**: 2026-05-04
> **최종 업데이트**: 2026-05-04
> **관련 크레이트**: `gadgetron-bundle-server-monitor`, `gadgetron-cli`, `gadgetron-web`
> **Phase**: P2B

---

## 1. 철학 & 컨셉 (Why)

Gadgetini는 서버 본체가 아니라 서버에 붙은 액체냉각 모니터링 child device다. 운영자 관점에서는 `dg5W-SKU02` 같은 서버 카드에서 CPU/GPU/RAM과 같은 맥락으로 coolant, leak, level, chassis 상태를 함께 봐야 하므로 별도 bundle보다 `server-monitor`의 per-host 확장으로 다룬다.

핵심 원칙:
- **부모 서버 중심 모델**: Gadgetini는 parent `HostRecord`에 매달린 child monitor다. 독립 서버로 등록하지 않는다.
- **IPv4 불안정성 회피**: 저장된 접속 기준은 IPv4가 아니라 IPv6 link-local + parent interface + MAC이다.
- **비밀번호 저장 금지**: factory/default credential은 등록 시 key bootstrap에만 쓰고, 이후에는 ed25519 key만 쓴다.
- **기존 telemetry 경로 재사용**: `server.stats`와 background poller가 센서 값을 포함하고, `host_metrics`에 같은 방식으로 저장한다.

채택하지 않는 대안:
- Gadgetini를 별도 `server.add` host로 등록: parent server card와 상태가 분리되어 사용성이 나쁘고, Redis가 localhost-only라 child SSH 특수 처리가 어차피 필요하다.
- 매 poll마다 password SSH: 운영 부담과 보안 위험이 크다.
- Gadgetini HTTP UI scraping: Next.js UI 구조에 종속되고 Redis의 canonical sensor 값보다 취약하다.

## 2. 상세 구현 방안 (What & How)

### 2.1 공개 API

`HostRecord`에 child monitor 설정을 추가한다. 기존 inventory JSON은 `#[serde(default)]`로 호환한다.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GadgetiniRecord {
    pub enabled: bool,
    pub host_name: String,          // default: "gadgetini.local"
    pub ssh_user: String,           // default: "gadgetini"
    pub ssh_port: u16,              // default: 22
    pub parent_iface: String,       // e.g. "enp3s0f1np1"
    pub ipv6_link_local: String,    // e.g. "fe80::584d:..."
    pub mac: Option<String>,
    pub key_path: PathBuf,          // keys/<host-id>-gadgetini
    pub web_port: u16,              // default: 3001
    pub last_ok_at: Option<DateTime<Utc>>,
}

pub struct HostRecord {
    // existing fields...
    #[serde(default)]
    pub gadgetini: Option<GadgetiniRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GadgetiniStats {
    pub air_humit: Option<f32>,
    pub air_temp: Option<f32>,
    pub chassis_stabil: Option<bool>,
    pub coolant_delta_t1: Option<f32>,
    pub coolant_leak: Option<bool>,
    pub coolant_level: Option<bool>,
    pub coolant_temp: Option<f32>,
    pub coolant_temp_inlet1: Option<f32>,
    pub coolant_temp_inlet2: Option<f32>,
    pub coolant_temp_outlet1: Option<f32>,
    pub coolant_temp_outlet2: Option<f32>,
    pub host_stat: Option<i32>,
}

pub struct ServerStats {
    // existing fields...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gadgetini: Option<GadgetiniStats>,
}
```

`server.add` input schema gains:

```json
{
  "gadgetini": {
    "enabled": true,
    "host_name": "gadgetini.local",
    "ssh_user": "gadgetini",
    "ssh_password": "one-shot only; optional",
    "try_factory_default": true
  }
}
```

`server.update` gains the same `gadgetini` block so an already-registered host can attach or detach Gadgetini without re-registering the server.

### 2.2 내부 구조

Discovery runs through the parent host:

```text
Gadgetron host
  -> SSH key to parent server (`dg5W-SKU02`)
     -> parent executes:
        getent hosts gadgetini.local
        avahi-resolve-host-name -6 gadgetini.local
        ip -6 neigh show dev <iface>
```

The selected child identity is `(parent_iface, ipv6_link_local, mac)`. MAC is used as a stability check: if IPv6 changes but MAC is the same, update `ipv6_link_local`; if MAC changes, report a warning and require operator confirmation.

Bootstrap uses local `sshpass` and the parent as a TCP relay. This avoids installing `sshpass` on the parent host.

```text
sshpass -e ssh \
  -o ProxyCommand="ssh <parent-key-options> parent nc -6 <child-ipv6>%<parent-iface> 22" \
  gadgetini@<child-ipv6>
```

Verified on 2026-05-04:
- Parent: `dg5W-SKU02` (`192.168.1.166`)
- Parent iface: `enp3s0f1np1`
- Child IPv6: `fe80::584d:7732:805c:a8f9`
- ProxyCommand route succeeded and returned `PROXY_NC_IPV6_OK`.

After bootstrap, `collect_gadgetini_stats(parent, gadgetini_record)` uses the stored key and the same ProxyCommand route to run:

```sh
redis-cli -h 127.0.0.1 -p 6379 --raw MGET \
  air_humit air_temp chassis_stabil coolant_delta_t1 coolant_leak \
  coolant_level coolant_temp coolant_temp_inlet1 coolant_temp_inlet2 \
  coolant_temp_outlet1 coolant_temp_outlet2 host_stat
```

Parsing is lenient. Missing keys become `None`; malformed values add a `warnings[]` entry but do not fail `server.stats`.

### 2.3 설정 스키마

The factory username can be a non-secret default. The factory password must come from runtime configuration, not from committed code.

```toml
[server_monitor.gadgetini]
factory_username = "gadgetini"
factory_password_env = "GADGETRON_GADGETINI_FACTORY_PASSWORD"
host_name = "gadgetini.local"
web_port = 3001
redis_port = 6379
connect_timeout_secs = 8
```

Validation:
- `factory_username`: non-empty, <= 64 bytes.
- `factory_password_env`: env-var name only. If unset, factory-default auto-probe is skipped and UI asks for custom credentials.
- `host_name`: no shell metacharacters; default `gadgetini.local`.
- `web_port`, `redis_port`: `1..=65535`.
- no password value is written to `inventory.json`, logs, DB, or API responses.

### 2.4 에러 & 로깅

No new `GadgetronError` variant is required. Gadget errors use existing `GadgetError::InvalidArgs` and `GadgetError::Execution` with redacted messages.

Tracing events:
- `server_monitor_gadgetini.discovery`: parent host id, interface, discovered IP/MAC.
- `server_monitor_gadgetini.bootstrap`: host id, success/failure, auth mode (`factory_default` or `custom_once`), no password.
- `server_monitor_gadgetini.stats`: host id, elapsed ms, missing key count, parse warning count.

STRIDE:

| Threat | Asset / boundary | Mitigation |
|---|---|---|
| Spoofing | `gadgetini.local` or reused IPv6 | pin MAC + known host key after bootstrap; warn on MAC/host-key change |
| Tampering | Redis sensor values | read-only telemetry; show source and last update; do not issue writes in stats path |
| Repudiation | who attached child monitor | existing workbench action audit captures `server.add` / `server.update` |
| Info disclosure | factory/custom password | one-shot secret, env indirection, redacted logs, no DB persistence |
| DoS | slow child SSH blocks server poller | per-child timeout; missing child only adds warning and stale status |
| Elevation of privilege | child credential used to alter parent | child key is stored separately; commands limited to Redis read path except explicit approved shell |

### 2.5 의존성

No new Rust crate is required. Existing dependencies cover the path:
- OpenSSH + `sshpass` binary for one-shot password bootstrap.
- `redis-cli` on Gadgetini for local Redis reads.
- Parent `nc` for link-local TCP relay. If `nc` is missing, fallback to `socat` if present, otherwise show actionable error.

### 2.6 서비스 기동 / 제공 경로

No new process is introduced. `./scripts/launch.sh` and `gadgetron serve` continue to start the same server.

Operator setup:
1. Set `GADGETRON_GADGETINI_FACTORY_PASSWORD` in local launch environment or `.env` outside git.
2. Start Gadgetron normally.
3. In `/web/servers`, register or update a host with `Include Gadgetini cooling monitor`.
4. If factory default succeeds, no credential fields are shown.
5. If factory default fails or env is unset, UI expands custom username/password fields.

Existing background poller picks up child telemetry on the next tick after inventory save.

## 3. 전체 모듈 연결 구도 (Where)

```text
gadgetron-web /web/servers
  -> workbench action server-add/server-update/server-stats
     -> gadgetron-bundle-server-monitor::ServerMonitorProvider
        -> InventoryStore::HostRecord::gadgetini
        -> ssh::exec / sshpass bootstrap / ProxyCommand parent nc relay
        -> collectors::collect_stats + collect_gadgetini_stats
        -> metrics::stats_to_samples
           -> host_metrics table
```

Crate boundary:
- child monitor types live in `gadgetron-bundle-server-monitor` because they are bundle-local inventory and telemetry details.
- `gadgetron-core` remains unchanged, preserving D-12 leaf-crate boundaries.
- `gadgetron-web` mirrors response shapes in local TypeScript interfaces; no shared core type is introduced because this is bundle-specific workbench telemetry, not a platform-wide GPU metric contract.

Graph validation:
- `graphify query "server monitor bundle inventory HostRecord collect_stats metrics" --budget 1200` finds the design-level node `Server Bundle (SSH primitive)` in Community 14 and related bundle architecture nodes.
- `graphify explain "HostRecord"` returns `No node matching 'HostRecord' found`.
- `graphify path "server-monitor" "host_metrics"` returns `No node matching 'server-monitor' found`.

Interpretation: current `graphify-out` was generated for `/Users/junghopark/dev/gadgetron-plan` on 2026-04-20 and does not index the current `bundles/server-monitor` Rust files. Implementation plan must include a fresh graphify update/query step before merge, but the local source inspection confirms the dependency path above.

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

- `GadgetiniStats` parser: converts Redis string values to typed fields.
- boolean mapping: `coolant_leak=0` means no leak, `coolant_level=1` means OK, `chassis_stabil=1` means OK.
- missing/malformed keys: field becomes `None`, warning increments, no hard failure.
- discovery parser: extracts IPv6 link-local, parent interface, MAC from `getent`/`avahi`/`ip neigh` fixture output.
- inventory serde: old `HostRecord` JSON without `gadgetini` loads; new record round-trips without password.
- metric fan-out: `stats_to_samples` emits `gadgetini.*` metrics with expected unit and labels.
- error redaction: password never appears in error/debug strings.

### 4.2 테스트 하네스

Fixtures live under `bundles/server-monitor/tests/fixtures/gadgetini/`:
- `redis_mget_ok.txt`
- `redis_mget_missing.txt`
- `discovery_avahi_ipv6.txt`
- `ip6_neigh_gadgetini.txt`

No wall-clock sleeps. SSH execution is abstracted behind small command-builder/parser functions so most tests avoid network.

### 4.3 커버리지 목표

Target focused branch coverage on:
- parser branches: >= 90%
- inventory compatibility path: explicit regression test
- command argv construction: explicit regression test for `%%` escaping in OpenSSH `ProxyCommand`

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

Integration scenarios:
1. `server.add` with `gadgetini.enabled=true` and factory password env set: inventory gains `gadgetini`, no password stored.
2. `server.add` with factory env unset/failing: response asks UI to collect custom credentials.
3. `server.stats` for a host with `gadgetini`: response contains `gadgetini` object and normal CPU/GPU fields.
4. `host_metrics` receives `gadgetini.coolant_temp_inlet1`, `gadgetini.coolant_leak`, `gadgetini.coolant_level`.
5. `/web/servers` renders cooling summary and warning state.

### 5.2 테스트 환경

Local automated integration should use a fake command runner, not live Gadgetini. Live smoke remains manual because link-local IPv6 scope depends on the physical parent NIC.

Manual smoke target from this session:
- parent host id: `e3bab16e-eb6e-4975-aa6c-cd5f74a29689`
- alias: `dg5W-SKU02`
- parent iface: `enp3s0f1np1`
- Gadgetini IPv6: `fe80::584d:7732:805c:a8f9`
- Gadgetini Redis keys: 12 string keys in DB0.

### 5.3 회귀 방지

These changes must fail tests if:
- code stores the Gadgetini password in inventory JSON.
- factory-default failure blocks ordinary server registration.
- `server.stats` fails hard when Gadgetini is offline.
- OpenSSH `ProxyCommand` uses unescaped `%` and breaks link-local addresses.
- metric names or boolean meanings flip.

### 5.4 운영 검증

Run:

```sh
cargo test -p gadgetron-bundle-server-monitor gadgetini
cargo test -p gadgetron-web ServersPage
cargo build --release -p gadgetron-cli
./scripts/launch.sh
```

Smoke:
1. Open `/web/servers`.
2. Attach Gadgetini to `dg5W-SKU02`.
3. Confirm card shows coolant inlet/outlet, leak, level, chassis.
4. Confirm `server-stats` action still returns in the normal poll cadence.
5. Query `host_metrics` for `metric LIKE 'gadgetini.%'`.

## 6. Phase 구분

- [P1] Inventory schema, discovery parser, key bootstrap, `server.stats` JSON field.
- [P1] Servers card cooling summary.
- [P2] Detail drawer charts and `host_metrics` historical graphs.
- [P2] Reattach/rotate Gadgetini key from UI.
- [P3] Multiple child monitors per server and non-Gadgetini monitor adapters.

## 7. 오픈 이슈 / 의사결정 필요

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| Q-1 | Factory password source | A. code constant / B. env secret / C. always prompt | B | 승인됨 |
| Q-2 | Live child access in automated tests | A. physical Gadgetini / B. fake command runner | B | 승인 필요 없음 |

---

## 리뷰 로그 (append-only)

### Round 1 — 2026-05-04 — @gateway-router-lead @devops-sre-lead
**결론**: Conditional Pass

**체크리스트**:
- [x] 인터페이스 계약 — workbench action path unchanged; `server.add/update/stats` payloads are additive.
- [x] 크레이트 경계 — bundle-local types remain in `gadgetron-bundle-server-monitor`; `gadgetron-core` unchanged.
- [x] 타입 중복 — no overlap with core GPU/node metric types.
- [x] 에러 반환 — existing `GadgetError` path sufficient.
- [x] 동시성 — background poller keeps per-host timeout; child failure is warning-only.
- [x] 의존성 방향 — no new crate edge.
- [ ] 그래프 검증 — current graphify corpus is stale for `bundles/server-monitor`; implementation must refresh graph evidence before merge.

**Action Items**:
- A1: Add implementation-plan step to refresh graphify evidence or document why graphify excludes bundle code.
- A2: Ensure `ProxyCommand` percent escaping has a unit test.

### Round 1.5 — 2026-05-04 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] Threat model — STRIDE table included.
- [x] Secret management — password one-shot only; env indirection; no DB/inventory persistence.
- [x] Auth/authorization — attach/update remains behind existing server-monitor write approval.
- [x] Error UX — factory default failure opens credential fields instead of failing the full registration.
- [x] Defaults — username/host name defaulted; password source secure-by-default.
- [x] Runbook — launch and smoke path included.

**Action Items**:
- A3: UI must never render the default password value.

### Round 2 — 2026-05-04 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] Unit test range covers parser, inventory compatibility, command argv, metrics fan-out.
- [x] Mockability — parser/command-builder split avoids physical network in CI.
- [x] Determinism — no wall-clock dependency in parser tests.
- [x] Integration scenario — server.add -> server.stats -> host_metrics -> UI listed.
- [x] Regression coverage — password persistence and `%` escaping explicitly named.

### Round 3 — 2026-05-04 — @chief-architect
**결론**: Conditional Pass

**체크리스트**:
- [x] Rust idiom — additive serde structs; no new global trait.
- [x] Zero-cost path — no extra hot-path allocation beyond optional child stats.
- [x] Error propagation — no new broad error enum.
- [x] Observability — tracing events named.
- [x] Hot path — child collector must be independently timeout-bounded.
- [ ] Graph evidence — stale corpus must be resolved before final merge.

**다음 라운드 조건**: A1/A2/A3 반영 in implementation plan.
