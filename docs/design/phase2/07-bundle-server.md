# 07 — Server Bundle (SSH primitive)

> **담당**: PM (Codex)
> **상태**: Approved
> **작성일**: 2026-04-18
> **최종 업데이트**: 2026-04-18
> **관련 크레이트**: `gadgetron-core`, `gadgetron-knowledge`, `gadgetron-penny`, `gadgetron-xaas`, future `bundles/server`
> **Phase**: [P2B-alpha] / [P2B-beta] / [P2C]
> **관련 문서**: `docs/design/phase2/06-backend-plugin-architecture.md`, `docs/design/phase2/10-penny-permission-inheritance.md`, `docs/design/phase2/12-external-gadget-runtime.md`, `docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md`, `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md`, `docs/process/04-decision-log.md` D-20260418-01, D-20260418-02, D-20260418-08
> **용어 메모**: 이 문서는 D-20260418-04/05 이후의 Bundle / Plug / Gadget 어휘를 기준으로 쓴다. 역사적 결정 인용에서만 구명칭 `plugin-server` 를 언급한다.

---

## 1. 철학 & 컨셉 (Why)

### 1.1 해결하는 문제

Gadgetron operator 는 Penny 에게 원격 Linux 호스트 fleet 에 대한 관측, 실행, 로그 확인, 서비스 제어, 문제 해결을 맡기고 싶다. 하지만 이 능력이 안전하려면 다음 네 조건이 동시에 성립해야 한다.

1. target host 에 별도 Gadgetron agent 바이너리를 설치하지 않아도 된다.
2. Penny 가 operator 권한을 우회해 더 강한 권한을 획득할 수 없다.
3. cluster 단위 작업도 단순 host 목록이 아니라 knowledge-aware topology 로 해석된다.
4. 누가 무엇을 어떤 범위에 실행했는지 append-only evidence 로 남는다.

Server Bundle 은 이 네 조건을 만족시키는 첫 번째 infra primitive 다. 이 bundle 은 "원격 Linux 박스를 다루는 능력" 을 제공하지만, 권한의 원천이 되지 않는다. 권한은 항상 caller user 에게 있고, bundle 은 그 권한을 대리 실행하는 안전한 기계층이다.

### 1.2 제품 비전과의 연결

- `docs/00-overview.md §1` 의 Gadgetron 은 heterogeneous cluster collaboration platform 이다. 따라서 원격 서버 제어는 특수한 운영 스크립트가 아니라 knowledge, identity, audit 와 연결된 공통 제어 표면이어야 한다.
- `docs/design/phase2/10-penny-permission-inheritance.md` 의 strict inheritance 원칙에 따라 Penny 는 caller 의 권한만 상속한다. Server Bundle 은 이 원칙이 실제 원격 실행에서도 깨지지 않도록 하는 첫 load-bearing bundle 이다.
- `docs/adr/ADR-P2A-10-bundle-plug-gadget-terminology.md` 의 Bundle / Plug / Gadget 분리 관점에서, Server Bundle 은 operator 가 enable/disable 하는 배포 단위이고, `server.*`, `service.*`, `log.*`, `file.*`, `package.*` 는 Penny-facing Gadget 이다.

### 1.3 고려한 대안과 채택하지 않은 이유

| 대안 | 장점 | 채택하지 않은 이유 |
|---|---|---|
| `ai-infra` 안에 서버 제어를 흡수 | 문서 수가 줄어든다 | 일반 Linux host 관리와 LLM infra 관리가 섞여 재사용성과 리뷰 경계가 무너진다 |
| target host 에 경량 Rust agent 설치 | telemetry 와 command path 가 풍부해진다 | 배포/업그레이드/trust bootstrap 비용이 커지고 "무설치 SSH primitive" 장점이 사라진다 |
| Ansible/SSH CLI wrapper | 구현이 빠르다 | stdout parsing 이 brittle 하고 audit, caller inheritance, approval, output quarantine 이 ad-hoc 이 된다 |
| cluster 를 DB 테이블로 직접 모델링 | 쿼리는 단순하다 | topology 설명, 운영 노트, runbook, incident 문맥이 knowledge layer 와 분리되어 drift 한다 |

채택: **SSH primitive + knowledge-defined cluster topology + strict caller inheritance**

### 1.4 핵심 설계 원칙과 trade-off

