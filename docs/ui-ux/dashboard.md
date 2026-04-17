# Gadgetron 대시보드 UI/UX 설계 문서

> **프로젝트**: Gadgetron — 지식 협업 플랫폼
> **문서 버전**: 1.0.0
> **작성일**: 2026-04-11
> **작성자**: UX Lead
> **상태**: 초안 (Draft)

---

## 목차

1. [개요](#1-개요)
2. [TUI 대시보드 (Ratatui)](#2-tui-대시보드-ratatui)
3. [Web UI (React + Recharts)](#3-web-ui-react--recharts)
4. [실시간 메트릭](#4-실시간-메트릭)
5. [모니터링](#5-모니터링)
6. [하드웨어 상세 뷰](#6-하드웨어-상세-뷰)
7. [소프트웨어 상세 뷰](#7-소프트웨어-상세-뷰)
8. [부록: 컴포넌트 계층 구조](#8-부록-컴포넌트-계층-구조)

---

## 1. 개요

Gadgetron 대시보드는 클러스터 내 GPU 노드, 모델 배포, 요청 처리, 비용 현황을 실시간으로 모니터링하고 제어할 수 있는 통합 인터페이스입니다. 운영 환경의 특성에 맞춰 두 가지 인터페이스를 제공합니다.

| 인터페이스 | 기술 스택 | 용도 |
|---|---|---|
| **TUI** | Ratatui (Rust) | 터미널 기반 경량 모니터링, SSH 원격 접속 환경 |
| **Web UI** | React 19 + TypeScript + Tailwind + Recharts | 브라우저 기반 풀 기능 대시보드, 시각적 분석 |

두 인터페이스는 동일한 Axum 백엔드 API와 WebSocket 엔드포인트를 공유하며, 데이터 일관성을 보장합니다.

---

## 2. TUI 대시보드 (Ratatui)

### 2.1 전체 레이아웃

TUI 대시보드는 Ratatui 프레임워크를 기반으로 구현되며, 터미널 화면을 세 개의 영역으로 분할합니다.

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│ Gadgetron v0.2.0 │ Nodes: 4/5 │ GPUs: 16/20 │ Models: 6 │ Req/m: 142 │ 14:32 │
├──────────────────┬──────────────────────────────┬───────────────────────────────┤
│                  │                              │                               │
│    NODES         │        MODELS                │        REQUESTS               │
│                  │                              │                               │
│ ┌──────────────┐ │ ┌──────────────────────────┐ │ ┌───────────────────────────┐ │
│ │ node-01      │ │ │ Model  Engine St VRAM Nd │ │ │ 14:32:01 llama3  local   │ │
│ │ GPU 0: ▓▓░ 72│ │ │ llama3  vLLM   ●  12G 01│ │ │    42ms  128tok ✓        │ │
│ │ GPU 1: ▓░░ 45│ │ │ mixtral  trt    ●  24G 02│ │ │ 14:32:00 mixtral remote  │ │
│ │ Temp: 72°C   │ │ │ codellm  vLLM   ◐  8G 01│ │ │   189ms  256tok ✓        │ │
│ │ VRAM: 36/80G │ │ │ phi3    onnx   ●   4G 03│ │ │ 14:31:58 llama3  local   │ │
│ │ CPU: ▓▓▓░ 68%│ │ │ gemma   vLLM   ○   -  - │ │ │    38ms  512tok ✓        │ │
│ │ RAM: ▓▓░░ 48%│ │ │ qwen2   trt    ●  16G 04│ │ │ 14:31:55 codellm local   │ │
│ │ Models: 3    │ │ │                            │ │ │    92ms  64tok  ✗        │ │
│ └──────────────┘ │ │ [d] Deploy  [u] Undeploy   │ │ │                           │ │
│ ┌──────────────┐ │ └──────────────────────────┘ │ │ │ [f] Filter  [/] Search    │ │
│ │ node-02      │ │                              │ │ └───────────────────────────┘ │
│ │ GPU 0: ▓▓▓ 91│ │                              │                               │
│ │ GPU 1: ▓▓░ 67│ │                              │                               │
│ │ ...          │ │                              │                               │
│ └──────────────┘ │                              │                               │
│                  │                              │                               │
├──────────────────┴──────────────────────────────┴───────────────────────────────┤
│ METRICS  ▁▂▃▅▆▇▅▃▂▁ req/min   ▁▂▄▅▄▃▂▁ avg_lat   ▁▁▂▃▂▁▁ err   ▁▂▃▄▅ cost/hr│
├─────────────────────────────────────────────────────────────────────────────────┤
│ [q]Quit [r]Refresh [d]Deploy [u]Undeploy [↑↓]Nav [Tab]Panel [/]Search [?]Help │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### 2.2 Header (상태 표시줄)

상태 표시줄은 클러스터 전체의 핵심 지표를 한 줄로 요약합니다.

| 요소 | 설명 | 예시 |
|---|---|---|
| 버전 | Gadgetron 버전 | `Gadgetron v0.2.0` |
| 노드 수 | 온라인/전체 노드 | `Nodes: 4/5` |
| GPU 수 | 활성/전체 GPU | `GPUs: 16/20` |
| 모델 수 | 실행 중 모델 | `Models: 6` |
| 요청률 | 분당 요청 수 | `Req/m: 142` |
| 시간 | 현재 시간 | `14:32` |

### 2.3 Body — 3-Column 레이아웃

#### 2.3.1 Nodes 패널 (좌측)

각 노드는 다음 정보를 포함하는 카드 형태로 표시됩니다.

**GPU 게이지 구성:**

```
┌─ node-01 ──────────────────┐
│ GPU 0: [▓▓▓▓▓▓▓░░░] 72%    │  ← VRAM 사용률 바
│ GPU 1: [▓▓▓░░░░░░░] 45%    │
│ Temp: 72°C  ⣿⣿⣷⣦⣤⣀⣀⣀    │  ← 온도 그라데이션 표시
│ Power: 285W / 350W          │  ← 전력 사용량
│ Clock: 1.8GHz               │  ← 클럭 속도
│ VRAM: 36.2 / 80.0 GB       │  ← 전체 VRAM 사용량
│ CPU:  [▓▓▓▓▓▓▓░░░] 68%     │  ← CPU 사용률 바
│ RAM:  [▓▓▓░░░░░░░] 48%     │  ← RAM 사용률 바
│ Running: llama3, codellm   │  ← 실행 중인 모델 목록
└────────────────────────────┘
```

**온도 그라데이션 색상 규칙:**

| 온도 범위 | 색상 | 의미 |
|---|---|---|
| 0–60°C | 녹색 (Green) | 정상 |
| 60–75°C | 노란색 (Yellow) | 주의 |
| 75–85°C | 주황색 (Orange) | 경고 |
| >85°C | 빨간색 (Red) | 위험 |

**VRAM 바 색상 규칙:**

| 사용률 | 색상 |
|---|---|
| 0–70% | 녹색 |
| 70–85% | 노란색 |
| 85–95% | 주황색 |
| >95% | 빨간색 (깜빡임) |

#### 2.3.2 Models 패널 (중앙)

모델 배포 현황을 테이블 형태로 표시합니다.

```
┌─ MODELS ────────────────────────────────────────────────┐
│ Model       Engine   Status   VRAM    Node  Actions     │
│──────────────────────────────────────────────────────────│
│ llama3      vLLM     ● Run    12.0G   01    [u][r]     │
│ mixtral     trt      ● Run    24.0G   02    [u][r]     │
│ codellm     vLLM     ◐ Load    8.0G   01    [u][r]     │
│ phi3        onnx     ● Run     4.0G   03    [u][r]     │
│ gemma       vLLM     ○ Stop      -     -    [d]        │
│ qwen2       trt      ● Run    16.0G   04    [u][r]     │
│                                                          │
│ [d] Deploy  [u] Undeploy  [r] Reload  [p] Profile      │
└──────────────────────────────────────────────────────────┘
```

**모델 상태 색상 및 아이콘:**

| 상태 | 아이콘 | 색상 | 설명 |
|---|---|---|---|
| Running | `●` | 녹색 | 정상 실행 중 |
| Loading | `◐` | 노란색 | 모델 로딩 중 |
| Stopped | `○` | 회색 | 배포 중단됨 |
| Error | `✗` | 빨간색 | 오류 발생 |
| Draining | `◔` | 주황색 | 연결 종료 대기 중 |

#### 2.3.3 Requests 패널 (우측)

실시간 요청 로그를 타임스탬프와 함께 표시합니다.

```
┌─ REQUESTS ─────────────────────────────────────────────┐
│ 14:32:01 llama3  local    42ms   128tok  ✓ 200        │
│ 14:32:00 mixtral remote  189ms   256tok  ✓ 200        │
│ 14:31:58 llama3  local    38ms   512tok  ✓ 200        │
│ 14:31:55 codellm local    92ms    64tok  ✗ 500        │
│ 14:31:53 phi3    local    15ms    32tok  ✓ 200        │
│ 14:31:51 qwen2   local   210ms  1024tok  ✓ 200        │
│ 14:31:49 llama3  remote  350ms   256tok  ◐ timeout    │
│                                                         │
│ [f] Filter  [/] Search  [e] Export  [t] Trace          │
└─────────────────────────────────────────────────────────┘
```

**요청 상태 아이콘:**

| 상태 | 아이콘 | 색상 |
|---|---|---|
| 성공 (2xx) | `✓` | 녹색 |
| 클라이언트 오류 (4xx) | `⚠` | 노란색 |
| 서버 오류 (5xx) | `✗` | 빨간색 |
| 타임아웃 | `◐` | 주황색 |
| 처리 중 | `↻` | 파란색 |

### 2.4 Metrics 패널 (바닥 상단)

스파크라인 차트 4개를 수평 배치합니다. 각 차트는 최근 60개 샘플을 표시합니다.

```
METRICS
 req/min  ▁▂▃▅▆▇▅▃▂▁▂▃▅▆▇█▇▅▃▂▁▂▃▅▆▇▅▃▂▁    142
 avg_lat  ▁▂▄▅▄▃▂▁▁▂▃▄▅▆▅▄▃▂▁▁▂▃▄▅▆▅▄▃▂▁     85ms
 err_rate ▁▁▂▃▂▁▁▁▁▂▃▄▃▂▁▁▁▁▁▂▃▂▁▁▁▁▁▁▂▁      2.1%
 cost/hr  ▁▂▃▄▅▄▃▂▁▂▃▄▅▆▇▆▅▄▃▂▁▂▃▄▅▆▅▄▃     $12.4
```

### 2.5 Footer (키바인딩)

```
[q]Quit  [r]Refresh  [d]Deploy  [u]Undeploy  [↑↓]Navigate  [Tab]Switch Panel  [/]Search  [?]Help
```

**전체 키바인딩 목록:**

| 키 | 동작 | 설명 |
|---|---|---|
| `q` | Quit | 대시보드 종료 |
| `r` | Refresh | 수동 새로고침 |
| `d` | Deploy | 선택한 모델 배포 |
| `u` | Undeploy | 선택한 모델 배포 중단 |
| `↑` / `↓` | Navigate | 패널 내 항목 이동 |
| `Tab` | Switch Panel | 패널 간 포커스 전환 |
| `/` | Search | 검색 모드 진입 |
| `?` | Help | 도움말 오버레이 |
| `Enter` | Confirm | 선택 항목 상세 보기 |
| `Esc` | Cancel / Back | 현재 모드 취소 |
| `p` | Profile | 모델 프로파일링 실행 |
| `e` | Export | 요청 로그 내보내기 |
| `t` | Trace | 요청 트레이스 보기 |
| `f` | Filter | 필터 모드 진입 |

### 2.6 TUI 색상 체계

기본 다크 테마 기반 색상 정의:

```
배경:        #1a1b26 (도쿄 나이트 배경)
전경:        #a9b1d6 (기본 텍스트)
강조:        #7aa2f7 (파란색 강조)
성공:        #9ece6a (녹색)
경고:        #e0af68 (노란색)
오류:        #f7768e (빨간색)
뮤트:        #565f89 (흐린 텍스트)
패널 보더:   #414868 (패널 테두리)
선택:        #33467c (선택된 행 배경)
GPU 온도:
  정상:      #9ece6a → #e0af68 (녹색→노란색 그라데이션)
  위험:      #ff9e64 → #f7768e (주황색→빨간색 그라데이션)
```

### 2.7 TUI 컴포넌트 계층 구조

```
App
├── Header (StatusBar)
│   ├── VersionLabel
│   ├── ClusterSummary (Nodes, GPUs, Models, Req/m)
│   └── Clock
├── Body (Horizontal Split)
│   ├── NodesPanel
│   │   └── NodeCard (per node)
│   │       ├── GpuGauge (per GPU)
│   │       │   ├── VramBar
│   │       │   ├── UtilizationPercent
│   │       │   ├── TemperatureGradient
│   │       │   ├── PowerReadout
│   │       │   └── ClockSpeed
│   │       ├── CpuBar
│   │       ├── RamBar
│   │       └── RunningModelList
│   ├── ModelsPanel
│   │   ├── ModelTable
│   │   │   └── ModelRow (model, engine, status, vram, node)
│   │   └── ActionHints
│   └── RequestsPanel
│       ├── RequestLog
│       │   └── RequestRow (timestamp, model, provider, latency, tokens, status)
│       └── ActionHints
├── MetricsBar
│   ├── Sparkline (req/min)
│   ├── Sparkline (avg latency)
│   ├── Sparkline (error rate)
│   └── Sparkline (cost/hr)
└── Footer (Keybindings)
    └── KeybindingHint (per key)
```

---

## 3. Web UI (React + Recharts)

### 3.1 기술 스택

| 기술 | 버전 | 용도 |
|---|---|---|
| React | 19 | UI 프레임워크 |
| TypeScript | 5.x | 타입 안전성 |
| Tailwind CSS | 4.x | 유틸리티 퍼스트 스타일링 |
| Recharts | 2.x | 차트 라이브러리 |
| shadcn/ui | 최신 | UI 컴포넌트 라이브러리 (Radix UI 기반) |
| Zustand | 5.x | 클라이언트 상태 관리 |
| TanStack Query | 5.x | 서버 상태 관리 및 캐싱 |
| React Router | 7.x | 클라이언트 사이드 라우팅 |

Axum 서버가 정적 에셋으로 Web UI를 서빙하며, 실시간 업데이트는 WebSocket을 통해 수신합니다.

### 3.2 페이지 구성

#### 3.2.1 Dashboard (개요 페이지)

클러스터 전체 상태를 한눈에 파악할 수 있는 개요 페이지입니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron                              🔍  [사용자]  ⚙ 설정    │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ │
│  │ 클러스터  │ │  총 GPU  │ │ 실행 모델│ │  요청/분  │ │  에러율  │ │
│  │   건강    │ │   수     │ │          │ │          │ │          │ │
│  │   ● 정상  │ │   16/20  │ │    6     │ │   142    │ │   2.1%   │ │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘ │
│                                                                     │
│  ┌─────────────────────────────┐ ┌─────────────────────────────┐   │
│  │    요청률 추이 (req/min)     │ │     평균 레이턴시 (ms)      │   │
│  │  📈 라인 차트 (24h)         │ │  📈 라인 차트 (24h)         │   │
│  │                             │ │                             │   │
│  └─────────────────────────────┘ └─────────────────────────────┘   │
│                                                                     │
│  ┌─────────────────────────────┐ ┌─────────────────────────────┐   │
│  │    GPU 사용률 분포           │ │     비용 추이 ($/hr)        │   │
│  │  📊 바 차트 (노드별)         │ │  📈 에어리어 차트 (24h)     │   │
│  │                             │ │                             │   │
│  └─────────────────────────────┘ └─────────────────────────────┘   │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐   │
│  │    최근 요청 로그 (실시간 스트리밍)                         │   │
│  │  시간      모델     제공자    레이턴시  토큰   상태          │   │
│  │  14:32:01  llama3  local     42ms     128    ✓             │   │
│  │  14:32:00  mixtral remote   189ms     256    ✓             │   │
│  │  ...                                                        │   │
│  └─────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  ┌──────────────────┐ ┌──────────────────────────────────────┐    │
│  │   알림           │ │    노드 상태 요약                    │    │
│  │  ⚠ node-03 온도  │ │  node-01 ●  node-02 ●  node-03 ⚠   │    │
│  │    85°C 초과      │ │  node-04 ●  node-05 ○              │    │
│  └──────────────────┘ └──────────────────────────────────────┘    │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

**주요 컴포넌트:**

- `ClusterHealthCard`: 클러스터 상태 표시 (정상/경고/오류)
- `StatCard`: 핵심 지표 카드 (GPU 수, 모델 수, 요청률, 에러율, 비용)
- `RequestsOverTimeChart`: 요청률 시계열 라인 차트 (Recharts `<LineChart>`)
- `LatencyChart`: 평균 레이턴시 시계열 라인 차트
- `GpuUtilizationBarChart`: 노드별 GPU 사용률 바 차트
- `CostTrendAreaChart`: 비용 추이 에어리어 차트
- `LiveRequestLog`: 실시간 요청 스트리밍 테이블 (WebSocket 구독)
- `AlertsPanel`: 활성 알림 목록
- `NodeStatusGrid`: 노드 상태 요약 그리드

#### 3.2.2 Nodes (노드 페이지)

개별 노드 상세 정보와 GPU 토폴로지를 확인합니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron > Nodes                                                │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌─────────────┐  │
│  │  node-01 ●  │ │  node-02 ●  │ │  node-03 ⚠  │ │  node-04 ●  │  │
│  │  4x A100    │ │  4x H100    │ │  4x A100    │ │  4x A100    │  │
│  │  VRAM 72%   │ │  VRAM 58%   │ │  VRAM 91%   │ │  VRAM 45%   │  │
│  │  CPU  68%   │ │  CPU  42%   │ │  CPU  89%   │ │  CPU  31%   │  │
│  └─────────────┘ └─────────────┘ └─────────────┘ └─────────────┘  │
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  GPU 토폴로지 (node-01)                                      │  │
│  │                                                               │  │
│  │   [GPU 0]──NVLink──[GPU 1]                                   │  │
│  │      │                 │                                      │  │
│  │   NVLink            NVLink                                    │  │
│  │      │                 │                                      │  │
│  │   [GPU 2]──NVLink──[GPU 3]                                   │  │
│  │                                                               │  │
│  │   NUMA 0: GPU 0, GPU 1    NUMA 1: GPU 2, GPU 3              │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  열 지도 (Thermal Heatmap)                                   │  │
│  │                                                               │  │
│  │        GPU0   GPU1   GPU2   GPU3                              │  │
│  │  N01   🟢62   🟡71   🟢58   🟢55                             │  │
│  │  N02   🟢55   🟢52   🟡68   🟢49                             │  │
│  │  N03   🟠78   🔴86   🟠79   🟡73                             │  │
│  │  N04   🟢48   🟢51   🟢45   🟢42                             │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │  전력 소비 차트 (실시간)                                      │  │
│  │  📈 라인 차트: GPU별 + 클러스터 총합                          │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

#### 3.2.3 Models (모델 페이지)

모델 카탈로그, 배포/배포 중단 제어, 버전 관리, 프로파일링 결과를 제공합니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron > Models                     [+ 새 모델 배포]          │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  [전체] [실행 중] [중단됨] [오류]                    🔍 검색       │
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ 모델      엔진   버전    상태     VRAM   노드   레이턴시      │  │
│  │───────────────────────────────────────────────────────────────│  │
│  │ llama3   vLLM   v2.1    ● 실행   12G    01     42ms         │  │
│  │ mixtral  trt     v1.3    ● 실행   24G    02     189ms        │  │
│  │ codellm  vLLM   v1.0    ◐ 로딩    8G    01     -            │  │
│  │ phi3     onnx    v3.2    ● 실행    4G    03     15ms         │  │
│  │ gemma    vLLM   v1.0    ○ 중단     -      -     -            │  │
│  │ qwen2    trt     v2.0    ● 실행   16G    04     210ms        │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 모델 상세 (llama3) ─────────────────────────────────────────┐  │
│  │  버전 관리:  v2.1 (현재)  │  v2.0  │  v1.9                   │  │
│  │  프로파일링:                                                │  │
│  │    프리필 레이턴시: 18ms   디코드 레이턴시: 24ms/tok         │  │
│  │    처리량: 128 tok/s       최대 배치: 32                     │  │
│  │    메모리: 11.2 GB         최적 배치: 16                     │  │
│  │  [배포 중단] [재시작] [프로파일링 실행] [버전 전환]         │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

#### 3.2.4 Routing (라우팅 페이지)

라우팅 전략 설정, 폴백 체인, 제공자 상태, 비용 비교를 관리합니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron > Routing                                              │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌─ 라우팅 전략 ────────────────────────────────────────────────┐  │
│  │  모델: llama3                                                 │  │
│  │  전략: [가중 라운드로빈 ▾]                                   │  │
│  │  기준: 레이턴시 우선 | 비용 우선 | 품질 우선 | 균형          │  │
│  │                                                               │  │
│  │  폴백 체인:                                                   │  │
│  │  local-vllm → local-trt → remote-openai → remote-anthropic   │  │
│  │                                                               │  │
│  │  제공자 가중치:                                               │  │
│  │  local-vllm: ████████░░ 80%                                  │  │
│  │  local-trt:   ██████░░░░ 60%                                 │  │
│  │  remote-openai: ████░░░░░░ 40%                               │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 제공자 상태 ────────────────────────────────────────────────┐  │
│  │  제공자        상태    레이턴시   가용성   큐 깊이            │  │
│  │  local-vllm    ● 정상   42ms     99.9%    3                  │  │
│  │  local-trt     ● 정상   38ms     99.5%    1                  │  │
│  │  remote-openai ● 정상  120ms     99.0%    0                  │  │
│  │  remote-anthro ◐ 지연   250ms    95.0%    7                  │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 비용 비교 ──────────────────────────────────────────────────┐  │
│  │  제공자        $/1K 토큰(입력)  $/1K 토큰(출력)  월 예상    │  │
│  │  local-vllm    $0.00 (자가호스팅)  $0.00            $0      │  │
│  │  local-trt     $0.00 (자가호스팅)  $0.00            $0      │  │
│  │  remote-openai $0.005             $0.015            $342     │  │
│  │  remote-anthro $0.003             $0.015            $285     │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

#### 3.2.5 Requests (요청 페이지)

실시간 요청 스트림, 필터 가능한 히스토리, 레이턴시 분포 차트를 제공합니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron > Requests                                             │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  [전체] [성공] [오류] [타임아웃]     🔍    📅 기간 선택    📥 내보내기│
│                                                                     │
│  ┌─ 실시간 요청 스트림 ─────────────────────────────────────────┐  │
│  │  시간        모델     제공자       레이턴시  토큰   상태     │  │
│  │  14:32:01.234 llama3  local-vllm   42ms     128    ✓ 200   │  │
│  │  14:32:00.891 mixtral remote-oai  189ms     256    ✓ 200   │  │
│  │  14:31:58.456 llama3  local-vllm   38ms     512    ✓ 200   │  │
│  │  14:31:55.123 codellm local-vllm   92ms      64    ✗ 500   │  │
│  │  ... (자동 스크롤)                                            │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 레이턴시 분포 ──────────────────────────────────────────────┐  │
│  │  📊 히스토그램 (p50: 45ms, p95: 180ms, p99: 350ms)          │  │
│  │                                                               │  │
│  │  ▏  0-50ms  ████████████████████████████  58%               │  │
│  │  ▏ 50-100ms ███████████████               24%               │  │
│  │  ▏ 100-200ms █████████                     12%               │  │
│  │  ▏ 200-500ms ███                            4%               │  │
│  │  ▏ 500ms+    █                              2%               │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 요청 트레이스 (선택 시) ────────────────────────────────────┐  │
│  │  요청 ID: req-abc123                                         │  │
│  │  ┌─ 라우팅 결정: local-vllm 선택 (레이턴시 우선 전략)       │  │
│  │  ├─ 큐 대기: 3ms                                            │  │
│  │  ├─ 프리필: 18ms                                            │  │
│  │  ├─ 디코드: 24ms (128 토큰)                                │  │
│  │  ├─ 후처리: 2ms                                             │  │
│  │  └─ 총 레이턴시: 47ms                                       │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

#### 3.2.6 Agents (에이전트 페이지 — 향후 구현)

에이전트 목록, 상태, 로그, 도구 호출 내역을 표시합니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron > Agents (Beta)                                        │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │ 에이전트       상태     모델      도구    실행 시간  요청     │  │
│  │───────────────────────────────────────────────────────────────│  │
│  │ code-assistant ● 실행   llama3    4개    2h 15m    1,247     │  │
│  │ data-analyst   ◐ 대기   mixtral   3개    45m       312       │  │
│  │ doc-writer     ○ 중단   phi3      2개    -         0         │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 에이전트 상세 (code-assistant) ──────────────────────────────┐  │
│  │  상태: ● 실행 중                                             │  │
│  │  모델: llama3 (local-vllm)                                   │  │
│  │  도구: file_read, file_write, shell_exec, web_search         │  │
│  │                                                               │  │
│  │  ┌─ 최근 로그 ─────────────────────────────────────────┐     │  │
│  │  │ 14:32 [tool_call] file_read("src/main.rs")          │     │  │
│  │  │ 14:32 [response] 파일 내용을 분석했습니다...        │     │  │
│  │  │ 14:31 [tool_call] shell_exec("cargo check")         │     │  │
│  │  │ 14:31 [response] 컴파일 성공                        │     │  │
│  │  └─────────────────────────────────────────────────────┘     │  │
│  │                                                               │  │
│  │  [중지] [재시작] [로그 다운로드]                            │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

#### 3.2.7 Settings (설정 페이지)

제공자 설정, API 키, 할당량 관리, 노드 등록을 관리합니다.

**와이어프레임 설명:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ◉ Gadgetron > Settings                                             │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│  [일반] [제공자] [API 키] [할당량] [노드] [알림]                  │
│                                                                     │
│  ┌─ 제공자 설정 ────────────────────────────────────────────────┐  │
│  │                                                               │  │
│  │  로컬 제공자:                                                │  │
│  │  ┌ vLLM ──────────────────────────────────────────────────┐  │  │
│  │  │ 엔드포인트: http://localhost:8000                       │  │  │
│  │  │ 최대 동시 요청: 32                                     │  │  │
│  │  │ 타임아웃: 30s                                          │  │  │
│  │  │ 상태: ● 연결됨                                         │  │  │
│  │  └────────────────────────────────────────────────────────┘  │  │
│  │  ┌ TensorRT ──────────────────────────────────────────────┐  │  │
│  │  │ 엔드포인트: http://localhost:8001                       │  │  │
│  │  │ 최대 동시 요청: 16                                     │  │  │
│  │  │ 타임아웃: 60s                                          │  │  │
│  │  │ 상태: ● 연결됨                                         │  │  │
│  │  └────────────────────────────────────────────────────────┘  │  │
│  │                                                               │  │
│  │  원격 제공자:                                                │  │
│  │  ┌ OpenAI ────────────────────────────────────────────────┐  │  │
│  │  │ API 키: sk-••••••••••••••••                             │  │  │
│  │  │ 조직 ID: org-••••••                                     │  │  │
│  │  │ 상태: ● 연결됨                                         │  │  │
│  │  └────────────────────────────────────────────────────────┘  │  │
│  │                                                               │  │
│  │  [+ 제공자 추가]                                             │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
│  ┌─ 알림 임계값 ────────────────────────────────────────────────┐  │
│  │  GPU 온도 > 85°C          [켜짐]  → Slack, Email            │  │
│  │  에러율 > 5%              [켜짐]  → Slack                    │  │
│  │  VRAM 사용률 > 95%        [켜짐]  → Email                    │  │
│  │  노드 오프라인             [켜짐]  → Slack, Email, PagerDuty │  │
│  │  비용 > 일일 한도          [꺼짐]                              │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

### 3.3 반응형 디자인

| 브레이크포인트 | 너비 | 레이아웃 | 설명 |
|---|---|---|---|
| 데스크톱 | >= 1280px | 풀 대시보드 | 모든 패널 표시, 4열 카드 그리드 |
| 태블릿 | 768–1279px | 2열 레이아웃 | 카드 2열, 차트 2열 스택 |
| 모바일 | < 768px | 단일 열 | 카드/차트 세로 스택, 접을 수 있는 패널 |

**반응형 동작:**

- 데스크톱: 사이드바 내비게이션 + 콘텐츠 영역 2-3열 그리드
- 태블릿: 상단 내비게이션 + 2열 카드 그리드, 차트 세로 스택
- 모바일: 하단 내비게이션 바 + 단일 열 스크롤, 차트 전체 너비

### 3.4 테마

- **기본 테마**: 다크 테마 (TUI와 색상 일관성 유지)
- **라이트 테마**: 토글 가능 (사용자 설정에 저장)
- 테마 전환은 CSS 변수 기반으로 구현하며, `prefers-color-scheme` 미디어 쿼리를 존중합니다.

**다크 테마 색상:**

```css
:root[data-theme="dark"] {
  --bg-primary:    #0f1117;
  --bg-secondary:  #1a1b26;
  --bg-card:       #1e2030;
  --border:        #414868;
  --text-primary:  #c0caf5;
  --text-secondary:#a9b1d6;
  --text-muted:    #565f89;
  --accent:        #7aa2f7;
  --success:       #9ece6a;
  --warning:       #e0af68;
  --error:         #f7768e;
  --info:          #7dcfff;
}
```

**라이트 테마 색상:**

```css
:root[data-theme="light"] {
  --bg-primary:    #ffffff;
  --bg-secondary:  #f5f5f5;
  --bg-card:       #ffffff;
  --border:        #e0e0e0;
  --text-primary:  #1a1b26;
  --text-secondary:#414868;
  --text-muted:    #9ca3af;
  --accent:        #3b82f6;
  --success:       #22c55e;
  --warning:       #f59e0b;
  --error:         #ef4444;
  --info:          #06b6d4;
}
```

### 3.5 Web UI 컴포넌트 계층 구조

```
App
├── Layout
│   ├── Sidebar (데스크톱) / BottomNav (모바일)
│   │   ├── NavItem (Dashboard, Nodes, Models, Routing, Requests, Agents, Settings)
│   │   ├── ThemeToggle
│   │   └── UserProfile
│   ├── TopBar
│   │   ├── SearchBar
│   │   ├── NotificationBell
│   │   └── UserMenu
│   └── MainContent
│       ├── DashboardPage
│       │   ├── StatCardGrid
│       │   │   └── StatCard (cluster health, GPU count, model count, req/min, error rate, cost)
│       │   ├── ChartGrid
│       │   │   ├── RequestsOverTimeChart (Recharts LineChart)
│       │   │   ├── LatencyChart (Recharts LineChart)
│       │   │   ├── GpuUtilizationBarChart (Recharts BarChart)
│       │   │   └── CostTrendAreaChart (Recharts AreaChart)
│       │   ├── LiveRequestLog (WebSocket)
│       │   ├── AlertsPanel
│       │   └── NodeStatusGrid
│       ├── NodesPage
│       │   ├── NodeCardGrid
│       │   │   └── NodeCard
│       │   │       ├── GpuGauge
│       │   │       ├── CpuBar
│       │   │       ├── RamBar
│       │   │       └── ModelList
│       │   ├── GpuTopologyDiagram
│       │   │   ├── GpuNode
│       │   │   ├── NvLinkConnection
│       │   │   └── NumaZone
│       │   ├── ThermalHeatmap
│       │   │   └── HeatmapCell
│       │   └── PowerChart (Recharts LineChart)
│       ├── ModelsPage
│       │   ├── ModelFilterBar
│       │   ├── ModelTable
│       │   │   └── ModelRow
│       │   ├── ModelDetailPanel
│       │   │   ├── VersionManager
│       │   │   ├── ProfilingResults
│       │   │   └── ActionButtons (Deploy, Undeploy, Restart, Profile)
│       │   └── DeployDialog (shadcn/ui Dialog)
│       ├── RoutingPage
│       │   ├── StrategyConfig
│       │   │   ├── StrategySelector
│       │   │   ├── FallbackChainEditor
│       │   │   └── WeightSliders
│       │   ├── ProviderHealthTable
│       │   │   └── ProviderHealthRow
│       │   └── CostComparisonTable
│       │       └── CostComparisonRow
│       ├── RequestsPage
│       │   ├── RequestFilterBar
│       │   ├── LiveRequestStream (WebSocket)
│       │   │   └── RequestRow
│       │   ├── LatencyHistogram (Recharts BarChart)
│       │   └── RequestTracePanel
│       │       ├── TraceTimeline
│       │       └── TraceStep
│       ├── AgentsPage (향후)
│       │   ├── AgentTable
│       │   │   └── AgentRow
│       │   ├── AgentDetailPanel
│       │   │   ├── AgentLog
│       │   │   ├── ToolCallList
│       │   │   └── ActionButtons
│       │   └── AgentCreateDialog
│       └── SettingsPage
│           ├── GeneralSettings
│           ├── ProviderSettings
│           │   ├── ProviderCard (vLLM, TensorRT, OpenAI, Anthropic)
│           │   └── AddProviderDialog
│           ├── ApiKeyManager
│           ├── QuotaManager
│           ├── NodeRegistration
│           └── AlertThresholdConfig
│               └── AlertRuleRow
└── WebSocketProvider (Zustand store + React context)
    ├── useGpuMetrics hook
    ├── useModelStatus hook
    ├── useRequestLog hook
    └── useClusterHealth hook
```

---

## 4. 실시간 메트릭

### 4.1 WebSocket 엔드포인트

| 항목 | 값 |
|---|---|
| 엔드포인트 | `ws://{host}/api/v1/ws/metrics` |
| 프로토콜 | JSON over WebSocket |
| 인증 | JWT 토큰 (쿼리 파라미터 또는 헤더) |

### 4.2 메시지 유형

서버가 클라이언트로 전송하는 메시지 유형:

#### 4.2.1 `gpu_metrics`

GPU 메트릭 업데이트. 모든 노드의 모든 GPU에 대해 전송됩니다.

```json
{
  "type": "gpu_metrics",
  "timestamp": "2026-04-11T14:32:01.234Z",
  "data": {
    "node_id": "node-01",
    "gpus": [
      {
        "index": 0,
        "vram_used_mb": 36864,
        "vram_total_mb": 81920,
        "utilization_pct": 72,
        "temperature_c": 72,
        "power_w": 285,
        "power_limit_w": 350,
        "clock_mhz": 1800,
        "fan_rpm": 2400
      },
      {
        "index": 1,
        "vram_used_mb": 22528,
        "vram_total_mb": 81920,
        "utilization_pct": 45,
        "temperature_c": 58,
        "power_w": 190,
        "power_limit_w": 350,
        "clock_mhz": 1600,
        "fan_rpm": 1800
      }
    ],
    "cpu_pct": 68,
    "ram_used_gb": 62.4,
    "ram_total_gb": 128.0,
    "network_rx_mbps": 450,
    "network_tx_mbps": 320
  }
}
```

**푸시 주기**: 1초

#### 4.2.2 `model_status`

모델 배포 상태 변경 시 전송됩니다.

```json
{
  "type": "model_status",
  "timestamp": "2026-04-11T14:32:01.234Z",
  "data": {
    "model_id": "llama3",
    "status": "running",
    "engine": "vLLM",
    "version": "v2.1",
    "node_id": "node-01",
    "vram_used_mb": 12288,
    "loaded_at": "2026-04-11T10:00:00Z"
  }
}
```

**푸시 주기**: 상태 변경 시 즉시 (이벤트 기반)

#### 4.2.3 `request_log`

개별 요청 완료 시 전송됩니다.

```json
{
  "type": "request_log",
  "timestamp": "2026-04-11T14:32:01.234Z",
  "data": {
    "request_id": "req-abc123",
    "model": "llama3",
    "provider": "local-vllm",
    "status": 200,
    "latency_ms": 42,
    "prompt_tokens": 24,
    "completion_tokens": 128,
    "total_tokens": 152,
    "routing_decision": {
      "strategy": "latency_priority",
      "selected_provider": "local-vllm",
      "fallback_level": 0
    }
  }
}
```

**푸시 주기**: 요청 완료 시 즉시 (최대 100ms 버퍼링)

#### 4.2.4 `cluster_health`

클러스터 전체 건강 상태 요약입니다.

```json
{
  "type": "cluster_health",
  "timestamp": "2026-04-11T14:32:01.234Z",
  "data": {
    "status": "healthy",
    "total_nodes": 5,
    "online_nodes": 4,
    "total_gpus": 20,
    "active_gpus": 16,
    "running_models": 6,
    "requests_per_minute": 142,
    "error_rate_pct": 2.1,
    "cost_per_hour_usd": 12.40,
    "alerts": [
      {
        "level": "warning",
        "message": "node-03 GPU 온도 86°C 초과",
        "timestamp": "2026-04-11T14:31:50Z"
      }
    ]
  }
}
```

**푸시 주기**: 5초

### 4.3 클라이언트 사이드 버퍼링 및 차트 애니메이션

| 메시지 유형 | 버퍼 전략 | 애니메이션 |
|---|---|---|
| `gpu_metrics` | 1초 샘플, 최근 300개 유지 | Recharts `isAnimationActive={true}`, 300ms 트랜지션 |
| `model_status` | 즉시 반영, 상태 배열 업데이트 | 행 추가/제거 시 200ms 페이드 |
| `request_log` | 100ms 배치, 최대 50개/초, 최근 500개 유지 | 스트리밍 테이블 자동 스크롤 |
| `cluster_health` | 5초 샘플, 최근 60개 유지 | 카드 값 트랜지션 200ms |

**Zustand 스토어 구조:**

```typescript
interface MetricsStore {
  // GPU 메트릭
  gpuMetrics: Map<string, GpuMetrics[]>;     // nodeId → 최근 300개 샘플
  // 모델 상태
  modelStatuses: Map<string, ModelStatus>;    // modelId → 현재 상태
  // 요청 로그
  requestLog: RequestEntry[];                 // 최근 500개
  // 클러스터 건강
  clusterHealth: ClusterHealth | null;
  // 구독 관리
  connect: () => void;
  disconnect: () => void;
}
```

---

## 5. 모니터링

### 5.1 하드웨어 모니터링

GPU, CPU, RAM, 네트워크, 디스크, 인터커넥트 대역폭을 노드별로 수집합니다.

| 메트릭 | 단위 | 수집 주기 | 설명 |
|---|---|---|---|
| GPU VRAM 사용량 | MB | 1s | 전체 VRAM 대비 사용량 |
| GPU 연산 활용률 | % | 1s | SM 활용률 |
| GPU 온도 | °C | 1s | 다이 온도 |
| GPU 전력 소비 | W | 1s | 현재 전력 / 전력 제한 |
| GPU 클럭 속도 | MHz | 1s | 코어/메모리 클럭 |
| GPU 팬 RPM | RPM | 1s | 쿨링 팬 속도 |
| CPU 사용률 | % | 1s | 전체 코어 평균 |
| RAM 사용량 | GB | 1s | 사용 / 전체 |
| 네트워크 I/O | Mbps | 1s | RX/TX 대역폭 |
| 디스크 I/O | MB/s | 5s | 읽기/쓰기 속도 |
| 인터커넥트 대역폭 | GB/s | 5s | NVLink/PCIe 대역폭 |

### 5.2 소프트웨어 모니터링

요청 처리 성능, 모델 처리량, 큐 상태를 추적합니다.

| 메트릭 | 단위 | 수집 주기 | 설명 |
|---|---|---|---|
| 요청률 | req/min | 1s (집계) | 분당 요청 수 |
| 레이턴시 p50 | ms | 10s | 중앙값 응답 시간 |
| 레이턴시 p95 | ms | 10s | 95퍼센타일 응답 시간 |
| 레이턴시 p99 | ms | 10s | 99퍼센타일 응답 시간 |
| 에러율 | % | 10s | 실패한 요청 비율 |
| 토큰 처리량 | tok/s | 1s | 초당 생성 토큰 수 |
| 모델 로드 시간 | s | 이벤트 | 모델 로딩 완료 시간 |
| 큐 깊이 | 개 | 1s | 대기 중인 요청 수 |
| 배치 크기 | 개 | 1s | 현재 처리 중인 배치 크기 |
| 프리필 레이턴시 | ms | 10s | 프리필 단계 레이턴시 |
| 디코드 레이턴시 | ms/tok | 10s | 토큰당 디코드 레이턴시 |

### 5.3 비즈니스 모니터링

비용, 효율성, 사용 패턴을 추적합니다.

| 메트릭 | 단위 | 수집 주기 | 설명 |
|---|---|---|---|
| 모델당 비용 | $/hr | 1min | 각 모델의 시간당 운영 비용 |
| 테넌트당 비용 | $/day | 1min | 테넌트별 일일 비용 |
| GPU 활용 효율 | % | 5min | 실제 연산 사용률 / 할당률 |
| 모델 인기 순위 | - | 1hr | 요청 수 기준 모델 순위 |
| 토큰당 비용 | $/1K tok | 1min | 1,000 토큰당 평균 비용 |
| 자가호스팅 절감액 | $/day | 1day | 원격 API 대비 절감 비용 |

### 5.4 알림 임계값

모든 메트릭에 대해 사용자 정의 알림 임계값을 설정할 수 있습니다.

**기본 임계값:**

| 메트릭 | 경고 | 위험 | 알림 채널 |
|---|---|---|---|
| GPU 온도 | > 75°C | > 85°C | Slack, Email |
| GPU VRAM 사용률 | > 85% | > 95% | Email |
| 에러율 | > 3% | > 5% | Slack |
| 레이턴시 p99 | > 500ms | > 1000ms | Slack |
| 노드 오프라인 | - | 즉시 | Slack, Email, PagerDuty |
| 디스크 사용률 | > 80% | > 90% | Email |
| 큐 깊이 | > 50 | > 100 | Slack |
| 비용 일일 한도 | > 80% | > 100% | Email |

**알림 임계값 설정 API:**

```json
POST /api/v1/alerts/rules
{
  "metric": "gpu_temperature",
  "condition": "gt",
  "warning_threshold": 75,
  "critical_threshold": 85,
  "duration_seconds": 60,
  "channels": ["slack", "email"],
  "enabled": true
}
```

### 5.5 외부 메트릭 내보내기

| 엔드포인트 | 형식 | 용도 |
|---|---|---|
| `/metrics` | Prometheus exposition format | Prometheus/Grafana 연동 |
| `/api/v1/traces` | OpenTelemetry OTLP | 분산 트레이싱 (Jaeger, Zipkin) |
| `/api/v1/health` | JSON | 헬스 체크 |

**Prometheus 메트릭 예시:**

```
# GPU 메트릭
gadgetron_gpu_vram_used_bytes{node="node-01",gpu="0"} 38654705664
gadgetron_gpu_vram_total_bytes{node="node-01",gpu="0"} 85899345920
gadgetron_gpu_utilization_percent{node="node-01",gpu="0"} 72
gadgetron_gpu_temperature_celsius{node="node-01",gpu="0"} 72
gadgetron_gpu_power_watts{node="node-01",gpu="0"} 285

# 요청 메트릭
gadgetron_request_duration_seconds_bucket{model="llama3",provider="local-vllm",le="0.05"} 580
gadgetron_request_duration_seconds_bucket{model="llama3",provider="local-vllm",le="0.1"} 820
gadgetron_request_total{model="llama3",provider="local-vllm",status="200"} 1247
gadgetron_request_total{model="llama3",provider="local-vllm",status="500"} 3

# 비용 메트릭
gadgetron_cost_dollars_total{model="llama3",tenant="default"} 4.20
```

---

## 6. 하드웨어 상세 뷰

### 6.1 GPU 상세 카드

각 GPU에 대한 상세 정보 카드입니다.

**TUI 뷰 (노드 상세 진입 시):**

```
┌─ GPU 0: NVIDIA A100-SXM4-80GB (node-01) ──────────────────────┐
│                                                                 │
│  VRAM:     [▓▓▓▓▓▓▓░░░░░░░] 72%  (58.2 / 80.0 GB)           │
│  연산률:   [▓▓▓▓▓▓▓░░░░░░░] 72%                               │
│  온도:     [▓▓▓▓▓▓▓▓░░░░░░] 72°C   ⣿⣿⣷⣦⣤⣀⣀⣀         │
│  전력:     [▓▓▓▓▓▓▓▓░░░░░░] 285W / 350W                       │
│  클럭:     1800 MHz (코어) / 1555 MHz (메모리)                 │
│  팬:       2400 RPM                                             │
│                                                                 │
│  실행 모델: llama3 (12.0 GB), codellm (8.0 GB)                │
│  NUMA:     NUMA 0                                               │
│  NVLink:   GPU 1 (600 MB/s)                                    │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

**Web UI 뷰:**

GPU 상세 카드는 다음 요소를 포함합니다:

- **모델명**: GPU 모델 이름 (예: NVIDIA A100-SXM4-80GB)
- **VRAM 바**: 사용량/전체 용량 바 차트 (색상 등급 적용)
- **활용률 게이지**: 원형 게이지 차트 (0–100%)
- **온도 게이지**: 온도 게이지 (그라데이션 색상 적용)
- **전력 게이지**: 전력 사용/제한 바 차트
- **클럭 속도**: 코어/메모리 클럭 표시
- **팬 RPM**: 쿨링 팬 속도 표시
- **실행 모델 목록**: 해당 GPU에서 실행 중인 모델
- **NUMA 할당**: NUMA 노드 매핑 정보
- **NVLink 연결**: NVLink 연결 대상 및 대역폭

### 6.2 NUMA 토폴로지 다이어그램

GPU와 NUMA 노드 간의 매핑, NVLink 연결을 시각화합니다.

**Web UI 구현:**

SVG 기반 인터랙티브 다이어그램으로 구현합니다.

```
┌──────────────────────────────────────────────────────────────────┐
│                     NUMA 토폴로지 (node-01)                      │
│                                                                  │
│      ┌─── NUMA 0 ─────────────────┐                              │
│      │                            │                              │
│      │  ┌─────────┐  NVLink  ┌─────────┐                       │
│      │  │  GPU 0   │═════════│  GPU 1   │                       │
│      │  │ A100-80G │  600GB/s│ A100-80G │                       │
│      │  │ 72°C     │         │ 58°C     │                       │
│      │  │ 72% util │         │ 45% util │                       │
│      │  └─────────┘         └─────────┘                       │
│      │       ║                     ║                           │
│      │    PCIe                   PCIe                          │
│      │       ║                     ║                           │
│      └───────╫─────────────────────╫───────────────────────────┘
│              ║                     ║                            │
│      ┌───────╫─────────────────────╫───────────────────────────┐
│      │       ║                     ║                           │
│      │  ┌─────────┐  NVLink  ┌─────────┐                       │
│      │  │  GPU 2   │═════════│  GPU 3   │                       │
│      │  │ A100-80G │  600GB/s│ A100-80G │                       │
│      │  │ 55°C     │         │ 49°C     │                       │
│      │  │ 38% util │         │ 42% util │                       │
│      │  └─────────┘         └─────────┘                       │
│      │                            │                              │
│      └─── NUMA 1 ─────────────────┘                              │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

**인터랙션:**

- GPU 노드 클릭: 상세 정보 패널 열기
- NVLink 링크: 대역폭 툴팁 표시
- PCIe 링크: 세대 및 대역폭 툴팁 표시
- NUMA 영역: 해당 NUMA 노드의 CPU/메모리 정보 툴팁

### 6.3 열 지도 (Thermal Heatmap)

클러스터 전체 GPU 온도를 색상 코딩된 그리드로 표시합니다.

**색상 매핑 규칙:**

| 온도 | 색상 | HEX |
|---|---|---|
| 0–40°C | 진한 파랑 | `#1e40af` |
| 40–55°C | 파랑 | `#3b82f6` |
| 55–65°C | 녹색 | `#22c55e` |
| 65–75°C | 노란색 | `#eab308` |
| 75–85°C | 주황색 | `#f97316` |
| >85°C | 빨간색 | `#ef4444` |

**Web UI 구현:**

Recharts `<HeatMap>` 또는 커스텀 SVG 그리드로 구현합니다. 각 셀은 클릭 가능하며, 해당 GPU 상세 뷰로 이동합니다.

### 6.4 전력 소비 차트

GPU별 및 클러스터 전체 실시간 전력 소비를 라인 차트로 표시합니다.

**Web UI 구현:**

Recharts `<LineChart>`를 사용하며, 각 GPU당 하나의 라인과 클러스터 총합 라인을 표시합니다.

```
전력 소비 (W)
 400 ┤
     │    ╭─╮  ╭─╮
 350 ┤   ╱  ╰─╯  ╰── GPU 0 (node-01)
     │  ╱            ╭─╮
 300 ┤ ╱        ╭──╮╯  ╰── GPU 1 (node-01)
     │╱    ╭──╮╯  ╰────── 클러스터 총합
 250 ┤     ╯  ╰────────── GPU 2 (node-01)
     │
 200 ┤──────────────────────
     └────────────────────── 시간
     14:30  14:31  14:32
```

---

## 7. 소프트웨어 상세 뷰

### 7.1 요청 타임라인 (Request Timeline)

개별 요청의 전체 처리 과정을 타임라인으로 시각화합니다.

**TUI 뷰 (요청 선택 시):**

```
┌─ 요청 트레이스: req-abc123 ──────────────────────────────────────┐
│                                                                   │
│  모델: llama3     제공자: local-vllm     상태: ✓ 200              │
│  총 레이턴시: 47ms     토큰: 24+128=152                           │
│                                                                   │
│  ┌─ 라우팅 결정 ──── 2ms ─────────────────────────────────────┐  │
│  │  전략: latency_priority                                    │  │
│  │  선택: local-vllm (폴백 레벨 0)                            │  │
│  │  이유: 가장 낮은 예상 레이턴시                             │  │
│  └────────────────────────────────────────────────────────────┘  │
│  ┌─ 큐 대기 ──────── 3ms ─────────────────────────────────────┐  │
│  │  큐 깊이: 2                                                │  │
│  └────────────────────────────────────────────────────────────┘  │
│  ┌─ 프리필 ──────── 18ms ─────────────────────────────────────┐  │
│  │  프롬프트 토큰: 24                                         │  │
│  │  배치 크기: 8                                              │  │
│  └────────────────────────────────────────────────────────────┘  │
│  ┌─ 디코드 ──────── 24ms ─────────────────────────────────────┐  │
│  │  생성 토큰: 128                                            │  │
│  │  토큰당 레이턴시: 0.19ms                                   │  │
│  └────────────────────────────────────────────────────────────┘  │
│  ┌─ 후처리 ──────── 2ms ──────────────────────────────────────┐  │
│  │  필터링: 적용됨                                            │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
```

**Web UI 구현:**

Recharts `<BarChart>`를 수평 방향으로 사용하거나, 커스텀 SVG 타임라인 컴포넌트로 구현합니다. 각 단계를 클릭하면 상세 정보를 확장합니다.

### 7.2 모델 성능 테이블

모델별 핵심 성능 지표를 비교 테이블로 표시합니다.

| 모델 | 엔진 | 평균 레이턴시 | 처리량 | 에러율 | 비용/1K 토큰 | VRAM |
|---|---|---|---|---|---|---|
| llama3 | vLLM | 42ms | 128 tok/s | 0.2% | $0.00 (로컬) | 12 GB |
| mixtral | TensorRT | 189ms | 64 tok/s | 0.5% | $0.00 (로컬) | 24 GB |
| codellm | vLLM | 92ms | 96 tok/s | 1.2% | $0.00 (로컬) | 8 GB |
| phi3 | ONNX Runtime | 15ms | 256 tok/s | 0.1% | $0.00 (로컬) | 4 GB |
| qwen2 | TensorRT | 210ms | 48 tok/s | 0.8% | $0.00 (로컬) | 16 GB |

**Web UI 구현:**

shadcn/ui `<Table>` 컴포넌트를 사용하며, 정렬 및 필터링 기능을 포함합니다. 각 행 클릭 시 모델 상세 뷰로 이동합니다.

### 7.3 제공자 비교

로컬 및 원격 제공자의 레이턴시, 비용, 품질을 나란히 비교합니다.

**Web UI 뷰:**

```
┌─ 제공자 비교 (llama3 기준) ─────────────────────────────────────┐
│                                                                   │
│  ┌─ 레이턴시 비교 ──────────────────────────────────────────────┐ │
│  │  📊 그룹 바 차트 (p50, p95, p99)                             │ │
│  │                                                               │ │
│  │  local-vllm:   p50=42ms  p95=85ms  p99=150ms                │ │
│  │  local-trt:    p50=38ms  p95=72ms  p99=120ms                │ │
│  │  remote-openai: p50=120ms p95=250ms p99=500ms               │ │
│  │  remote-anthro: p50=180ms p95=350ms p99=700ms               │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                                                   │
│  ┌─ 비용 비교 ──────────────────────────────────────────────────┐ │
│  │  📊 바 차트 ($/1K 토큰)                                      │ │
│  │                                                               │ │
│  │  local-vllm:    $0.00 (자가호스팅, 전력 비용만)              │ │
│  │  local-trt:     $0.00 (자가호스팅, 전력 비용만)              │ │
│  │  remote-openai: $0.005 (입력) / $0.015 (출력)               │ │
│  │  remote-anthro: $0.003 (입력) / $0.015 (출력)               │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                                                   │
│  ┌─ 품질 벤치마크 ──────────────────────────────────────────────┐ │
│  │  📊 레이더 차트 (정확도, 창의성, 코딩, 추론, 다국어)        │ │
│  │                                                               │ │
│  │  local-vllm:     정확도 85  창의성 78  코딩 82  추론 80      │ │
│  │  remote-openai:  정확도 92  창의성 88  코딩 90  추론 89      │ │
│  │  remote-anthro:  정확도 90  창의성 85  코딩 88  추론 91      │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                                                   │
└───────────────────────────────────────────────────────────────────┘
```

### 7.4 토큰 사용량 추이

시간별/일별/주별 토큰 사용량을 집계하여 차트로 표시합니다.

**Web UI 구현:**

Recharts `<AreaChart>`를 사용하며, 시간 범위 선택 (1h / 6h / 24h / 7d / 30d) 기능을 포함합니다.

```
토큰 사용량 (일별 추이)
 500K ┤
       │        ╭──╮
 400K ┤       ╭╯  ╰╮    ╭─╮
       │      ╯     ╰──╮╯  ╰──
 300K ┤   ╭──╯         ╰─────
       │  ╯
 200K ┤──╯
       │
 100K ┤
       └──────────────────────────
        월   화   수   목   금   토   일

 ── 입력 토큰   ── 출력 토큰
```

**집계 수준:**

| 시간 범위 | 집계 단위 | 데이터 포인트 |
|---|---|---|
| 1시간 | 1분 | 60 |
| 6시간 | 5분 | 72 |
| 24시간 | 15분 | 96 |
| 7일 | 1시간 | 168 |
| 30일 | 1일 | 30 |

---

## 8. 부록: 컴포넌트 계층 구조

### 8.1 TUI 컴포넌트 (Ratatui)

```
gadgetron-tui/
├── src/
│   ├── app.rs                  # 앱 진입점, 이벤트 루프
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── layout.rs           # 전체 레이아웃 정의
│   │   ├── header.rs           # 상태 표시줄
│   │   ├── footer.rs           # 키바인딩 표시줄
│   │   ├── nodes_panel.rs      # 노드 패널
│   │   ├── models_panel.rs     # 모델 패널
│   │   ├── requests_panel.rs   # 요청 패널
│   │   ├── metrics_bar.rs      # 메트릭 스파크라인
│   │   └── components/
│   │       ├── mod.rs
│   │       ├── gauge.rs        # GPU/CPU/RAM 게이지 바
│   │       ├── temperature.rs  # 온도 그라데이션
│   │       ├── sparkline.rs    # 스파크라인 차트
│   │       ├── table.rs        # 테이블 뷰
│   │       └── status_icon.rs  # 상태 아이콘/색상
│   ├── state/
│   │   ├── mod.rs
│   │   ├── cluster.rs          # 클러스터 상태
│   │   ├── node.rs             # 노드 상태
│   │   ├── model.rs            # 모델 상태
│   │   └── request.rs          # 요청 로그
│   ├── ws/
│   │   ├── mod.rs
│   │   └── client.rs           # WebSocket 클라이언트
│   └── theme/
│       ├── mod.rs
│       └── colors.rs           # 색상 정의
```

### 8.2 Web UI 컴포넌트 (React)

```
gadgetron-web/
├── src/
│   ├── main.tsx                # 진입점
│   ├── App.tsx                 # 루트 컴포넌트
│   ├── routes.tsx              # 라우트 정의
│   ├── layouts/
│   │   ├── AppLayout.tsx       # 메인 레이아웃 (사이드바 + 콘텐츠)
│   │   ├── Sidebar.tsx         # 사이드바 내비게이션
│   │   └── BottomNav.tsx       # 모바일 하단 내비게이션
│   ├── pages/
│   │   ├── DashboardPage.tsx
│   │   ├── NodesPage.tsx
│   │   ├── ModelsPage.tsx
│   │   ├── RoutingPage.tsx
│   │   ├── RequestsPage.tsx
│   │   ├── AgentsPage.tsx      # 향후 구현
│   │   └── SettingsPage.tsx
│   ├── components/
│   │   ├── common/
│   │   │   ├── StatCard.tsx
│   │   │   ├── StatusBadge.tsx
│   │   │   ├── SearchBar.tsx
│   │   │   ├── FilterBar.tsx
│   │   │   ├── ThemeToggle.tsx
│   │   │   └── LiveIndicator.tsx
│   │   ├── charts/
│   │   │   ├── RequestsOverTimeChart.tsx
│   │   │   ├── LatencyChart.tsx
│   │   │   ├── GpuUtilizationBarChart.tsx
│   │   │   ├── CostTrendAreaChart.tsx
│   │   │   ├── LatencyHistogram.tsx
│   │   │   ├── ThermalHeatmap.tsx
│   │   │   ├── PowerChart.tsx
│   │   │   ├── TokenUsageChart.tsx
│   │   │   └── ProviderRadarChart.tsx
│   │   ├── nodes/
│   │   │   ├── NodeCard.tsx
│   │   │   ├── NodeCardGrid.tsx
│   │   │   ├── GpuGauge.tsx
│   │   │   ├── CpuBar.tsx
│   │   │   ├── RamBar.tsx
│   │   │   ├── GpuTopologyDiagram.tsx
│   │   │   └── GpuDetailCard.tsx
│   │   ├── models/
│   │   │   ├── ModelTable.tsx
│   │   │   ├── ModelRow.tsx
│   │   │   ├── ModelDetailPanel.tsx
│   │   │   ├── VersionManager.tsx
│   │   │   ├── ProfilingResults.tsx
│   │   │   └── DeployDialog.tsx
│   │   ├── routing/
│   │   │   ├── StrategyConfig.tsx
│   │   │   ├── StrategySelector.tsx
│   │   │   ├── FallbackChainEditor.tsx
│   │   │   ├── WeightSliders.tsx
│   │   │   ├── ProviderHealthTable.tsx
│   │   │   └── CostComparisonTable.tsx
│   │   ├── requests/
│   │   │   ├── LiveRequestStream.tsx
│   │   │   ├── RequestRow.tsx
│   │   │   ├── RequestFilterBar.tsx
│   │   │   ├── RequestTracePanel.tsx
│   │   │   ├── TraceTimeline.tsx
│   │   │   └── TraceStep.tsx
│   │   ├── agents/
│   │   │   ├── AgentTable.tsx
│   │   │   ├── AgentRow.tsx
│   │   │   ├── AgentDetailPanel.tsx
│   │   │   ├── AgentLog.tsx
│   │   │   ├── ToolCallList.tsx
│   │   │   └── AgentCreateDialog.tsx
│   │   └── settings/
│   │       ├── GeneralSettings.tsx
│   │       ├── ProviderSettings.tsx
│   │       ├── ProviderCard.tsx
│   │       ├── AddProviderDialog.tsx
│   │       ├── ApiKeyManager.tsx
│   │       ├── QuotaManager.tsx
│   │       ├── NodeRegistration.tsx
│   │       └── AlertThresholdConfig.tsx
│   ├── hooks/
│   │   ├── useWebSocket.ts     # WebSocket 연결 관리
│   │   ├── useGpuMetrics.ts    # GPU 메트릭 구독
│   │   ├── useModelStatus.ts   # 모델 상태 구독
│   │   ├── useRequestLog.ts    # 요청 로그 구독
│   │   ├── useClusterHealth.ts # 클러스터 건강 구독
│   │   └── useTheme.ts         # 테마 토글
│   ├── stores/
│   │   ├── metricsStore.ts     # Zustand 메트릭 스토어
│   │   ├── uiStore.ts          # UI 상태 (사이드바, 필터 등)
│   │   └── settingsStore.ts    # 사용자 설정
│   ├── api/
│   │   ├── client.ts           # Axum API 클라이언트
│   │   ├── nodes.ts            # 노드 API
│   │   ├── models.ts           # 모델 API
│   │   ├── routing.ts          # 라우팅 API
│   │   ├── requests.ts         # 요청 API
│   │   └── settings.ts         # 설정 API
│   ├── types/
│   │   ├── node.ts
│   │   ├── model.ts
│   │   ├── request.ts
│   │   ├── routing.ts
│   │   ├── metrics.ts
│   │   └── agent.ts
│   └── styles/
│       ├── globals.css         # Tailwind 전역 스타일
│       ├── theme.css           # CSS 변수 (다크/라이트)
│       └── components.css      # 컴포넌트 스타일
```

### 8.3 공유 타입 정의

TUI와 Web UI가 공유하는 Rust 타입 (Axum 서버에서 직렬화):

```rust
// GPU 메트릭
pub struct GpuMetrics {
    pub node_id: String,
    pub gpu_index: u32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub utilization_pct: f32,
    pub temperature_c: f32,
    pub power_w: f32,
    pub power_limit_w: f32,
    pub clock_mhz: u32,
    pub fan_rpm: u32,
}

// 모델 상태
pub struct ModelStatus {
    pub model_id: String,
    pub status: ModelStatusKind,
    pub engine: String,
    pub version: String,
    pub node_id: Option<String>,
    pub vram_used_mb: Option<u64>,
    pub loaded_at: Option<DateTime<Utc>>,
}

pub enum ModelStatusKind {
    Running,
    Loading,
    Stopped,
    Error,
    Draining,
}

// 요청 로그 항목
pub struct RequestEntry {
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub provider: String,
    pub status: u16,
    pub latency_ms: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub routing_decision: Option<RoutingDecision>,
}

// 클러스터 건강
pub struct ClusterHealth {
    pub status: HealthStatus,
    pub total_nodes: u32,
    pub online_nodes: u32,
    pub total_gpus: u32,
    pub active_gpus: u32,
    pub running_models: u32,
    pub requests_per_minute: f32,
    pub error_rate_pct: f32,
    pub cost_per_hour_usd: f64,
    pub alerts: Vec<Alert>,
}

pub enum HealthStatus {
    Healthy,
    Degraded,
    Critical,
}

// WebSocket 메시지
pub enum WsMessage {
    GpuMetrics(GpuMetricsBatch),
    ModelStatus(ModelStatus),
    RequestLog(RequestEntry),
    ClusterHealth(ClusterHealth),
}
```

---

> **문서 이력**
>
> | 버전 | 날짜 | 변경 내용 |
> |---|---|---|
> | 1.0.0 | 2026-04-11 | 초안 작성 |