1. **Primitive independence**
   Server Bundle 은 AI inference 가 전혀 없는 일반 Linux fleet 에도 쓸 수 있어야 한다.
2. **Caller inheritance is absolute**
   bundle 은 caller user 의 권한을 대리 실행할 뿐, daemon privilege 나 hidden service account 권한을 Penny 에게 넘기지 않는다.
3. **Knowledge defines topology, not inventory tables**
   host 인벤토리는 DB 에 있지만, cluster 의미는 wiki page 가 정의한다.
4. **OS allowlist is the final guardrail**
   approval 이 통과되어도 sudoers allowlist 밖의 명령은 target OS 가 거부한다.
5. **Audit is persistent, structured, and append-only**
   tracing log 만으로는 release gate 를 통과할 수 없다. 원격 실행은 별도 audit row 로 남아야 한다.
6. **Secure defaults beat flexible defaults**
   single-tenant multi-user, batch approval threshold 2, cluster-wide auto-T3, no-network credential handling 을 기본값으로 둔다.

Trade-off:

- 문서와 구현 계층이 단순 SSH wrapper 보다 무겁다.
- 하지만 이 구조를 문서 단계에서 닫지 않으면 approval, ACL, runbook, Penny inheritance, audit review 가 모든 구현 PR 에서 반복해서 흔들린다.

---

## 2. 상세 구현 방안 (What & How)

### 2.1 개념과 공개 API

이 bundle 이 다루는 핵심 개념은 다섯 가지다.

| 개념 | 설명 |
|---|---|
| **Host** | SSH 로 접근 가능한 단일 POSIX 서버. `host_id` 는 single-tenant P2B 범위에서 전역 유일한 kebab-case 문자열 |
| **Cluster** | `type = "cluster"` wiki page 가 정의하는 의미적 그룹. bundle 인벤토리 테이블에는 저장하지 않는다 |
| **CredentialBroker** | SSH identity, known_hosts, sudo policy 를 runtime 에 공급하는 유일한 secret ingress |
| **HostConnector** | target host 와 실제로 통신하는 transport 계층. v1 은 OpenSSH subprocess 기반 구현만 포함 |
| **ApprovalGate** | state-changing / destructive gadget 호출을 사용자 승인에 연결하는 공통 policy choke-point |

핵심 타입 시그니처는 다음과 같다.

```rust
use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use gadgetron_core::{
    approval::ApprovalGate,
    entity::{EntityKindSpec, EntityRef},
    error::GadgetronError,
    identity::{AuthenticatedContext, TenantId},
};

pub type HostId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSelector {
    pub ids: Option<Vec<HostId>>,
    pub labels: Option<BTreeMap<String, String>>,
    pub cluster: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecStrategy {
    Parallel,
    Rolling { concurrency: u32 },
    Canary { first: u32, then_concurrency: u32 },
    HaltOnError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOpts {
    pub sudo: bool,
    pub timeout: Duration,
    pub dry_run: bool,
    pub strategy: ExecStrategy,
    pub health_check: Option<String>,
    pub abort_on_failure_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostExecResult {
    pub host_id: HostId,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub total: u32,
    pub succeeded: u32,
    pub failed: u32,
    pub timed_out: u32,
    pub aborted: bool,
    pub per_host: Vec<HostExecResult>,
}

#[async_trait]
pub trait CredentialBroker: Send + Sync + std::fmt::Debug {
    async fn ssh_identity(
        &self,
        actor: &AuthenticatedContext,
        host: &HostId,
    ) -> Result<SshIdentity, GadgetronError>;

    async fn known_hosts(&self, host: &HostId) -> Result<KnownHostsMaterial, GadgetronError>;

    async fn sudo_policy(
        &self,
        actor: &AuthenticatedContext,
        host: &HostId,
    ) -> Result<SudoPolicy, GadgetronError>;
}

#[async_trait]
pub trait HostConnector: Send + Sync + std::fmt::Debug {
    async fn exec(
        &self,
        actor: &AuthenticatedContext,
        host: &HostRecord,
        identity: SshIdentity,
        command: RemoteCommand,
    ) -> Result<RawHostOutput, GadgetronError>;
}
```

설계 규칙:

- 모든 gadget 진입점은 `AuthenticatedContext` 를 필수 인자로 받는다.
- `server.exec` 는 `approval` 과 `classification` 을 통과한 뒤에만 `HostConnector::exec()` 로 내려간다.
- cluster selector 는 knowledge page 를 통해 host selector 로 환원되며, bundle 내부에 별도 cluster truth table 을 두지 않는다.
- `SshIdentity`, `KnownHostsMaterial`, `SudoPolicy` 는 `serde::Serialize` 를 구현하지 않는다. MCP 응답에 실수로 직렬화될 수 없어야 한다.

### 2.2 Gadget surface

v1 gadget 표면은 다음과 같이 고정한다.

| Gadget | Tier | Mode | 설명 |
|---|:---:|:---:|---|
| `server.list(selector?)` | T1 | auto | 인벤토리 조회 |
| `server.get(id, include_children?)` | T1 | auto | 단일 host 및 선택적 child entity 요약 |
| `server.health(selector)` | T1 | auto | CPU, mem, disk, load, uptime probe |
| `server.metrics(selector, since?, until?)` | T1 | auto | 최근 1시간 링 버퍼 시계열 |
| `log.tail(selector, source, lines?, since?)` | T1 | auto | path 또는 systemd unit 기준 로그 조회 |
| `log.search(selector, pattern, source, since?, until?)` | T1 | auto | grep/journalctl 기반 검색 |
| `server.add(host_config)` | T2 | ask | host 인벤토리 등록 |
| `server.update(id, patch)` | T2 | ask | labels/metadata 수정 |
| `server.remove(id)` | T2 | ask | 인벤토리 제거 |
| `server.exec(selector, cmd, opts)` | T2/T3 | ask | 원격 명령 실행 |
| `service.status(selector, service)` | T1 | auto | systemd 상태 조회 |
| `service.start/stop/restart(selector, service, strategy?)` | T2 | ask | systemd 제어 |
| `file.read(selector, path, max_bytes)` | T1/T2 | auto/ask | 파일 읽기 |
| `file.write(selector, path, content, sudo?)` | T2 | ask | 파일 쓰기 |
| `package.list(selector, filter?)` | T1 | auto | 패키지 목록 |
| `package.install/remove(selector, pkg, strategy?)` | T2/T3 | ask | 패키지 변경 |

분류 규칙:

- read-only 조회는 T1.
- state change 는 T2.
- destructive regex hit 또는 `GT_PKG_WRITE` allowlist 카테고리 진입, cluster-wide 전체 대상, `shutdown`/`mkfs`/`dd`/`rm -rf` 류 명령은 T3.

MCP wire namespace 는 `mcp__server__<tool>` 로 노출한다. 예: `mcp__server__server.exec`.

### 2.3 인벤토리, entity tree, cluster 해석

P2B 는 single-tenant multi-user 이므로 inventory row 는 모두 `tenant_id` 를 가지지만 값은 `"default"` 로 고정된다. `owner_tenant_id` 같은 과거 placeholder 는 쓰지 않는다.

```sql
CREATE SCHEMA bundle_server;

CREATE TABLE bundle_server.hosts (
    id               TEXT PRIMARY KEY,
    tenant_id        TEXT NOT NULL,
    address          TEXT NOT NULL,
    ssh_identity_ref TEXT NOT NULL,
    labels           JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata         JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heartbeat   TIMESTAMPTZ
);

CREATE INDEX idx_bundle_server_hosts_labels
    ON bundle_server.hosts USING GIN (labels);

CREATE TABLE bundle_server.exec_audit (
    id               BIGSERIAL PRIMARY KEY,
    request_id       TEXT NOT NULL,
    tenant_id        TEXT NOT NULL,
    actor_user_id    TEXT NOT NULL,
    host_id          TEXT NOT NULL REFERENCES bundle_server.hosts(id),
    command_preview  TEXT NOT NULL,
    sudo             BOOLEAN NOT NULL,
    policy_decision  TEXT NOT NULL,
    ssh_session_id   TEXT,
    strategy         TEXT NOT NULL,
    exit_code        INT,
    duration_ms      INT,
    started_at       TIMESTAMPTZ NOT NULL,
    completed_at     TIMESTAMPTZ
);
```

EntityTree 등록은 bundle 설치 시 Plug 경로로 수행한다.

```rust
ctx.plugs.entity_kinds.register(EntityKindSpec {
    plugin: "server".into(),
    kind: "host".into(),
    parent_kinds: vec!["cluster".into()],
    display_name: "Server".into(),
    list_fn: Arc::new(list_hosts),
    get_fn: Arc::new(get_host),
    children_fn: Arc::new(|_, _| Ok(vec![])),
});
```

cluster 는 bundle 이 직접 소유하지 않는다. 해석 흐름은 다음과 같다.

1. caller 가 `HostSelector { cluster: Some("prod-ml"), .. }` 를 보낸다.
2. Server Bundle 이 `gadgetron-knowledge` 에 `infra/clusters/prod-ml.md` frontmatter 조회를 요청한다.
3. knowledge layer 가 `label_selector`, `expected_members`, `runbooks`, `allowed_operators` 를 반환한다.
4. Server Bundle 이 inventory labels 에 selector 를 적용해 최종 host 집합을 만든다.
5. `expected_members` 와 실제 해석 결과가 다르면 경고 메타를 반환하지만 실행은 policy 에 따라 계속할 수 있다.

이 구조 덕분에 topology 정의, 운영 노트, runbook, incident 문맥이 같은 knowledge surface 에 유지된다.

### 2.4 실행 경로와 5-layer 보안

#### 2.4.1 정상 실행 시퀀스

```text
caller user
  -> gateway / penny
  -> server gadget dispatcher
  -> selector resolution (ids / labels / cluster wiki page)
  -> command classification (T1/T2/T3)
  -> approval gate (if needed)
  -> credential broker
  -> OpenSSH connector
  -> output quarantine
  -> persistent audit append
  -> caller response
```

#### 2.4.2 CredentialBroker

v1 구현체:

| 구현 | 백엔드 | 포함 여부 |
|---|---|---|
| `EnvBroker` | `GADGETRON_SSH_KEY_<HOST_ID>` 류 env | [P2B-alpha] |
| `OsKeychainBroker` | macOS Keychain / Linux libsecret | [P2B-alpha] |
| `SystemdCredsBroker` | `LoadCredential=` | [P2B-beta] |
| `VaultBroker` | HashiCorp Vault | [P2C] |

강제 규칙:

- private key 는 평문 파일로 디스크에 쓰지 않는다.
- Linux 는 `memfd_create` + sealed FD 를, macOS 는 즉시 unlink 하는 임시 파일을 사용한다.
- broker 구현체의 `Debug` 출력은 항상 redact 된다.
- identity material 은 request scope 로만 유지되고 `Drop` 시 zeroize/close 된다.

#### 2.4.3 HostConnector

v1 transport 는 시스템 OpenSSH subprocess 이다.

```bash
ssh \
  -o ControlMaster=auto \
  -o ControlPath="${XDG_RUNTIME_DIR:-/tmp}/gadgetron-ssh-%C" \
  -o ControlPersist=60s \
  -o StrictHostKeyChecking=yes \
  -o PasswordAuthentication=no \
  -o PreferredAuthentications=publickey \
  -o UserKnownHostsFile=<broker-provided-file> \
  -i /proc/self/fd/N \
  user@host -- <cmd>
```

채택 이유:

- OpenSSH 보안 성숙도와 운영자 친화성이 가장 높다.
- `~/.ssh/config` 와 ProxyJump 를 재활용할 수 있다.
- batch execution 에서 ControlMaster 세션 재사용으로 핸드셰이크 비용을 줄일 수 있다.

#### 2.4.4 Sudo policy

target host 에는 dedicated `gadgetron` user 와 allowlist 기반 sudoers 파일을 둔다.

```text
Cmnd_Alias GT_READ      = /bin/systemctl status *, /usr/bin/journalctl --no-pager *, /usr/bin/tail /var/log/*
Cmnd_Alias GT_SVC       = /bin/systemctl start <svc-allowlist>, /bin/systemctl stop <svc-allowlist>, /bin/systemctl restart <svc-allowlist>
Cmnd_Alias GT_PKG_READ  = /usr/bin/dpkg -l *, /usr/bin/rpm -qa
Cmnd_Alias GT_PKG_WRITE = /usr/bin/apt-get install <pkg-allowlist>, /usr/bin/apt-get remove <pkg-allowlist>

gadgetron ALL=(root) NOPASSWD: GT_READ, GT_PKG_READ, GT_SVC
gadgetron ALL=(root) PASSWD:   GT_PKG_WRITE
```

정책 요약:

- allowlist 밖 명령은 approval 이 통과되어도 OS 가 거부한다.
- package write 는 PASSWD + approval 둘 다 필요하다.
- service control 은 NOPASSWD 여도 state change 이므로 T2 ask 가 기본이다.

#### 2.4.5 ApprovalGate

`docs/adr/ADR-P2A-06-approval-flow-deferred-to-p2b.md` 의 후속 구현이 선행조건이다. 이 문서는 timing 을 다시 열지 않는다. Server Bundle v1 은 approval gate 를 동반한 상태만 ship target 으로 본다.

기본 정책:

- `batch_approval_threshold = 2`
- `cluster_wide_auto_t3 = true`
- destructive T3 는 "Always allow" 를 금지하고 request 단위 재확인을 강제
- approval payload 에는 target host 목록, strategy, command preview, estimated blast radius 를 모두 포함

#### 2.4.6 Output quarantine

SSH stdout/stderr 는 항상 untrusted text 로 처리한다.

1. Penny 로 보내는 출력은 `<untrusted_output ...>` 펜스로 감싼다.
2. host 당 `stdout_max_bytes = 65536`, `stderr_max_bytes = 32768` 를 넘으면 truncate 한다.
3. non-UTF8 비율이 `binary_threshold_pct` 이상이면 본문 대신 suppression 메시지를 반환한다.
4. `BEGIN OPENSSH PRIVATE KEY`, `-----BEGIN CERTIFICATE-----` 류 시그니처는 2차 redact 룰을 적용한다.

이 경로는 `docs/design/phase2/10-penny-permission-inheritance.md` T1/T2 위협 시나리오의 1차 방어선이다.

### 2.5 설정 스키마

```toml
[bundles.server]
credential_brokers = ["os_keychain", "env"]
default_sudo_mode = "allowlist"
service_allowlist = ["nginx", "postgresql", "vllm@*", "gadgetron-node"]
package_allowlist = ["curl", "jq", "linux-image-*", "nvidia-driver-*"]

[bundles.server.ssh]
control_persist_seconds = 60
connect_timeout_seconds = 10
default_exec_timeout_seconds = 300

[bundles.server.output_quarantine]
stdout_max_bytes = 65536
stderr_max_bytes = 32768
binary_threshold_pct = 5

[bundles.server.batch]
default_concurrency = 4
max_concurrency = 32
batch_approval_threshold = 2
cluster_wide_auto_t3 = true
```

검증 규칙:

- `credential_brokers` 는 알려진 broker 이름만 허용한다.
- `default_concurrency <= max_concurrency`.
- `batch_approval_threshold >= 2`.
- `connect_timeout_seconds <= default_exec_timeout_seconds`.
- `service_allowlist` 의 `*` 는 systemd template instance 위치에만 허용한다.

운영자 touchpoint:

- bundle enable 후 `server.add` 없이 `server.list` 가 empty 여야 한다. hidden bootstrap 는 없다.
- 설정 오류는 "무엇이 잘못됐는지 / 왜 막혔는지 / 어떻게 고칠지" 3요소를 모두 담아야 한다.
- quick-start 는 5분 안에 single host 등록, `service.status`, `log.tail`, `server.exec("uname -a")` 까지 도달해야 한다.

### 2.6 에러, 로깅, STRIDE

이 설계는 새 top-level D-13 style error variant 를 요구하지 않는다. v1 은 기존 Gadgetron error families 를 재사용하고, gadget wire surface 에서는 다음 stable `code` 문자열을 우선한다.

| code | 의미 |
|---|---|
| `host_not_found` | selector 가 빈 결과 또는 존재하지 않는 host 를 가리킴 |
| `approval_required` | request 가 ask/T3 경로인데 아직 승인되지 않음 |
| `permission_denied` | caller role/team 이 target cluster 또는 action 을 허용받지 않음 |
| `command_not_allowlisted` | sudoers allowlist 또는 bundle allowlist 에서 거부됨 |
| `ssh_connect_failed` | OpenSSH 연결 또는 host key 검증 실패 |
| `ssh_timeout` | remote command timeout |
| `binary_output_suppressed` | 출력이 binary 혹은 too large |

필수 tracing span:

- `server.selector.resolve`
  fields: `request_id`, `selector_kind`, `resolved_hosts`
- `server.approval.check`
  fields: `request_id`, `tier`, `target_count`, `policy_decision`
- `server.ssh.exec`
  fields: `request_id`, `host_id`, `ssh_session_id`, `sudo`
- `server.audit.persist`
  fields: `request_id`, `host_id`, `actor_user_id`

STRIDE 요약:

| 위협 | 자산 | 완화 |
|---|---|---|
| Spoofing | target host identity | strict known_hosts, host key mismatch hard fail |
| Tampering | command / audit trail | OpenSSH transport + append-only audit row |
| Repudiation | 누가 실행했는지 | `actor_user_id`, `tenant_id`, `policy_decision`, `request_id` 저장 |
| Information disclosure | SSH key, sudo policy, secret output | broker isolation, `!Serialize`, output quarantine, redaction |
| DoS | gateway, sshd, target host | concurrency caps, abort-on-failure, timeout, quota |
| Elevation of privilege | allowlist 밖 명령, admin-only action | strict inheritance, approval gate, sudoers allowlist |

### 2.7 의존성

추가/유지 의존성:

| 의존성 | 위치 | 이유 |
|---|---|---|
| `tokio` | bundle crate | SSH subprocess, async orchestration |
| `sqlx` | shared workspace | inventory/audit persistence |
| `nix` | bundle crate only | `memfd_create` 등 Linux FD handling |
| `zeroize` | bundle crate only | secret memory zeroization |
| `keyring` | optional feature | OS keychain broker |

비채택:

- `russh` / custom SSH stack: v1 보안 감사를 OpenSSH 로 단순화하기 위해 배제
- bundle 내부 장기 metrics DB: P2B 는 in-process ring buffer 로 제한

---

## 3. 전체 모듈 연결 구도 (Where)

### 3.1 상위/하위 의존 관계

- 상위 소비자:
  - `gadgetron-gateway` / `gadgetron-penny` 가 Server Bundle Gadgets 를 호출
  - `gadgetron-web` 과 CLI 가 동일 gadget/contracts 를 operator workflow 에 재사용
- 직접 의존:
  - `gadgetron-core`: `Bundle`, `BundleContext`, `EntityTree`, `ApprovalGate`, `AuthenticatedContext`
  - `gadgetron-knowledge`: cluster page frontmatter, seed page import, runbook/wiki 연동
  - `gadgetron-xaas`: audit persistence, actor identity, default tenant model
- 하위 외부 시스템:
  - OpenSSH client
  - target Linux host
  - local OS keychain / env vars

### 3.2 데이터 흐름 다이어그램

```text
User / Team / Admin
        |
        v
gateway session or API key
        |
        v
Penny / Web / CLI
        |
        v
GadgetRegistry -> Server Bundle Gadgets
        |
        +--> knowledge cluster page lookup
        |
        +--> approval gate
        |
        +--> credential broker -> host connector -> target host
        |
        +--> output quarantine
        |
        +--> xaas audit sink
```

### 3.3 타 도메인과의 인터페이스 계약

| 도메인 | 계약 |
|---|---|
| `gadgetron-knowledge` | cluster definition, runbook seed pages, incident/wiki writeback. host selector 자체는 knowledge table 을 직접 읽지 않고 API/trait 경유 |
| `gadgetron-penny` | gadget 호출자일 뿐 credential material 을 보지 않는다. `AuthenticatedContext` 와 `untrusted_output` contract 를 반드시 준수 |
| `gadgetron-xaas` | actor identity, audit schema, single-tenant `"default"` model 제공 |
| future `bundle-gpu` | parent=`host` child entity 를 통해 topology 확장. Server Bundle 은 child 구현 상세를 모른다 |
| future `bundle-ai-infra` | remote engine control 에 Server Bundle 실행/파일/log surface 를 재사용 |

### 3.4 D-12 크레이트 경계 준수

- SSH, sudo, command classification, output quarantine 는 **bundle crate** 에 머문다. `gadgetron-core` 에 OpenSSH-specific 로직을 밀어 넣지 않는다.
- `EntityRef`, `EntityKindSpec`, `ApprovalGate`, `AuthenticatedContext` 같은 cross-cutting 타입만 core 로 올린다.
- `gadgetron-knowledge` 는 cluster truth source 이지만 host inventory owner 는 아니다.
- `gadgetron-xaas` 는 audit/identity owner 이지만 command execution owner 는 아니다.

권장 bundle 구조:

```text
bundles/server/
├── src/
│   ├── lib.rs
│   ├── inventory/
│   ├── connector/
│   ├── credentials/
│   ├── sudo/
│   ├── exec/
│   ├── logs/
│   ├── monitoring/
│   ├── output/
│   ├── audit/
│   └── gadgets/
└── seed_pages/
```

---

## 4. 단위 테스트 계획 (Verify)

### 4.1 테스트 범위

| 대상 | 검증 invariant |
|---|---|
| `HostSelector` 해석기 | `ids`, `labels`, `cluster` 조합이 deterministic 하게 host 집합으로 환원된다 |
| destructive classifier | regex hit, allowlist category hit, cluster-wide hit 중 하나라도 있으면 T3 로 승격된다 |
| approval policy mapper | T1/T2/T3 와 `batch_approval_threshold` 규칙이 정확히 적용된다 |
| output quarantine | truncate, binary suppression, secret signature redaction, `untrusted_output` wrapper 가 모두 적용된다 |
| config validation | invalid broker name, concurrency inversion, threshold < 2 를 startup 에서 거부한다 |
| credential broker dispatch | Env/OsKeychain fallback 순서가 예측 가능하며 secret debug 노출이 없다 |
| entity projection | host row 가 stable `EntityRef { plugin: "server", kind: "host", id }` 로 투영된다 |

### 4.2 테스트 하네스

- fake inventory repository
- fake knowledge cluster resolver
- fake approval gate
- fake host connector returning scripted stdout/stderr/exit_code
- property-based input generator for selector / batch policy / destructive classification

원칙:

- mock 보다는 fake 를 우선한다.
- wall-clock 의존 timeout 테스트는 `tokio::time::pause()` 를 사용한다.
- secret-bearing 타입은 snapshot 테스트에 raw value 가 남지 않도록 redact-aware debug assertion 을 쓴다.

### 4.3 커버리지 목표

- `bundles/server` line coverage 85% 이상
- `connector`, `credentials`, `output`, `approval` 모듈 branch coverage 80% 이상
- property-based tests: selector/classifier 각각 CI 1024 cases

---

## 5. 통합 테스트 계획 (Integrate)

### 5.1 통합 범위

함께 테스트할 구성:

- future `bundles/server`
- `gadgetron-core`
- `gadgetron-knowledge`
- `gadgetron-xaas`
- `gadgetron-penny` path 의 inheritance harness

핵심 시나리오:

1. single host 등록 -> `service.status` -> `server.exec("uname -a")`
2. cluster wiki page 기반 selector -> rolling restart -> health check 통과
3. member user 가 destructive cluster exec 시도 -> approval denied -> 실행 없음
4. allowlist 밖 명령 -> approval 이후에도 OS-level deny
5. malicious log output -> Penny follow-up privileged read 차단
6. audit row 에 `tenant_id`, `actor_user_id`, `policy_decision`, `ssh_session_id` 가 모두 기록

### 5.2 테스트 환경

- Postgres 는 testcontainers 로 올린다.
- SSH 는 실제 OS host 대신 PTY 기반 fake target 또는 containerized OpenSSH target 을 쓴다.
- knowledge 는 in-memory wiki fixture 또는 temporary git-backed fixture 로 cluster page 를 공급한다.
- Penny inheritance 는 `docs/design/phase2/10-penny-permission-inheritance.md` 의 fake Claude subprocess harness 를 재사용한다.

### 5.3 회귀 방지

다음 변경은 반드시 이 테스트를 실패시켜야 한다.

- `AuthenticatedContext` 없이 gadget 실행이 가능해짐
- cluster selector 가 knowledge lookup 을 우회하고 inventory 내부 상태만으로 결정됨
- `untrusted_output` wrapper 또는 truncate/redaction 이 빠짐
- audit row 에 actor identity 가 기록되지 않음
- `cluster_wide_auto_t3` 가 무시되어 destructive batch 가 T2 로 떨어짐

---

## 6. Phase 구분

### 6.1 [P2B-alpha]

- single-tenant multi-user (`tenant_id = "default"`)
- `EnvBroker`, `OsKeychainBroker`
- OpenSSH subprocess connector + ControlMaster reuse
- host inventory + `exec_audit`
- `server.*`, `service.*`, `log.*`, `file.*`, `package.*` gadget surface
- approval gate integration
- knowledge-based cluster selector lookup
- output quarantine + audit persistence

### 6.2 [P2B-beta]

- `SystemdCredsBroker`
- richer ring-buffer metrics and operator seed/runbook set
- cluster entity auto-promotion polish on knowledge side
- CLI/Web onboarding hardening and troubleshooting docs

### 6.3 [P2C]

- Vault-backed broker
- SSH CA / short-lived cert identities
- agent-installed alternative `HostConnector`
- managed coordinator bundles (`k8s`, `slurm`, `patroni`) 와의 orchestration
- alert rule engine 및 장기 metrics 보관
- WinRM / non-POSIX host support

---

## 7. 오픈 이슈 / 의사결정 필요

현재 **P2B-alpha 진행을 막는 사용자 승인 대기 항목은 없다**. 남은 항목은 follow-up ownership 성격이다.

| ID | 내용 | 옵션 | 추천 | 상태 |
|---|---|---|---|---|
| O-1 | cluster wiki page 의 가상 entity 승격 세부 구현 위치 | A. `gadgetron-knowledge` / B. core service | **A** | 🟢 follow-up — knowledge 설계 세부화 필요 |
| O-2 | 정식 alert rule engine 위치 | A. server bundle / B. 별도 bundle / C. core service | **B** 또는 **C** | 🟢 P2C deferred |
| O-3 | WinRM / non-POSIX connector | A. P2B 포함 / B. P2C 이후 | **B** | 🟢 deferred |

---

## 리뷰 로그 (append-only)

### Round 0 — 2026-04-18 — PM draft
**결론**: Legacy `07-plugin-server.md` draft v0 를 reviewable bundle-server 문서로 재구성 시작.

**체크리스트**:
- [x] 철학/구현/연결/테스트/phase 섹션 존재
- [ ] 최신 Bundle / Plug / Gadget 용어 정합성
- [ ] explicit unit / integration test plan

### Round 1 — 2026-04-18 — @gateway-router-lead @xaas-platform-lead
**결론**: Conditional Pass

**체크리스트**:
- [x] 인터페이스 계약
- [x] 크레이트 경계
- [x] 타입 중복 방지
- [x] 에러 반환
- [x] 동시성
- [x] 의존성 방향
- [x] Phase 태그
- [x] 레거시 결정 준수

**Action Items**:
- A1: 구명칭 `plugin-server` 초안을 bundle 용어와 새 문서 경로 `07-bundle-server.md` 로 정리
- A2: D-20260418-02 D2-a 에 맞춰 inventory schema 와 narrative 를 single-tenant multi-user 기준으로 고정
- A3: unit / integration test 계획을 분리해 QA review 진입 조건을 충족

**다음 라운드 조건**: A1, A2, A3 반영

### Round 1.5 — 2026-04-18 — @security-compliance-lead @dx-product-lead
**결론**: Pass

**체크리스트**:
- [x] 위협 모델
- [x] 신뢰 경계 입력 검증
- [x] 인증·인가
- [x] 시크릿 관리
- [x] 감사 로그
- [x] 에러 정보 누출 방지
- [x] 사용자 touchpoint 워크스루
- [x] defaults 안전성
- [x] quick-start / runbook 경로
- [x] 하위 호환 고려

**Action Items**:
- 없음

### Round 2 — 2026-04-18 — @qa-test-architect
**결론**: Pass

**체크리스트**:
- [x] 단위 테스트 범위
- [x] mock/fake 가능성
- [x] 결정론
- [x] 통합 시나리오
- [x] CI 재현성
- [x] 회귀 테스트
- [x] 테스트 데이터 전략

**Action Items**:
- 없음

### Round 3 — 2026-04-18 — @chief-architect
**결론**: Pass

**체크리스트**:
- [x] Rust 관용구
- [x] 제로 비용 추상화
- [x] 트레이트 설계
- [x] 에러 전파
- [x] 의존성 추가 정당화
- [x] 관측성
- [x] D-12 크레이트 경계 준수

**Action Items**:
- 없음

### 최종 승인 — 2026-04-18 — PM
**결론**: Approved. Server Bundle 은 P2B-alpha 에서 remote host control 의 기준 설계 문서로 사용 가능하다.
