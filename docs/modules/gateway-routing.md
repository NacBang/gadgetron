# Gadgetron API 게이트웨이 및 라우팅 모듈 설계 문서

> **버전**: 1.0.0-draft
> **작성자**: API Lead
> **최종 수정**: 2026-04-11
> **상태**: 설계 단계 (Design Phase)

---

## 목차

1. [개요](#1-개요)
2. [OpenAI 호환 API](#2-openai-호환-api)
3. [프로바이더 프로토콜 변환](#3-프로바이더-프로토콜-변환)
4. [라우팅 전략](#4-라우팅-전략)
5. [엔드포인트 구성 및 파이프라인](#5-엔드포인트-구성-및-파이프라인)
6. [관리 API](#6-관리-api)
7. [엔드포인트 설정 시스템](#7-엔드포인트-설정-시스템)
8. [XaaS 레이어](#8-xaas-레이어)

---

## 1. 개요

Gadgetron의 API 게이트웨이 및 라우팅 모듈은 다중 AI 프로바이더(OpenAI, Anthropic, Gemini, Ollama, vLLM, SGLang 등)에 대한 단일 진입점을 제공하며, OpenAI 호환 API를 표준 인터페이스로 채택하여 클라이언트 측 변경 없이 프로바이더 간 전환을 가능하게 한다.

### 1.1 핵심 설계 원칙

- **프로토콜 투명성**: 클라이언트는 OpenAI API 포맷만 인지하며, 내부 프로바이더 차이는 게이트웨이가 흡수
- **스트리밍 우선**: 모든 응답은 SSE(Server-Sent Events) 스트리밍을 기본으로 지원
- **제로 카피 패스스루**: 스트리밍 응답에 대해 불필요한 버퍼링 없이 프로바이더→클라이언트로 직접 전달
- **선언적 설정**: TOML 기반 엔드포인트 정의로 런타임 핫 리로드 지원
- **멀티테넌시**: 가상 키(Virtual Key) 기반 인증으로 테넌트별 격리 보장

### 1.2 아키텍처 다이어그램

```
┌─────────────┐     ┌──────────────────────────────────────────────────┐
│   Client     │────▶│              Gadgetron Gateway                   │
│  (OpenAI SDK)│     │                                                  │
└─────────────┘     │  ┌─────────┐  ┌──────────┐  ┌───────────────┐  │
                    │  │  Auth   │─▶│Rate Limit│─▶│  Guardrails   │  │
                    │  └─────────┘  └──────────┘  └───────────────┘  │
                    │       │                                  │        │
                    │       ▼                                  ▼        │
                    │  ┌──────────────────────────────────────────┐   │
                    │  │            Routing Engine                │   │
                    │  │  ┌──────────┐ ┌─────────┐ ┌──────────┐  │   │
                    │  │  │RoundRobin│ │CostOptml│ │LatOptml  │  │   │
                    │  │  └──────────┘ └─────────┘ └──────────┘  │   │
                    │  │  ┌──────────┐ ┌─────────┐ ┌──────────┐  │   │
                    │  │  │QualOptml │ │Fallback │ │Weighted  │  │   │
                    │  │  └──────────┘ └─────────┘ └──────────┘  │   │
                    │  └──────────────────┬───────────────────────┘   │
                    │                     │                           │
                    │       ┌─────────────┼─────────────┐            │
                    │       ▼             ▼             ▼            │
                    │  ┌─────────┐  ┌──────────┐  ┌──────────┐      │
                    │  │Protocol │  │Protocol  │  │Protocol  │      │
                    │  │Translate│  │Translate │  │Translate │      │
                    │  │(Anthrc) │  │(Gemini)  │  │(Ollama)  │      │
                    │  └────┬────┘  └────┬─────┘  └────┬─────┘      │
                    └───────┼────────────┼────────────┼────────────┘
                            ▼            ▼            ▼
                    ┌───────────┐ ┌───────────┐ ┌───────────┐
                    │ Anthropic  │ │  Gemini   │ │  Ollama   │
                    │    API     │ │   API     │ │   Local   │
                    └───────────┘ └───────────┘ └───────────┘
```

---

## 2. OpenAI 호환 API

Gadgetron은 OpenAI Chat Completions API와의 완전한 호환성을 제공하여, 기존 OpenAI SDK 및 도구 생태계를 그대로 활용할 수 있도록 한다.

### 2.1 POST /v1/chat/completions

메인 프록시 엔드포인트. 스트리밍 및 비스트리밍 모두 지원.

#### 요청 스키마

```json
{
  "model": "string (필수)",
  "messages": [
    {
      "role": "system | user | assistant | tool",
      "content": "string | array (멀티모달)",
      "name": "string (선택)",
      "tool_calls": [
        {
          "id": "string",
          "type": "function",
          "function": {
            "name": "string",
            "arguments": "string (JSON)"
          }
        }
      ],
      "tool_call_id": "string (role=tool인 경우)"
    }
  ],
  "temperature": "number (0~2, 기본값: 1)",
  "top_p": "number (0~1, 기본값: 1)",
  "n": "integer (기본값: 1)",
  "max_tokens": "integer",
  "max_completion_tokens": "integer",
  "stream": "boolean (기본값: false)",
  "stream_options": {
    "include_usage": "boolean"
  },
  "stop": "string | array",
  "presence_penalty": "number (-2~2)",
  "frequency_penalty": "number (-2~2)",
  "logit_bias": "object",
  "user": "string",
  "response_format": {
    "type": "text | json_object | json_schema",
    "json_schema": {
      "name": "string",
      "schema": "object (JSON Schema)",
      "strict": "boolean"
    }
  },
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "string",
        "description": "string",
        "parameters": "object (JSON Schema)"
      }
    }
  ],
  "tool_choice": "auto | none | required | { type: 'function', function: { name: '...' } }",
  "seed": "integer",
  "metadata": "object"
}
```

#### 비스트리밍 응답 스키마

```json
{
  "id": "chatcmpl-<uuid>",
  "object": "chat.completion",
  "created": 1710000000,
  "model": "gadgetron:<provider>/<model>",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "string | null",
        "tool_calls": [
          {
            "id": "call_<uuid>",
            "type": "function",
            "function": {
              "name": "string",
              "arguments": "string"
            }
          }
        ]
      },
      "finish_reason": "stop | length | tool_calls | content_filter"
    }
  ],
  "usage": {
    "prompt_tokens": 0,
    "completion_tokens": 0,
    "total_tokens": 0,
    "prompt_tokens_details": {
      "cached_tokens": 0
    },
    "completion_tokens_details": {
      "reasoning_tokens": 0
    }
  },
  "system_fingerprint": "fp_<hash>"
}
```

#### 스트리밍 응답 (SSE)

```
data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","usage":{"prompt_tokens":25,"completion_tokens":12,"total_tokens":37}}

data: [DONE]
```

#### 모델명 규칙

Gadgetron은 내부 라우팅을 위해 확장된 모델명 포맷을 사용한다:

```
gadgetron:<provider>/<model>[:variant]

예시:
  gadgetron:anthropic/claude-sonnet-4-20250514
  gadgetron:openai/gpt-4o
  gadgetron:gemini/gemini-2.5-pro
  gadgetron:ollama/llama3:70b
  gadgetron:vllm/mixtral-8x7b:quantized
```

별칭(Alias) 지정도 지원:

```toml
[models.aliases]
"gpt-4" = "gadgetron:openai/gpt-4o"
"claude" = "gadgetron:anthropic/claude-sonnet-4-20250514"
"default" = "gadgetron:anthropic/claude-sonnet-4-20250514"
```

### 2.2 POST /v1/completions

레거시 Completions API. 하위 호환성을 위해 유지.

#### 요청 스키마

```json
{
  "model": "string (필수)",
  "prompt": "string | array (필수)",
  "suffix": "string",
  "max_tokens": "integer",
  "temperature": "number",
  "top_p": "number",
  "n": "integer",
  "stream": "boolean",
  "logprobs": "integer",
  "echo": "boolean",
  "stop": "string | array",
  "presence_penalty": "number",
  "frequency_penalty": "number",
  "best_of": "integer",
  "logit_bias": "object",
  "user": "string"
}
```

#### 응답 스키마

```json
{
  "id": "cmpl-<uuid>",
  "object": "text_completion",
  "created": 1710000000,
  "model": "gadgetron:<provider>/<model>",
  "choices": [
    {
      "text": "string",
      "index": 0,
      "logprobs": null,
      "finish_reason": "stop | length"
    }
  ],
  "usage": {
    "prompt_tokens": 0,
    "completion_tokens": 0,
    "total_tokens": 0
  }
}
```

> **참고**: Completions API는 `messages` 형식이 아닌 `prompt` 형식을 사용하므로, 내부적으로 Chat Completions 포맷으로 변환 후 프로바이더에 전달한다.

### 2.3 GET /v1/models

등록된 모든 프로바이더의 사용 가능한 모델 목록을 반환한다.

#### 요청 파라미터

| 파라미터 | 타입 | 필수 | 설명 |
|-----------|------|------|------|
| `provider` | string | 아니오 | 특정 프로바이더 필터 (예: `anthropic`, `openai`) |
| `type` | string | 아니오 | `chat`, `embedding`, `completion` 필터 |
| `capability` | string | 아니오 | 기능 필터: `vision`, `tools`, `streaming` |

#### 응답 스키마

```json
{
  "object": "list",
  "data": [
    {
      "id": "gadgetron:anthropic/claude-sonnet-4-20250514",
      "object": "model",
      "created": 1710000000,
      "owned_by": "anthropic",
      "permission": [],
      "root": "claude-sonnet-4-20250514",
      "parent": null,
      "capabilities": {
        "vision": true,
        "tools": true,
        "streaming": true,
        "json_mode": true,
        "max_context": 200000,
        "max_output": 8192
      },
      "pricing": {
        "input_per_mtok": 3.00,
        "output_per_mtok": 15.00,
        "currency": "USD"
      },
      "metadata": {
        "provider": "anthropic",
        "region": "us-east-1",
        "avg_latency_ms": 320,
        "availability": 0.999
      }
    }
  ]
}
```

### 2.4 GET /v1/embeddings (향후 지원)

임베딩 프록시 엔드포인트. 초기 릴리즈 이후 지원 예정.

#### 요청 스키마

```json
{
  "model": "string (필수)",
  "input": "string | array<string> | array<integer> | array<array<integer>>",
  "encoding_format": "float | base64",
  "dimensions": "integer",
  "user": "string"
}
```

#### 응답 스키마

```json
{
  "object": "list",
  "data": [
    {
      "object": "embedding",
      "index": 0,
      "embedding": [0.0023, -0.0091, ...]
    }
  ],
  "model": "gadgetron:openai/text-embedding-3-large",
  "usage": {
    "prompt_tokens": 8,
    "total_tokens": 8
  }
}
```

### 2.5 인증

#### Bearer 토큰 (API 키)

모든 `/v1/*` 엔드포인트는 `Authorization` 헤더를 통한 Bearer 토큰 인증을 요구한다.

```
Authorization: Bearer gad-k-v1-<base64-encoded-key>
```

#### 가상 키 (Virtual Keys) — 멀티테넌시

멀티테넌트 환경에서는 가상 키를 통해 테넌트별 격리 및 할당량 관리를 수행한다.

```toml
[auth.virtual_keys]
# 가상 키 구조: gad-vk-<tenant_id>-<key_suffix>
# 예: gad-vk-acme-corp-a1b2c3

[auth.virtual_keys.tenants.acme-corp]
name = "Acme Corporation"
api_key = "gad-vk-acme-corp-a1b2c3"
provider_keys = { anthropic = "sk-ant-...", openai = "sk-..." }
rate_limits = { rpm = 1000, tpm = 2000000 }
allowed_models = ["gadgetron:anthropic/*", "gadgetron:openai/gpt-4*"]
budget = { monthly_usd = 5000.00 }
metadata = { contact = "admin@acme.example.com", tier = "enterprise" }

[auth.virtual_keys.tenants.startup-x]
name = "Startup X"
api_key = "gad-vk-startup-x-d4e5f6"
provider_keys = { openai = "sk-..." }
rate_limits = { rpm = 100, tpm = 200000 }
allowed_models = ["gadgetron:openai/gpt-3.5-turbo"]
budget = { monthly_usd = 100.00 }
metadata = { contact = "dev@startupx.io", tier = "free" }
```

#### 인증 흐름

```
1. 요청 수신 → Authorization 헤더 추출
2. 키 형식 판별:
   - gad-k-v1-* → 직접 API 키 → 키 저장소에서 검증
   - gad-vk-*   → 가상 키 → 테넌트 매핑 → 권한/할당량 확인
3. 키 메타데이터를 요청 컨텍스트에 주입
4. 다음 미들웨어(레이트 리밋)로 전달
```

---

## 3. 프로바이더 프로토콜 변환

각 프로바이더는 고유한 API 포맷을 사용하므로, 게이트웨이는 OpenAI 포맷과 각 프로바이더 포맷 간의 양방향 변환을 수행한다.

### 3.1 OpenAI ↔ Anthropic 변환

#### 메시지 포맷 변환

**OpenAI → Anthropic (요청)**

| OpenAI 필드 | Anthropic 필드 | 변환 로직 |
|---|---|---|
| `messages[].role = "system"` | `system` (최상위 필드) | 시스템 메시지는 `messages` 배열에서 분리하여 `system` 필드로 이동. 복수의 시스템 메시지는 개행으로 결합 |
| `messages[].role = "user"` | `messages[].role = "user"` | 직접 매핑 |
| `messages[].role = "assistant"` | `messages[].role = "assistant"` | 직접 매핑 (tool_calls 변환 필요) |
| `messages[].role = "tool"` | `messages[].role = "user"` + `content: [{type: "tool_result", ...}]` | tool 결과는 Anthropic에서 user 메시지 내 `tool_result` 콘텐츠 블록으로 표현 |
| `messages[].content = "string"` | `messages[].content = [{type: "text", text: "..."}]` | 문자열 콘텐츠를 콘텐츠 블록 배열로 래핑 |
| `messages[].content = [{type: "image_url", ...}]` | `content: [{type: "image", source: {type: "base64", ...}}]` | 이미지 URL → base64 다운로드 후 변환, 또는 URL 그대로 전달 |

**Anthropic → OpenAI (응답)**

| Anthropic 필드 | OpenAI 필드 | 변환 로직 |
|---|---|---|
| `content[].type = "text"` | `choices[0].message.content` | 텍스트 블록 병합 |
| `content[].type = "tool_use"` | `choices[0].message.tool_calls[]` | `tool_use` → `tool_calls` 변환 |
| `content[].type = "thinking"` | (사고 토큰, 스트리밍에서만) | `reasoning_tokens` 카운트에 반영, 내용은 선택적 전달 |
| `stop_reason = "end_turn"` | `finish_reason = "stop"` | 직접 매핑 |
| `stop_reason = "tool_use"` | `finish_reason = "tool_calls"` | 직접 매핑 |
| `stop_reason = "max_tokens"` | `finish_reason = "length"` | 직접 매핑 |

#### Tool Calls 변환

```json
// OpenAI tool_calls (요청/응답)
{
  "tool_calls": [
    {
      "id": "call_abc123",
      "type": "function",
      "function": {
        "name": "get_weather",
        "arguments": "{\"location\": \"Seoul\"}"
      }
    }
  ]
}

// Anthropic tool_use (요청/응답)
{
  "content": [
    {
      "type": "tool_use",
      "id": "toolu_abc123",
      "name": "get_weather",
      "input": { "location": "Seoul" }
    }
  ]
}
```

**변환 규칙**:
- `tool_calls[].id` ↔ `tool_use[].id`: ID 형식이 다르므로 매핑 테이블 유지
- `tool_calls[].function.arguments` (JSON 문자열) ↔ `tool_use[].input` (JSON 객체): 직렬화/역직렬화
- `tool_calls[].function.name` ↔ `tool_use[].name`: 직접 매핑

#### 사고 토큰 (Thinking Tokens) 변환

Anthropic의 확장 사고(extended thinking)는 OpenAI 포맷에 직접 대응하는 필드가 없으므로, 다음 전략을 사용한다:

```toml
[providers.anthropic.thinking]
# 사고 토큰 처리 방식
mode = "passthrough"  # "passthrough" | "strip" | "summarize"

# passthrough: 스트리밍에서 thinking 블록을 커스텀 SSE 이벤트로 전달
#   event: thinking
#   data: {"type": "thinking", "thinking": "...", "signature": "..."}

# strip: 사고 내용을 제거하고 usage에만 reasoning_tokens로 반영
# summarize: 사고 내용을 요약하여 메타데이터에 포함 (향후 지원)

budget_tokens = 10000  # 사고에 허용할 최대 토큰 수
```

스트리밍 패스스루 형식:

```
data: {"type":"thinking","thinking":"사용자의 질문을 분석하면...","signature":"..."}

data: {"type":"content_block_start","content_block":{"type":"text","text":""}}

data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"안녕하세요"}}

data: {"type":"message_stop"}
```

### 3.2 OpenAI ↔ Gemini 변환

#### 메시지 변환

**OpenAI → Gemini (요청)**

| OpenAI | Gemini | 변환 로직 |
|---|---|---|
| `messages` | `contents` | 역할 매핑 후 `parts` 배열로 변환 |
| `role = "system"` | `systemInstruction` | 시스템 메시지는 `systemInstruction` 필드로 분리 |
| `role = "user"` | `role = "user"` | 직접 매핑 |
| `role = "assistant"` | `role = "model"` | `"assistant"` → `"model"` |
| `role = "tool"` | `role = "function` | 함수 응답은 `functionResponse` 파트로 변환 |
| `content = "text"` | `parts: [{text: "..."}]` | 텍스트를 `parts` 배열로 래핑 |
| `content = [{type: "image_url"}]` | `parts: [{inlineData: {mimeType, data}}]` | 이미지 → `inlineData` |
| `tools[].function` | `tools[].functionDeclarations` | 함수 선언 변환 |

**Gemini → OpenAI (응답)**

| Gemini | OpenAI | 변환 로직 |
|---|---|---|
| `candidates[0].content.parts` | `choices[0].message.content` | 텍스트 파트 병합 |
| `candidates[0].content.parts[].functionCall` | `choices[0].message.tool_calls[]` | `functionCall` → `tool_calls` 변환 |
| `finishReason = "STOP"` | `finish_reason = "stop"` | 직접 매핑 |
| `finishReason = "MAX_TOKENS"` | `finish_reason = "length"` | 직접 매핑 |
| `usageMetadata.promptTokenCount` | `usage.prompt_tokens` | 직접 매핑 |

#### Tools 변환 상세

```json
// OpenAI tools 정의
{
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "search",
        "description": "Search the web",
        "parameters": {
          "type": "object",
          "properties": {
            "query": { "type": "string", "description": "Search query" }
          },
          "required": ["query"]
        }
      }
    }
  ]
}

// Gemini functionDeclarations 변환 결과
{
  "tools": [
    {
      "functionDeclarations": [
        {
          "name": "search",
          "description": "Search the web",
          "parameters": {
            "type": "object",
            "properties": {
              "query": { "type": "string", "description": "Search query" }
            },
            "required": ["query"]
          }
        }
      ]
    }
  ]
}
```

**주요 차이점**:
- Gemini는 `type: "function"` 래핑이 없고 `functionDeclarations` 배열을 직접 사용
- Gemini의 `functionCall`은 `arguments`가 문자열이 아닌 객체
- Gemini는 `functionResponse` 형식으로 도구 결과를 전달 (`name` + `response` 객체)

### 3.3 OpenAI ↔ Ollama 변환

Ollama는 OpenAI API와 네이티브 호환 모드를 지원하므로 최소한의 변환만 필요하다.

#### 변환 로직

| 항목 | 변환 필요 | 비고 |
|---|---|---|
| 엔드포인트 경로 | 예 | `/v1/chat/completions` → `http://localhost:11434/v1/chat/completions` (Ollama OpenAI 호환 모드) |
| 요청/응답 포맷 | 아니오 | Ollama가 OpenAI 포맷을 네이티브 지원 |
| 스트리밍 SSE | 아니오 | 동일한 SSE 형식 |
| 모델명 매핑 | 예 | `gadgetron:ollama/llama3:70b` → `llama3:70b` (프로바이더 프리픽스 제거) |
| 인증 | 아니오 | 로컬 실행 시 인증 없음 |
| 특수 파라미터 | 선택 | `num_ctx`, `num_gpu`, `num_batch` 등 Ollama 전용 파라미터는 `metadata` 필드로 전달 |

```toml
[providers.ollama]
type = "ollama"
base_url = "http://localhost:11434"
api_format = "openai_compatible"  # 네이티브 OpenAI 호환 모드 사용
auth = "none"

[providers.ollama.defaults]
num_ctx = 4096
num_gpu = 1
temperature = 0.7
```

### 3.4 OpenAI ↔ vLLM / SGLang 변환

vLLM 및 SGLang은 모두 OpenAI 호환 서버 모드를 제공하므로 변환이 거의 필요 없다.

#### 변환 로직

| 항목 | 변환 필요 | 비고 |
|---|---|---|
| 엔드포인트 | 아니오 | `/v1/chat/completions`, `/v1/completions`, `/v1/models` 모두 동일 |
| 요청/응답 포맷 | 아니오 | 네이티브 OpenAI 호환 |
| 스트리밍 SSE | 아니오 | 동일 |
| 모델명 매핑 | 예 | `gadgetron:vllm/mixtral-8x7b` → 모델 ID 매핑 |
| 특수 파라미터 | 선택 | vLLM: `guided_json`, `guided_regex`; SGLang: `regex` 등 구조화 출력 파라미터는 `response_format`으로 정규화 |

```toml
[providers.vllm]
type = "vllm"
base_url = "http://localhost:8000"
api_format = "openai_compatible"
auth = "bearer"
api_key = "token-vllm-internal"

[providers.vllm.extra_params]
guided_decoding_backend = "outlines"

[providers.sglang]
type = "sglang"
base_url = "http://localhost:30000"
api_format = "openai_compatible"
auth = "none"
```

### 3.5 스트리밍 SSE 포맷 정규화

모든 프로바이더의 스트리밍 응답을 OpenAI SSE 포맷으로 정규화한다.

#### 프로바이더별 원시 SSE 포맷

**Anthropic (원시)**:
```
event: message_start
data: {"type":"message_start","message":{"id":"msg_...","role":"assistant","content":[],...}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":12}}

event: message_stop
data: {"type":"message_stop"}
```

**Gemini (원시)** — SSE가 아닌 청크 스트리밍:
```json
{"candidates":[{"content":{"parts":[{"text":"Hello"}],"role":"model"}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":1}}
```

**정규화된 OpenAI SSE (출력)**:
```
data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: {"id":"chatcmpl-<uuid>","object":"chat.completion.chunk","created":1710000000,"model":"gadgetron:anthropic/claude-sonnet-4-20250514","usage":{"prompt_tokens":25,"completion_tokens":12,"total_tokens":37}}

data: [DONE]
```

#### 정규화 변환 테이블

| 프로바이더 | 이벤트 유형 | OpenAI 델타 필드 | 비고 |
|---|---|---|---|
| Anthropic | `content_block_delta.text_delta` | `delta.content` | 텍스트 스트리밍 |
| Anthropic | `content_block_start.tool_use` | `delta.tool_calls[0]` | 도구 호출 시작 |
| Anthropic | `content_block_delta.input_json_delta` | `delta.tool_calls[0].function.arguments` | 도구 인자 스트리밍 |
| Anthropic | `message_delta.stop_reason` | `delta.finish_reason` | 종료 사유 |
| Gemini | `candidates[0].content.parts[].text` | `delta.content` | 텍스트 스트리밍 |
| Gemini | `candidates[0].content.parts[].functionCall` | `delta.tool_calls[0]` | 도구 호출 |
| Gemini | `candidates[0].finishReason` | `delta.finish_reason` | 종료 사유 |
| Ollama/vLLM/SGLang | (OpenAI 호환) | 직접 패스스루 | 변환 불필요 |

### 3.6 구조화 출력 / JSON 모드 변환

```json
// OpenAI 요청: response_format 지정
{
  "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
  "messages": [...],
  "response_format": {
    "type": "json_schema",
    "json_schema": {
      "name": "user_profile",
      "schema": {
        "type": "object",
        "properties": {
          "name": { "type": "string" },
          "age": { "type": "integer" }
        },
        "required": ["name", "age"]
      },
      "strict": true
    }
  }
}
```

**프로바이더별 변환**:

| 프로바이더 | JSON 모드 지원 | 변환 로직 |
|---|---|---|
| OpenAI | 네이티브 | `response_format` 직접 전달 |
| Anthropic | 프롬프트 기반 | `response_format.json_schema`를 시스템 프롬프트에 JSON 스키마 지시로 삽입. `prefill`에 `{` 추가하여 JSON 시작 보장 |
| Gemini | 네이티브 | `generationConfig.responseMimeType = "application/json"`, `responseSchema` 매핑 |
| Ollama | `format: "json"` | `format: "json"` 파라미터 추가. JSON Schema는 `template` 변수로 주입 |
| vLLM | `guided_json` | `extra_body.guided_json = json_schema` 매핑 |
| SGLang | `regex` / `json_schema` | `extra_body.json_schema = schema` 매핑 |

### 3.7 비전/멀티모달 메시지 포맷 변환

```json
// OpenAI 멀티모달 메시지
{
  "role": "user",
  "content": [
    { "type": "text", "text": "이 이미지에서 무엇을 보나요?" },
    {
      "type": "image_url",
      "image_url": {
        "url": "https://example.com/image.png",
        "detail": "high"
      }
    }
  ]
}
```

**Anthropic 변환**:
```json
{
  "role": "user",
  "content": [
    { "type": "text", "text": "이 이미지에서 무엇을 보나요?" },
    {
      "type": "image",
      "source": {
        "type": "url",
        "url": "https://example.com/image.png"
      }
    }
  ]
}
```

**Gemini 변환**:
```json
{
  "role": "user",
  "parts": [
    { "text": "이 이미지에서 무엇을 보나요?" },
    {
      "fileData": {
        "mimeType": "image/png",
        "fileUri": "https://example.com/image.png"
      }
    }
  ]
}
```

**이미지 처리 전략**:

```toml
[providers.image_handling]
# 이미지 처리 방식
mode = "proxy"  # "proxy" | "download_and_reencode" | "reject_above_size"

# proxy: 원본 URL을 프로바이더에 그대로 전달 (프로바이더가 URL 접근 가능해야 함)
# download_and_reencode: 게이트웨이에서 다운로드 후 base64로 재인코딩하여 전달
# reject_above_size: 크기 제한 초과 시 400 에러 반환

max_size_mb = 20
allowed_mime_types = ["image/jpeg", "image/png", "image/gif", "image/webp"]
auto_resize = { max_width = 2048, max_height = 2048, quality = 85 }
```

---

## 4. 라우팅 전략

라우팅 엔진은 요청을 수신하면 구성된 전략에 따라 최적의 프로바이더/모델을 선택한다.

### 4.1 RoundRobin (라운드 로빈)

순차적으로 프로바이더를 순환하며 요청을 분산한다.

```toml
[routing.strategies.round_robin]
# 모델별로 활성화 가능
enabled = true

# 라운드 로빈 대상 프로바이더 목록
providers = [
  "gadgetron:openai/gpt-4o",
  "gadgetron:anthropic/claude-sonnet-4-20250514",
  "gadgetron:gemini/gemini-2.5-pro"
]

# 가중치 (선택, 기본값: 모두 동일)
weights = [1, 1, 1]
```

**동작**:
- 내부 카운터를 유지하여 `counter % len(providers)` 인덱스로 프로바이더 선택
- 가중치가 있는 경우: 가중치 비례 라운드 로빈 (Weighted Round Robin)
- 실패한 프로바이더는 일시적 제외 (서킷 브레이커와 연동)

### 4.2 CostOptimal (비용 최적화)

요청 처리 비용이 최소가 되는 프로바이더를 선택한다.

```toml
[routing.strategies.cost_optimal]
enabled = true

# 비용 데이터 소스
cost_source = "config"  # "config" | "api" (프로바이더 실시간 가격 API)

# 비용 테이블 (1M 토큰당 USD)
[routing.strategies.cost_optimal.costs]
"gadgetron:openai/gpt-4o" = { input = 2.50, output = 10.00 }
"gadgetron:openai/gpt-4o-mini" = { input = 0.15, output = 0.60 }
"gadgetron:anthropic/claude-sonnet-4-20250514" = { input = 3.00, output = 15.00 }
"gadgetron:anthropic/claude-haiku-3-20240307" = { input = 0.25, output = 1.25 }
"gadgetron:gemini/gemini-2.5-pro" = { input = 1.25, output = 10.00 }
"gadgetron:gemini/gemini-2.5-flash" = { input = 0.15, output = 0.60 }

# 프롬프트 토큰 추정 방식
token_estimation = "tiktoken"  # "tiktoken" | "character_ratio" | "exact_count"

# 동일 비용 시 타이브레이커
tiebreaker = "latency"  # "latency" | "random" | "quality"
```

**비용 계산 공식**:

```
estimated_cost = (estimated_input_tokens / 1_000_000) * input_price
               + (estimated_output_tokens / 1_000_000) * output_price
```

### 4.3 LatencyOptimal (지연 시간 최적화)

가장 낮은 응답 지연 시간을 제공하는 프로바이더를 선택한다.

```toml
[routing.strategies.latency_optimal]
enabled = true

# 지연 시간 측정 방식
measurement = "p95"  # "p50" | "p90" | "p95" | "p99" | "avg"

# 측정 윈도우
window = "5m"  # "1m" | "5m" | "15m" | "1h"

# 최소 샘플 수 (이 이하이면 해당 프로바이더 측정값을 신뢰하지 않음)
min_samples = 10

# 지연 시간 가중치 (0~1, 1이면 지연 시간만 고려)
# 나머지 가중치는 가용성에 할당
latency_weight = 0.8

# 초기 지연 시간 추정치 (데이터가 없을 때 사용)
[routing.strategies.latency_optimal.initial_estimates]
"gadgetron:openai/gpt-4o" = { p95_ms = 800 }
"gadgetron:anthropic/claude-sonnet-4-20250514" = { p95_ms = 600 }
"gadgetron:gemini/gemini-2.5-pro" = { p95_ms = 1000 }
"gadgetron:ollama/llama3:70b" = { p95_ms = 200 }
```

### 4.4 QualityOptimal (품질 최적화)

과거 평가 데이터를 기반으로 최고 품질의 응답을 제공하는 프로바이더를 선택한다.

```toml
[routing.strategies.quality_optimal]
enabled = true

# 품질 평가 방식
evaluation_source = "config"  # "config" | "arena_elo" | "custom_api"

# 작업 유형별 품질 점수 (0~100)
[routing.strategies.quality_optimal.scores]
"gadgetron:openai/gpt-4o" = { coding = 92, reasoning = 90, creative = 88, conversation = 85 }
"gadgetron:anthropic/claude-sonnet-4-20250514" = { coding = 95, reasoning = 93, creative = 90, conversation = 88 }
"gadgetron:gemini/gemini-2.5-pro" = { coding = 88, reasoning = 91, creative = 85, conversation = 82 }

# 기본 작업 유형 (요청에서 지정되지 않은 경우)
default_task = "reasoning"

# 작업 유형 감지
task_detection = "heuristic"  # "heuristic" | "explicit" | "ml_classifier"

# 휴리스틱 감지 규칙
[routing.strategies.quality_optimal.task_detection_rules]
coding = ["code", "function", "debug", "implement", "refactor"]
reasoning = ["explain", "analyze", "compare", "evaluate", "why"]
creative = ["write", "story", "poem", "creative", "imagine"]
conversation = ["chat", "help", "suggest", "recommend"]
```

### 4.5 폴백 체인 (Fallback Chains)

프로바이더 실패 시 대체 프로바이더로 순차적으로 전환한다. 클라우드 → 로컬 우아적 성능 저하(graceful degradation) 패턴을 지원한다.

```toml
[routing.fallbacks]
# 모델별 폴백 체인 정의
[routing.fallbacks.chains]
"gadgetron:openai/gpt-4o" = [
  "gadgetron:anthropic/claude-sonnet-4-20250514",
  "gadgetron:gemini/gemini-2.5-pro",
  "gadgetron:ollama/llama3:70b"
]
"gadgetron:anthropic/claude-sonnet-4-20250514" = [
  "gadgetron:openai/gpt-4o",
  "gadgetron:gemini/gemini-2.5-pro",
  "gadgetron:ollama/llama3:70b"
]
"gadgetron:openai/gpt-4o-mini" = [
  "gadgetron:anthropic/claude-haiku-3-20240307",
  "gadgetron:gemini/gemini-2.5-flash",
  "gadgetron:ollama/llama3:8b"
]

# 폴백 트리거 조건
[routing.fallbacks.trigger]
# HTTP 상태 코드 기반
status_codes = [429, 500, 502, 503, 504]
# 타임아웃 (초)
timeout_seconds = 30
# 연속 실패 횟수 (서킷 브레이커)
consecutive_failures = 3
# 서킷 브레이커 복구 시간 (초)
recovery_seconds = 60

# 폴백 시 클라이언트 통지
[routing.fallbacks.notification]
# 폴백 발생 시 응답 헤더에 정보 포함
header = true
header_name = "X-Gadgetron-Fallback"
# 로깅
log_level = "warn"
```

**폴백 응답 헤더**:

```
X-Gadgetron-Fallback: true
X-Gadgetron-Original-Provider: openai
X-Gadgetron-Actual-Provider: anthropic
X-Gadgetron-Fallback-Reason: rate_limit
```

### 4.6 가중치 무작위 라우팅 (Weighted Random)

가중치 비례 확률로 프로바이더를 무작위 선택한다.

```toml
[routing.strategies.weighted_random]
enabled = true

[routing.strategies.weighted_random.pools]
# 풀별 가중치 정의
"default" = [
  { provider = "gadgetron:openai/gpt-4o", weight = 40 },
  { provider = "gadgetron:anthropic/claude-sonnet-4-20250514", weight = 35 },
  { provider = "gadgetron:gemini/gemini-2.5-pro", weight = 25 }
]
"coding" = [
  { provider = "gadgetron:anthropic/claude-sonnet-4-20250514", weight = 50 },
  { provider = "gadgetron:openai/gpt-4o", weight = 30 },
  { provider = "gadgetron:gemini/gemini-2.5-pro", weight = 20 }
]
"local_only" = [
  { provider = "gadgetron:ollama/llama3:70b", weight = 60 },
  { provider = "gadgetron:vllm/mixtral-8x7b", weight = 40 }
]
```

### 4.7 시맨틱 라우팅 (Semantic Routing) — 향후 지원

임베딩 기반 의도 분류를 통해 요청의 의미에 가장 적합한 모델을 자동 선택한다.

```toml
[routing.strategies.semantic]
enabled = false  # 향후 활성화

# 의도 분류 모델
classifier_model = "gadgetron:openai/text-embedding-3-small"

# 의도 카테고리 정의
[routing.strategies.semantic.intents]
code_generation = {
  examples = ["함수를 작성해", "버그를 수정해", "코드 리뷰해줘"],
  model = "gadgetron:anthropic/claude-sonnet-4-20250514"
}
creative_writing = {
  examples = ["소설을 써줘", "시를 지어줘", "창의적인 글"],
  model = "gadgetron:openai/gpt-4o"
}
factual_qa = {
  examples = ["설명해줘", "무엇인가요", "어떻게 동작하나요"],
  model = "gadgetron:gemini/gemini-2.5-pro"
}
quick_chat = {
  examples = ["안녕", "잘 지내?", "간단히 대답해"],
  model = "gadgetron:anthropic/claude-haiku-3-20240307"
}

# 유사도 임계값 (이 이하이면 기본 라우팅 사용)
similarity_threshold = 0.7
```

### 4.8 ML 기반 라우팅 (ML-Based Routing) — 향후 지원

과거 요청-응답 품질 데이터를 기반으로 품질 예측 모델을 학습하여 라우팅决策를 최적화한다.

```toml
[routing.strategies.ml_based]
enabled = false  # 향후 활성화

# 품질 예측 모델
model_path = "./models/routing_predictor.onnx"
model_type = "onnx"  # "onnx" | "torchscript" | "triton"

# 입력 피처
features = [
  "prompt_length",           # 프롬프트 길이
  "prompt_language",         # 프롬프트 언어
  "task_type",               # 작업 유형 (분류)
  "estimated_complexity",    # 추정 복잡도
  "required_capabilities",   # 필요 기능 (vision, tools 등)
  "time_of_day",             # 시간대
  "provider_load",           # 프로바이더 부하
  "historical_quality",      # 과거 품질 점수
  "historical_latency"       # 과거 지연 시간
]

# 학습 데이터 수집
[routing.strategies.ml_based.data_collection]
enabled = true
storage = "sqlite"  # "sqlite" | "postgres" | "parquet"
path = "./data/routing_feedback.db"
# 피드백 수집 방식
feedback_sources = ["implicit", "explicit"]
# implicit: 재시도, 응답 시간, 에러율
# explicit: 사용자 평가 (thumbs up/down)

# 모델 재학습 주기
retrain_interval = "24h"
```

### 4.9 모델별 라우팅 오버라이드

특정 모델에 대해 전역 라우팅 전략을 오버라이드할 수 있다.

```toml
[routing.overrides]
# 특정 모델별 라우팅 전략 오버라이드
"gadgetron:anthropic/claude-sonnet-4-20250514" = { strategy = "quality_optimal", task = "coding" }
"gadgetron:openai/gpt-4o-mini" = { strategy = "cost_optimal" }
"gadgetron:ollama/*" = { strategy = "round_robin" }

# 테넌트별 오버라이드
[routing.overrides.tenants]
"acme-corp" = { default_strategy = "quality_optimal" }
"startup-x" = { default_strategy = "cost_optimal" }

# 엔드포인트별 오버라이드
[routing.overrides.endpoints]
"/v1/chat/completions" = { default_strategy = "latency_optimal" }
"/v1/embeddings" = { strategy = "round_robin" }
```

---

## 5. 엔드포인트 구성 및 파이프라인

### 5.1 요청 파이프라인

모든 요청은 다음 미들웨어 체인을 순차적으로 통과한다:

```
Client Request
    │
    ▼
┌──────────────────────┐
│ 1. Auth Middleware    │  Bearer 토큰 검증, 가상 키 매핑
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 2. Rate Limit        │  테넌트/키별 RPM/TPM 제한
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 3. Guardrails        │  입력/출력 필터링, PII 감지, 콘텐츠 정책
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 4. Routing           │  라우팅 전략 적용, 프로바이더 선택
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 5. Protocol Translate│  OpenAI → 프로바이더 포맷 변환
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 6. Provider Call     │  프로바이더 API 호출 (재시도 포함)
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 7. Protocol Translate│  프로바이더 → OpenAI 포맷 변환
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 8. Guardrails (out)  │  출력 필터링
└──────────┬───────────┘
           ▼
┌──────────────────────┐
│ 9. Metrics & Logging │  사용량 기록, 메트릭 수집
└──────────┬───────────┘
           ▼
Client Response
```

### 5.2 미들웨어 체인 아키텍처 (Tower Service 레이어)

Rust의 `tower` 생태계를 기반으로 미들웨어 체인을 구성한다.

```rust
// 개념적 아키텍처 (의사코드)
use tower::ServiceBuilder;
use tower::ServiceExt;

type GatewayService = Stack<
    MetricsLayer,
    Stack<
        GuardrailsOutputLayer,
        Stack<
            ProtocolTranslateOutputLayer,
            Stack<
                ProviderCallLayer,
                Stack<
                    ProtocolTranslateInputLayer,
                    Stack<
                        RoutingLayer,
                        Stack<
                            GuardrailsInputLayer,
                            Stack<
                                RateLimitLayer,
                                Stack<
                                    AuthLayer,
                                    RouterService
                                >
                            >
                        >
                    >
                >
            >
        >
    >
>;

// 서비스 빌더를 통한 파이프라인 구성
let service = ServiceBuilder::new()
    .layer(AuthLayer::new(auth_config))
    .layer(RateLimitLayer::new(rate_limit_config))
    .layer(GuardrailsInputLayer::new(guardrails_config))
    .layer(RoutingLayer::new(routing_config))
    .layer(ProtocolTranslateInputLayer::new())
    .layer(ProviderCallLayer::new(provider_registry))
    .layer(ProtocolTranslateOutputLayer::new())
    .layer(GuardrailsOutputLayer::new(guardrails_config))
    .layer(MetricsLayer::new(metrics_config))
    .service(RouterService::new());
```

#### 미들웨어 인터페이스

```rust
/// 각 미들웨어 레이어의 공통 인터페이스
#[async_trait]
pub trait Middleware: Clone + Send + Sync + 'static {
    type Request;
    type Response;
    type Error;

    async fn process(
        &self,
        request: Self::Request,
        next: Next<Self::Request, Self::Response, Self::Error>,
    ) -> Result<Self::Response, Self::Error>;
}

/// 요청 컨텍스트 — 모든 미들웨어가 공유하는 상태
pub struct RequestContext {
    pub request_id: Uuid,
    pub tenant_id: Option<String>,
    pub api_key: ApiKeyInfo,
    pub model: String,
    pub provider: Option<String>,
    pub routing_decision: Option<RoutingDecision>,
    pub timestamps: RequestTimestamps,
    pub metadata: HashMap<String, Value>,
}

/// 요청 타임스탬프 — 각 단계별 소요 시간 추적
pub struct RequestTimestamps {
    pub received_at: Instant,
    pub auth_completed_at: Option<Instant>,
    pub rate_limit_completed_at: Option<Instant>,
    pub guardrails_completed_at: Option<Instant>,
    pub routing_completed_at: Option<Instant>,
    pub provider_call_started_at: Option<Instant>,
    pub provider_call_completed_at: Option<Instant>,
    pub response_sent_at: Option<Instant>,
}
```

### 5.3 요청/응답 변환 훅

파이프라인의 각 단계에 커스텀 변환 훅을 삽입할 수 있다.

```toml
[pipeline.hooks]
# 요청 전처리 훅 (인증 이후, 라우팅 이전)
[pipeline.hooks.pre_routing]
# 프롬프트 템플릿 주입
type = "prompt_template"
enabled = true

[pipeline.hooks.pre_routing.config]
# 시스템 프롬프트 앞에 안전 지침 추가
system_prefix = "당신은 안전하고 유용한 AI 어시스턴트입니다. "
# 사용자 메시지 뒤에 컨텍스트 추가
user_suffix = ""

[pipeline.hooks.post_provider]
# 프로바이더 응답 후처리
type = "response_transform"
enabled = true

[pipeline.hooks.post_provider.config]
# PII 마스킹
mask_pii = true
# URL 정규화
normalize_urls = false

[pipeline.hooks.pre_response]
# 클라이언트 응답 전 최종 처리
type = "response_finalize"
enabled = true

[pipeline.hooks.pre_response.config]
# 사용량 헤더 추가
add_usage_headers = true
# 프로바이더 정보 헤더
add_provider_headers = true
```

### 5.4 스트리밍 패스스루 (제로 카피)

스트리밍 응답에 대해 불필요한 버퍼링을 방지하고 프로바이더→클라이언트로 직접 전달한다.

```rust
/// 스트리밍 패스스루 아키텍처 개념
///
/// 프로바이더 SSE 스트림 → 정규화 변환기 → 클라이언트 SSE 스트림
///
/// 핵심 원칙:
/// 1. 청크 단위 변환: 전체 응답을 버퍼링하지 않고 개별 SSE 이벤트를 변환
/// 2. 백프레셔 전파: 클라이언트가 느린 경우 프로바이더에 백프레셔 전달
/// 3. 부분 실패 처리: 스트리밍 중 프로바이더 연결 끊김 시 클라이언트에 에러 이벤트 전송
pub struct StreamingPassthrough {
    /// 프로바이더별 SSE 디코더
    provider_decoder: Box<dyn SseDecoder>,
    /// OpenAI SSE 인코더
    openai_encoder: OpenAiSseEncoder,
    /// 청크 버퍼 크기
    chunk_size: usize,
    /// 플러시 간격 (밀리초)
    flush_interval_ms: u64,
}

/// 스트리밍 응답 메타데이터
pub struct StreamMetadata {
    pub request_id: String,
    pub model: String,
    pub provider: String,
    pub created_at: i64,
    pub stream_started_at: Instant,
    pub first_token_at: Option<Instant>,
    pub tokens_streamed: u64,
}
```

**스트리밍 플로우**:

```
Provider SSE Stream
       │
       ▼
┌──────────────────┐
│ SSE Chunk Reader │  비동기 청크 읽기
└────────┬─────────┘
         ▼
┌──────────────────┐
│ Provider Decoder │  프로바이더별 SSE 디코딩
└────────┬─────────┘
         ▼
┌──────────────────┐
│ Normalizer       │  OpenAI SSE 포맷으로 정규화
└────────┬─────────┘
         ▼
┌──────────────────┐
│ Guardrails (out) │  스트리밍 가드레일 (토큰 단위)
└────────┬─────────┘
         ▼
┌──────────────────┐
│ OpenAI Encoder   │  OpenAI SSE 인코딩
└────────┬─────────┘
         ▼
Client SSE Stream
```

**스트리밍 중 에러 처리**:

```
data: {"id":"chatcmpl-abc","choices":[{"delta":{"content":"안녕하"},"finish_reason":null}]}

data: {"id":"chatcmpl-abc","choices":[{"delta":{"content":"세요"},"finish_reason":null}]}

data: {"error":{"type":"provider_error","code":"upstream_timeout","message":"프로바이더 연결 시간 초과"}}

data: [DONE]
```

---

## 6. 관리 API

Gadgetron 클러스터의 운영 및 관리를 위한 API 모음이다. 모든 관리 API는 `/api/v1` 경로 하에 위치한다.

### 6.1 GET /api/v1/nodes

클러스터 내 모든 노드의 목록을 반환한다.

#### 요청 파라미터

| 파라미터 | 타입 | 필수 | 설명 |
|---|---|---|---|
| `status` | string | 아니오 | 상태 필터: `online`, `offline`, `draining` |
| `role` | string | 아니오 | 역할 필터: `gateway`, `worker`, `inference` |
| `region` | string | 아니오 | 리전 필터 |

#### 응답 스키마

```json
{
  "nodes": [
    {
      "id": "node-abc123",
      "hostname": "gadgetron-worker-01",
      "role": "inference",
      "status": "online",
      "region": "us-east-1",
      "ip_address": "10.0.1.100",
      "port": 8080,
      "gpu": {
        "count": 4,
        "model": "NVIDIA A100 80GB",
        "memory_total_mb": 327680,
        "memory_used_mb": 245760,
        "utilization_pct": 78
      },
      "cpu": {
        "cores": 64,
        "utilization_pct": 45
      },
      "memory": {
        "total_mb": 262144,
        "used_mb": 180224,
        "available_mb": 81920
      },
      "models_loaded": [
        "gadgetron:ollama/llama3:70b",
        "gadgetron:vllm/mixtral-8x7b"
      ],
      "uptime_seconds": 864000,
      "last_heartbeat": "2026-04-11T10:30:00Z",
      "version": "1.0.0",
      "labels": {
        "tier": "production",
        "gpu-type": "a100"
      }
    }
  ],
  "total": 5,
  "page": 1,
  "per_page": 20
}
```

### 6.2 GET /api/v1/nodes/:id/metrics

특정 노드의 리소스 메트릭을 반환한다.

#### 경로 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `id` | string | 노드 ID |

#### 쿼리 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `range` | string | 시간 범위: `1h`, `6h`, `24h`, `7d` (기본값: `1h`) |
| `interval` | string | 데이터 포인트 간격: `1m`, `5m`, `15m`, `1h` |

#### 응답 스키마

```json
{
  "node_id": "node-abc123",
  "range": "1h",
  "interval": "5m",
  "metrics": {
    "gpu_utilization": [
      { "timestamp": "2026-04-11T09:30:00Z", "value": 75.2 },
      { "timestamp": "2026-04-11T09:35:00Z", "value": 78.1 },
      { "timestamp": "2026-04-11T09:40:00Z", "value": 82.0 }
    ],
    "gpu_memory_used_mb": [
      { "timestamp": "2026-04-11T09:30:00Z", "value": 240000 },
      { "timestamp": "2026-04-11T09:35:00Z", "value": 245760 },
      { "timestamp": "2026-04-11T09:40:00Z", "value": 250000 }
    ],
    "cpu_utilization": [
      { "timestamp": "2026-04-11T09:30:00Z", "value": 42.0 },
      { "timestamp": "2026-04-11T09:35:00Z", "value": 45.3 },
      { "timestamp": "2026-04-11T09:40:00Z", "value": 47.1 }
    ],
    "memory_used_mb": [
      { "timestamp": "2026-04-11T09:30:00Z", "value": 175000 },
      { "timestamp": "2026-04-11T09:35:00Z", "value": 180224 },
      { "timestamp": "2026-04-11T09:40:00Z", "value": 182000 }
    ],
    "requests_per_second": [
      { "timestamp": "2026-04-11T09:30:00Z", "value": 15.3 },
      { "timestamp": "2026-04-11T09:35:00Z", "value": 18.7 },
      { "timestamp": "2026-04-11T09:40:00Z", "value": 16.2 }
    ],
    "inference_latency_ms": {
      "p50": [
        { "timestamp": "2026-04-11T09:30:00Z", "value": 120 },
        { "timestamp": "2026-04-11T09:35:00Z", "value": 135 },
        { "timestamp": "2026-04-11T09:40:00Z", "value": 128 }
      ],
      "p95": [
        { "timestamp": "2026-04-11T09:30:00Z", "value": 450 },
        { "timestamp": "2026-04-11T09:35:00Z", "value": 520 },
        { "timestamp": "2026-04-11T09:40:00Z", "value": 490 }
      ],
      "p99": [
        { "timestamp": "2026-04-11T09:30:00Z", "value": 800 },
        { "timestamp": "2026-04-11T09:35:00Z", "value": 900 },
        { "timestamp": "2026-04-11T09:40:00Z", "value": 850 }
      ]
    }
  }
}
```

### 6.3 POST /api/v1/models/deploy

로컬 모델을 배포한다.

#### 요청 스키마

```json
{
  "model_id": "gadgetron:ollama/mistral-7b",
  "source": {
    "type": "ollama",
    "name": "mistral:7b"
  },
  "target": {
    "node_id": "node-abc123",
    "gpu_count": 1,
    "gpu_memory_required_mb": 16384
  },
  "config": {
    "quantization": "q4_0",
    "context_length": 4096,
    "batch_size": 32,
    "num_parallel": 4
  },
  "scheduling": {
    "priority": "normal",
    "preemptible": true
  }
}
```

| 필드 | 타입 | 필수 | 설명 |
|---|---|---|---|
| `model_id` | string | 예 | 배포할 모델의 Gadgetron 식별자 |
| `source.type` | string | 예 | 모델 소스: `ollama`, `huggingface`, `local_path`, `s3` |
| `source.name` | string | 예 | 소스의 모델명 (예: `mistral:7b`, `meta-llama/Llama-3-8B`) |
| `target.node_id` | string | 아니오 | 특정 노드 지정 (미지정 시 스케줄러가 선택) |
| `target.gpu_count` | integer | 아니오 | 필요 GPU 수 (기본값: 1) |
| `target.gpu_memory_required_mb` | integer | 아니오 | 필요 GPU 메모리 |
| `config.quantization` | string | 아니오 | 양자화 방식: `q4_0`, `q4_1`, `q5_0`, `q8_0`, `fp16`, `fp32` |
| `config.context_length` | integer | 아니오 | 최대 컨텍스트 길이 |
| `config.batch_size` | integer | 아니오 | 배치 크기 |
| `scheduling.priority` | string | 아니오 | 우선순위: `low`, `normal`, `high`, `critical` |
| `scheduling.preemptible` | boolean | 아니오 | 선점 가능 여부 |

#### 응답 스키마

```json
{
  "deployment_id": "deploy-xyz789",
  "model_id": "gadgetron:ollama/mistral-7b",
  "status": "pending",
  "target_node": "node-abc123",
  "estimated_time_seconds": 120,
  "created_at": "2026-04-11T10:00:00Z"
}
```

### 6.4 DELETE /api/v1/models/:id

모델 배포를 해제한다.

#### 경로 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `id` | string | 모델 ID (예: `gadgetron:ollama/mistral-7b`) |

#### 쿼리 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `force` | boolean | 진행 중인 요청이 있어도 강제 해제 (기본값: `false`) |
| `drain_timeout_seconds` | integer | 진행 중인 요청 완료 대기 시간 (기본값: `30`) |

#### 응답 스키마

```json
{
  "model_id": "gadgetron:ollama/mistral-7b",
  "status": "draining",
  "active_requests": 3,
  "drain_timeout_seconds": 30,
  "message": "모델이 해제 중입니다. 진행 중인 3개의 요청이 완료된 후 해제됩니다."
}
```

### 6.5 GET /api/v1/models/status

모델 배포 상태를 조회한다.

#### 쿼리 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `model_id` | string | 특정 모델 ID 필터 |
| `node_id` | string | 특정 노드 필터 |
| `status` | string | 상태 필터: `pending`, `loading`, `ready`, `draining`, `failed` |

#### 응답 스키마

```json
{
  "models": [
    {
      "model_id": "gadgetron:ollama/llama3:70b",
      "status": "ready",
      "node_id": "node-abc123",
      "loaded_at": "2026-04-11T08:00:00Z",
      "gpu_memory_used_mb": 40960,
      "active_requests": 5,
      "total_requests": 1250,
      "avg_latency_ms": 180,
      "last_inference_at": "2026-04-11T10:29:45Z"
    },
    {
      "model_id": "gadgetron:ollama/mistral-7b",
      "status": "loading",
      "node_id": "node-abc123",
      "progress_pct": 65,
      "estimated_remaining_seconds": 42,
      "started_at": "2026-04-11T10:00:00Z"
    },
    {
      "model_id": "gadgetron:vllm/mixtral-8x7b",
      "status": "failed",
      "node_id": "node-def456",
      "error": "GPU 메모리 부족: 32768MB 필요, 24576MB 사용 가능",
      "failed_at": "2026-04-11T09:50:00Z"
    }
  ],
  "total": 3
}
```

### 6.6 GET /api/v1/usage

토큰 사용량 집계를 조회한다.

#### 쿼리 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `start_date` | string | 시작일 (ISO 8601, 기본값: 오늘 - 30일) |
| `end_date` | string | 종료일 (ISO 8601, 기본값: 오늘) |
| `granularity` | string | 집계 단위: `hourly`, `daily`, `monthly` (기본값: `daily`) |
| `tenant_id` | string | 테넌트 필터 |
| `model` | string | 모델 필터 |
| `provider` | string | 프로바이더 필터 |
| `group_by` | string | 그룹화: `model`, `provider`, `tenant`, `endpoint` |

#### 응답 스키마

```json
{
  "period": {
    "start": "2026-03-11",
    "end": "2026-04-11",
    "granularity": "daily"
  },
  "usage": [
    {
      "date": "2026-04-11",
      "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
      "provider": "anthropic",
      "tenant_id": "acme-corp",
      "input_tokens": 1500000,
      "output_tokens": 300000,
      "total_tokens": 1800000,
      "cached_tokens": 200000,
      "requests": 4500,
      "errors": 12,
      "avg_latency_ms": 320
    }
  ],
  "totals": {
    "input_tokens": 45000000,
    "output_tokens": 9000000,
    "total_tokens": 54000000,
    "requests": 135000,
    "errors": 360
  }
}
```

### 6.7 GET /api/v1/costs

비용 추적 데이터를 조회한다.

#### 쿼리 파라미터

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `start_date` | string | 시작일 |
| `end_date` | string | 종료일 |
| `granularity` | string | 집계 단위: `daily`, `monthly` |
| `tenant_id` | string | 테넌트 필터 |
| `model` | string | 모델 필터 |
| `provider` | string | 프로바이더 필터 |
| `budget_id` | string | 예산 ID 필터 |

#### 응답 스키마

```json
{
  "period": {
    "start": "2026-03-01",
    "end": "2026-04-11",
    "granularity": "monthly"
  },
  "costs": [
    {
      "month": "2026-03",
      "provider": "anthropic",
      "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
      "tenant_id": "acme-corp",
      "input_cost_usd": 135.00,
      "output_cost_usd": 45.00,
      "total_cost_usd": 180.00,
      "input_tokens": 45000000,
      "output_tokens": 3000000,
      "budget_usd": 5000.00,
      "budget_used_pct": 3.6
    }
  ],
  "totals": {
    "total_cost_usd": 1250.00,
    "budget_usd": 5000.00,
    "budget_used_pct": 25.0,
    "cost_by_provider": {
      "anthropic": 800.00,
      "openai": 350.00,
      "google": 100.00
    },
    "cost_by_model": {
      "gadgetron:anthropic/claude-sonnet-4-20250514": 600.00,
      "gadgetron:openai/gpt-4o": 350.00,
      "gadgetron:anthropic/claude-haiku-3-20240307": 200.00,
      "gadgetron:gemini/gemini-2.5-pro": 100.00
    }
  }
}
```

### 6.8 WebSocket /api/v1/ws/metrics

실시간 메트릭 스트림을 위한 WebSocket 엔드포인트.

#### 연결

```
ws://<host>:<port>/api/v1/ws/metrics
```

#### 인증

쿼리 파라미터 또는 `Sec-WebSocket-Protocol` 헤더를 통한 API 키 전달:

```
ws://<host>:<port>/api/v1/ws/metrics?token=gad-k-v1-<key>
```

또는:

```
Sec-WebSocket-Protocol: bearer.gad-k-v1-<key
```

#### 구독 메시지 (클라이언트 → 서버)

```json
{
  "action": "subscribe",
  "channels": [
    "node_metrics",
    "request_metrics",
    "cost_metrics",
    "model_status"
  ],
  "filters": {
    "node_id": "node-abc123",
    "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
    "tenant_id": "acme-corp"
  },
  "interval_ms": 5000
}
```

#### 이벤트 메시지 (서버 → 클라이언트)

**node_metrics**:
```json
{
  "channel": "node_metrics",
  "timestamp": "2026-04-11T10:30:00.000Z",
  "data": {
    "node_id": "node-abc123",
    "gpu_utilization_pct": 78.5,
    "gpu_memory_used_mb": 245760,
    "cpu_utilization_pct": 45.2,
    "memory_used_mb": 180224,
    "requests_per_second": 15.3,
    "active_connections": 42
  }
}
```

**request_metrics**:
```json
{
  "channel": "request_metrics",
  "timestamp": "2026-04-11T10:30:00.123Z",
  "data": {
    "request_id": "req-abc123",
    "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
    "provider": "anthropic",
    "tenant_id": "acme-corp",
    "latency_ms": 320,
    "input_tokens": 150,
    "output_tokens": 85,
    "finish_reason": "stop",
    "is_fallback": false,
    "routing_strategy": "latency_optimal"
  }
}
```

**cost_metrics**:
```json
{
  "channel": "cost_metrics",
  "timestamp": "2026-04-11T10:30:00Z",
  "data": {
    "tenant_id": "acme-corp",
    "period": "2026-04",
    "cost_usd": 180.00,
    "budget_usd": 5000.00,
    "budget_used_pct": 3.6,
    "projected_monthly_usd": 450.00
  }
}
```

**model_status**:
```json
{
  "channel": "model_status",
  "timestamp": "2026-04-11T10:30:00Z",
  "data": {
    "model_id": "gadgetron:ollama/mistral-7b",
    "node_id": "node-abc123",
    "previous_status": "loading",
    "current_status": "ready",
    "gpu_memory_used_mb": 16384
  }
}
```

#### 구독 해제 메시지

```json
{
  "action": "unsubscribe",
  "channels": ["cost_metrics"]
}
```

#### 에러 메시지

```json
{
  "channel": "system",
  "type": "error",
  "code": "auth_expired",
  "message": "인증 토큰이 만료되었습니다. 다시 연결하세요."
}
```

---

## 7. 엔드포인트 설정 시스템

### 7.1 선언적 TOML 기반 엔드포인트 정의

모든 엔드포인트 설정은 TOML 파일로 선언적으로 정의되며, 런타임에 핫 리로드된다.

```toml
# gadgetron.toml — Gadgetron 게이트웨이 메인 설정 파일

[server]
host = "0.0.0.0"
port = 8080
workers = 4
max_connections = 10000
request_timeout_seconds = 300
graceful_shutdown_timeout_seconds = 30

# ──────────────────────────────────────────────
# 인증 설정
# ──────────────────────────────────────────────

[auth]
# 마스터 API 키
master_key = "gad-k-v1-master-xxxxxxxxxxxx"

# 키 검증 방식
validation = "hmac"  # "hmac" | "database" | "file"

# 키 만료 (초, 0 = 만료 없음)
key_expiry_seconds = 0

# ──────────────────────────────────────────────
# 엔드포인트 정의
# ──────────────────────────────────────────────

[[endpoints]]
path = "/v1/chat/completions"
method = "POST"
description = "메인 챗 완성 프록시 엔드포인트"

[endpoints.auth]
required = true
# 이 엔드포인트에만 적용되는 인증 키 (선택)
# 지정하지 않으면 마스터 키 또는 가상 키 사용
allowed_key_types = ["master", "virtual"]

[endpoints.rate_limit]
# 분당 요청 수 제한
rpm = 1000
# 분당 토큰 수 제한
tpm = 2000000
# 동시 요청 수 제한
concurrency = 50
# 버스트 허용 비율
burst_ratio = 1.5

[endpoints.timeout]
# 전체 요청 타임아웃
request_timeout_seconds = 300
# 첫 번째 토큰까지의 타임아웃 (TTFT)
first_token_timeout_seconds = 30
# 연결 타임아웃
connect_timeout_seconds = 10

[endpoints.retry]
# 최대 재시도 횟수
max_retries = 3
# 재시도 간 대기 시간 (밀리초)
retry_delay_ms = 1000
# 재시도 간 증가 비율 (지수 백오프)
backoff_multiplier = 2.0
# 재시도 가능한 HTTP 상태 코드
retryable_status_codes = [429, 500, 502, 503, 504]
# 재시도 시 폴백 체인 사용
retry_with_fallback = true

[endpoints.middleware]
# 적용할 미들웨어 (순서대로)
chain = ["auth", "rate_limit", "guardrails", "routing", "provider"]

[endpoints.guardrails]
# 입력 가드레일
input = { pii_detection = true, content_policy = "standard", max_prompt_length = 128000 }
# 출력 가드레일
output = { content_policy = "standard", pii_masking = true }

[[endpoints]]
path = "/v1/completions"
method = "POST"
description = "레거시 완성 엔드포인트"

[endpoints.auth]
required = true
allowed_key_types = ["master", "virtual"]

[endpoints.rate_limit]
rpm = 500
tpm = 1000000
concurrency = 25

[endpoints.timeout]
request_timeout_seconds = 120
first_token_timeout_seconds = 20
connect_timeout_seconds = 10

[endpoints.retry]
max_retries = 2
retry_delay_ms = 500
backoff_multiplier = 2.0
retryable_status_codes = [429, 500, 502, 503, 504]
retry_with_fallback = true

[[endpoints]]
path = "/v1/models"
method = "GET"
description = "사용 가능한 모델 목록"

[endpoints.auth]
required = true
allowed_key_types = ["master", "virtual"]

[endpoints.rate_limit]
rpm = 100
concurrency = 10

[endpoints.timeout]
request_timeout_seconds = 10
connect_timeout_seconds = 5

[endpoints.retry]
max_retries = 0

[[endpoints]]
path = "/v1/embeddings"
method = "GET"
description = "임베딩 프록시 (향후 지원)"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 200
tpm = 500000
concurrency = 20

[endpoints.timeout]
request_timeout_seconds = 30
connect_timeout_seconds = 10

[endpoints.retry]
max_retries = 2
retry_delay_ms = 1000
backoff_multiplier = 2.0

# ──────────────────────────────────────────────
# 관리 API 엔드포인트
# ──────────────────────────────────────────────

[[endpoints]]
path = "/api/v1/nodes"
method = "GET"
description = "클러스터 노드 목록"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 60
concurrency = 5

[endpoints.timeout]
request_timeout_seconds = 15

[[endpoints]]
path = "/api/v1/nodes/:id/metrics"
method = "GET"
description = "노드 리소스 메트릭"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 60
concurrency = 5

[[endpoints]]
path = "/api/v1/models/deploy"
method = "POST"
description = "로컬 모델 배포"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 10
concurrency = 2

[endpoints.timeout]
request_timeout_seconds = 600  # 모델 다운로드 시 시간 소요

[endpoints.retry]
max_retries = 0  # 배포는 재시도하지 않음

[[endpoints]]
path = "/api/v1/models/:id"
method = "DELETE"
description = "모델 배포 해제"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 10
concurrency = 2

[[endpoints]]
path = "/api/v1/models/status"
method = "GET"
description = "모델 배포 상태"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 60
concurrency = 5

[[endpoints]]
path = "/api/v1/usage"
method = "GET"
description = "토큰 사용량 집계"

[endpoints.auth]
required = true
allowed_key_types = ["master", "virtual"]

[endpoints.rate_limit]
rpm = 30
concurrency = 3

[[endpoints]]
path = "/api/v1/costs"
method = "GET"
description = "비용 추적"

[endpoints.auth]
required = true
allowed_key_types = ["master"]

[endpoints.rate_limit]
rpm = 30
concurrency = 3

# ──────────────────────────────────────────────
# 프로바이더 설정
# ──────────────────────────────────────────────

[providers.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
auth = "bearer"
api_key = "sk-..."
organization = "org-..."
max_retries = 3
timeout_seconds = 300

[providers.openai.models]
"gpt-4o" = { context_window = 128000, max_output = 16384, vision = true, tools = true, streaming = true }
"gpt-4o-mini" = { context_window = 128000, max_output = 16384, vision = true, tools = true, streaming = true }
"o3" = { context_window = 200000, max_output = 100000, vision = true, tools = true, streaming = true }

[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.com"
auth = "x-api-key"
api_key = "sk-ant-..."
version = "2023-06-01"
max_retries = 3
timeout_seconds = 300

[providers.anthropic.models]
"claude-sonnet-4-20250514" = { context_window = 200000, max_output = 8192, vision = true, tools = true, streaming = true, thinking = true }
"claude-haiku-3-20240307" = { context_window = 200000, max_output = 4096, vision = true, tools = true, streaming = true }

[providers.gemini]
type = "gemini"
base_url = "https://generativelanguage.googleapis.com/v1beta"
auth = "api_key"
api_key = "AIza..."
max_retries = 3
timeout_seconds = 300

[providers.gemini.models]
"gemini-2.5-pro" = { context_window = 1048576, max_output = 8192, vision = true, tools = true, streaming = true }
"gemini-2.5-flash" = { context_window = 1048576, max_output = 8192, vision = true, tools = true, streaming = true }

[providers.ollama]
type = "ollama"
base_url = "http://localhost:11434"
auth = "none"
api_format = "openai_compatible"
max_retries = 2
timeout_seconds = 600  # 로컬 모델은 타임아웃을 길게

[providers.ollama.models]
"llama3:70b" = { context_window = 8192, max_output = 4096, vision = false, tools = false, streaming = true }
"llama3:8b" = { context_window = 8192, max_output = 4096, vision = false, tools = false, streaming = true }
"mistral:7b" = { context_window = 32768, max_output = 4096, vision = false, tools = true, streaming = true }

[providers.vllm]
type = "vllm"
base_url = "http://localhost:8000"
auth = "bearer"
api_key = "token-vllm-internal"
api_format = "openai_compatible"
max_retries = 2
timeout_seconds = 600

[providers.vllm.models]
"mixtral-8x7b" = { context_window = 32768, max_output = 4096, vision = false, tools = false, streaming = true }

[providers.sglang]
type = "sglang"
base_url = "http://localhost:30000"
auth = "none"
api_format = "openai_compatible"
max_retries = 2
timeout_seconds = 600

# ──────────────────────────────────────────────
# 라우팅 설정
# ──────────────────────────────────────────────

[routing]
# 기본 라우팅 전략
default_strategy = "latency_optimal"

# 전역 폴백 활성화
fallback_enabled = true

[routing.strategies.round_robin]
enabled = true
providers = [
  "gadgetron:openai/gpt-4o",
  "gadgetron:anthropic/claude-sonnet-4-20250514",
  "gadgetron:gemini/gemini-2.5-pro"
]

[routing.strategies.cost_optimal]
enabled = true
cost_source = "config"

[routing.strategies.cost_optimal.costs]
"gadgetron:openai/gpt-4o" = { input = 2.50, output = 10.00 }
"gadgetron:openai/gpt-4o-mini" = { input = 0.15, output = 0.60 }
"gadgetron:anthropic/claude-sonnet-4-20250514" = { input = 3.00, output = 15.00 }
"gadgetron:anthropic/claude-haiku-3-20240307" = { input = 0.25, output = 1.25 }
"gadgetron:gemini/gemini-2.5-pro" = { input = 1.25, output = 10.00 }
"gadgetron:gemini/gemini-2.5-flash" = { input = 0.15, output = 0.60 }

[routing.strategies.latency_optimal]
enabled = true
measurement = "p95"
window = "5m"
min_samples = 10
latency_weight = 0.8

[routing.strategies.quality_optimal]
enabled = true
evaluation_source = "config"
default_task = "reasoning"

[routing.fallbacks.chains]
"gadgetron:openai/gpt-4o" = [
  "gadgetron:anthropic/claude-sonnet-4-20250514",
  "gadgetron:gemini/gemini-2.5-pro",
  "gadgetron:ollama/llama3:70b"
]

[routing.fallbacks.trigger]
status_codes = [429, 500, 502, 503, 504]
timeout_seconds = 30
consecutive_failures = 3
recovery_seconds = 60
```

### 7.2 핫 리로드 (Hot-Reload)

설정 파일 변경 시 프로세스 재시작 없이 런타임에 설정을 갱신한다.

```toml
[server.hot_reload]
# 핫 리로드 활성화
enabled = true

# 감시할 설정 파일 경로
watch_paths = [
  "./gadgetron.toml",
  "./gadgetron.local.toml",
  "./providers.d/*.toml",
  "./models.d/*.toml"
]

# 파일 변경 후 반영까지 대기 시간 (밀리초)
# (연속 변경 방지를 위한 디바운스)
debounce_ms = 500

# 설정 검증 실패 시 롤백
rollback_on_failure = true

# 설정 변경 로깅
log_changes = true
```

**핫 리로드 프로세스**:

```
1. 파일 시스템 감시자(Watcher)가 설정 파일 변경 감지
2. 디바운스 윈도우 대기 (연속 변경 병합)
3. 새 설정 파일 파싱 및 스키마 검증
4. 기존 설정과 차이점(Diff) 계산
5. 변경 영향 분석:
   - 프로바이더 추가/제거 → 프로바이더 레지스트리 업데이트
   - 라우팅 전략 변경 → 라우팅 엔진 재구성
   - 레이트 리밋 변경 → 토큰 버켓 재초기화
   - 모델 설정 변경 → 모델 레지스트리 업데이트
6. 진행 중인 요청에 영향을 주지 않는 변경사항은 즉시 반영
7. 영향을 주는 변경사항은 드레이닝 후 반영
8. 검증 실패 시 이전 설정으로 롤백
9. 변경 이력 기록
```

### 7.3 엔드포인트별 레이트 리밋

```toml
# 글로벌 레이트 리밋 (모든 엔드포인트에 적용)
[rate_limits.global]
rpm = 5000
tpm = 10000000
concurrency = 200

# 엔드포인트별 레이트 리밋 (글로벌 오버라이드)
[rate_limits.endpoints]
"/v1/chat/completions" = { rpm = 1000, tpm = 2000000, concurrency = 50 }
"/v1/completions" = { rpm = 500, tpm = 1000000, concurrency = 25 }
"/v1/models" = { rpm = 100, concurrency = 10 }
"/v1/embeddings" = { rpm = 200, tpm = 500000, concurrency = 20 }

# 테넌트별 레이트 리밋
[rate_limits.tenants]
"acme-corp" = { rpm = 2000, tpm = 5000000, concurrency = 100 }
"startup-x" = { rpm = 100, tpm = 200000, concurrency = 10 }

# 모델별 레이트 리밋
[rate_limits.models]
"gadgetron:anthropic/claude-sonnet-4-20250514" = { rpm = 500, tpm = 1000000 }
"gadgetron:openai/gpt-4o" = { rpm = 500, tpm = 1000000 }
"gadgetron:ollama/llama3:70b" = { rpm = 50, tpm = 500000 }  # 로컬 모델은 더 낮게

# 레이트 리밋 알고리즘
[rate_limits.algorithm]
type = "token_bucket"  # "token_bucket" | "sliding_window" | "fixed_window"
# 토큰 버켓 매개변수
refill_rate = "per_minute"  # "per_second" | "per_minute"
burst_allowance = 1.5  # 버스트 배수
```

### 7.4 엔드포인트별 타임아웃 및 재시도

```toml
# 기본 타임아웃 설정
[timeouts.defaults]
request_timeout_seconds = 300
first_token_timeout_seconds = 30
connect_timeout_seconds = 10
read_timeout_seconds = 60
idle_timeout_seconds = 5

# 기본 재시도 설정
[retry.defaults]
max_retries = 3
retry_delay_ms = 1000
backoff_multiplier = 2.0
max_retry_delay_ms = 30000
retryable_status_codes = [429, 500, 502, 503, 504]
retryable_errors = ["timeout", "connection_reset", "dns_resolution"]
retry_with_fallback = true

# 프로바이더별 타임아웃 오버라이드
[timeouts.providers]
anthropic = { request_timeout_seconds = 600, first_token_timeout_seconds = 60 }
openai = { request_timeout_seconds = 300, first_token_timeout_seconds = 20 }
gemini = { request_timeout_seconds = 300, first_token_timeout_seconds = 30 }
ollama = { request_timeout_seconds = 900, first_token_timeout_seconds = 120 }
vllm = { request_timeout_seconds = 900, first_token_timeout_seconds = 120 }

# 프로바이더별 재시도 오버라이드
[retry.providers]
anthropic = { max_retries = 3, retry_delay_ms = 1000 }
openai = { max_retries = 3, retry_delay_ms = 500 }
gemini = { max_retries = 2, retry_delay_ms = 1000 }
ollama = { max_retries = 5, retry_delay_ms = 2000 }  # 로컬 모델은 재시도 많이
```

### 7.5 엔드포인트별 인증

```toml
# 엔드포인트별 인증 구성
[auth.endpoints]
# /v1/* 엔드포인트: 마스터 키 또는 가상 키
"/v1/*" = { required = true, allowed_key_types = ["master", "virtual"] }

# /api/v1/* 관리 엔드포인트: 마스터 키만
"/api/v1/nodes" = { required = true, allowed_key_types = ["master"] }
"/api/v1/nodes/*" = { required = true, allowed_key_types = ["master"] }
"/api/v1/models/deploy" = { required = true, allowed_key_types = ["master"] }
"/api/v1/models/*/status" = { required = true, allowed_key_types = ["master", "virtual"] }

# /api/v1/usage: 가상 키도 자신의 테넌트 데이터만 조회 가능
"/api/v1/usage" = { required = true, allowed_key_types = ["master", "virtual"], scope = "own_tenant" }
"/api/v1/costs" = { required = true, allowed_key_types = ["master"], scope = "all" }

# WebSocket: 쿼리 파라미터 인증
"/api/v1/ws/metrics" = { required = true, allowed_key_types = ["master"], auth_method = "query_or_header" }

# 별도 인증 키 (선택)
[auth.endpoint_keys]
# 특정 엔드포인트에만 유효한 키
"/api/v1/models/deploy" = ["gad-ek-deploy-xxx"]  # 배포 전용 키
"/api/v1/usage" = ["gad-ek-readonly-xxx"]  # 읽기 전용 키
```

---

## 8. XaaS 레이어

Gadgetron은 인프라 리소스를 서비스 형태(XaaS)로 제공하는 레이어를 포함한다.

### 8.1 GPUaaS — GPU as a Service

GPU 리소스를 동적으로 할당 및 해제한다.

#### POST /api/v1/gpu/allocate

GPU 리소스를 할당한다.

**요청 스키마**:

```json
{
  "gpu_count": 2,
  "gpu_type": "a100-80gb",
  "purpose": "inference",
  "node_preference": "node-abc123",
  "duration_seconds": 3600,
  "preemptible": true,
  "labels": {
    "model": "llama3-70b",
    "team": "ml-research"
  }
}
```

| 필드 | 타입 | 필수 | 설명 |
|---|---|---|---|
| `gpu_count` | integer | 예 | 요청 GPU 수 |
| `gpu_type` | string | 아니오 | GPU 유형: `a100-40gb`, `a100-80gb`, `h100-80gb`, `any` (기본값: `any`) |
| `purpose` | string | 아니오 | 용도: `inference`, `training`, `fine_tuning` |
| `node_preference` | string | 아니오 | 선호 노드 ID |
| `duration_seconds` | integer | 아니오 | 예상 사용 시간 (0 = 무제한) |
| `preemptible` | boolean | 아니오 | 선점 가능 여부 (비용 절감) |
| `labels` | object | 아니오 | 사용자 정의 레이블 |

**응답 스키마**:

```json
{
  "allocation_id": "gpu-alloc-xyz789",
  "status": "allocated",
  "gpu_count": 2,
  "gpu_type": "a100-80gb",
  "node_id": "node-abc123",
  "gpu_indices": [0, 1],
  "allocated_at": "2026-04-11T10:00:00Z",
  "expires_at": "2026-04-11T11:00:00Z",
  "cost": {
    "per_hour_usd": 4.90,
    "estimated_total_usd": 4.90
  }
}
```

#### POST /api/v1/gpu/release

할당된 GPU 리소스를 해제한다.

**요청 스키마**:

```json
{
  "allocation_id": "gpu-alloc-xyz789",
  "force": false
}
```

| 필드 | 타입 | 필수 | 설명 |
|---|---|---|---|
| `allocation_id` | string | 예 | 해제할 할당 ID |
| `force` | boolean | 아니오 | 진행 중인 작업이 있어도 강제 해제 (기본값: `false`) |

**응답 스키마**:

```json
{
  "allocation_id": "gpu-alloc-xyz789",
  "status": "released",
  "released_at": "2026-04-11T10:45:00Z",
  "actual_duration_seconds": 2700,
  "actual_cost_usd": 3.68,
  "gpu_count_released": 2,
  "node_id": "node-abc123"
}
```

#### GPU 상태 조회 (선택적 엔드포인트)

**GET /api/v1/gpu/allocations**

```json
{
  "allocations": [
    {
      "allocation_id": "gpu-alloc-xyz789",
      "status": "allocated",
      "gpu_count": 2,
      "gpu_type": "a100-80gb",
      "node_id": "node-abc123",
      "purpose": "inference",
      "allocated_at": "2026-04-11T10:00:00Z",
      "expires_at": "2026-04-11T11:00:00Z",
      "labels": {
        "model": "llama3-70b",
        "team": "ml-research"
      }
    }
  ],
  "cluster_total": {
    "total_gpus": 16,
    "allocated_gpus": 10,
    "available_gpus": 6
  }
}
```

### 8.2 ModelaaS — Model as a Service

모델 서빙 및 추론 엔드포인트.

#### POST /api/v1/models/serve

모델 서빙 인스턴스를 생성한다. 배포(`deploy`)와 달리, 서빙은 특정 구성으로 모델을 서비스 형태로 노출한다.

**요청 스키마**:

```json
{
  "model_id": "gadgetron:ollama/llama3-70b",
  "serve_config": {
    "replicas": 2,
    "min_replicas": 1,
    "max_replicas": 5,
    "autoscaling": {
      "enabled": true,
      "target_requests_per_second": 10,
      "scale_up_cooldown_seconds": 60,
      "scale_down_cooldown_seconds": 300
    },
    "resource_requirements": {
      "gpu_count": 1,
      "gpu_type": "a100-80gb",
      "cpu_cores": 8,
      "memory_mb": 32768
    },
    "scheduling": {
      "priority": "normal",
      "preemptible": true,
      "affinity": {
        "node_labels": { "gpu-type": "a100" }
      }
    },
    "liveness_probe": {
      "type": "http",
      "path": "/health",
      "interval_seconds": 30,
      "timeout_seconds": 5,
      "failure_threshold": 3
    },
    "readiness_probe": {
      "type": "inference",
      "test_prompt": "Hello, world!",
      "interval_seconds": 60,
      "timeout_seconds": 30,
      "failure_threshold": 3
    }
  },
  "expose": {
    "endpoint_type": "openai_compatible",
    "base_path": "/v1",
    "authentication": {
      "type": "bearer",
      "api_key": "gad-serve-xxx"
    }
  }
}
```

**응답 스키마**:

```json
{
  "serve_id": "serve-abc123",
  "model_id": "gadgetron:ollama/llama3-70b",
  "status": "creating",
  "endpoints": [
    {
      "url": "http://gadgetron-worker-01:8080/v1/chat/completions",
      "type": "openai_compatible",
      "authentication": {
        "type": "bearer",
        "api_key": "gad-serve-xxx"
      }
    }
  ],
  "replicas": {
    "desired": 2,
    "ready": 0,
    "creating": 2
  },
  "created_at": "2026-04-11T10:00:00Z"
}
```

#### POST /api/v1/models/:id/inference

특정 모델에 대한 직접 추론 엔드포인트. 모델 라우팅을 거치지 않고 지정된 모델로 직접 요청을 전송한다.

**경로 파라미터**:

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `id` | string | 모델 ID (URL 인코딩) |

**요청 스키마**:

```json
{
  "messages": [
    { "role": "user", "content": "Rust의 소유권 시스템을 설명하세요." }
  ],
  "parameters": {
    "temperature": 0.7,
    "max_tokens": 2048,
    "top_p": 0.95
  },
  "stream": true,
  "metadata": {
    "request_source": "internal_tool",
    "priority": "high"
  }
}
```

**응답 스키마**:

(`/v1/chat/completions` 응답과 동일한 포맷. `model` 필드에 지정된 모델 ID가 포함됨)

```json
{
  "id": "chatcmpl-direct-xyz789",
  "object": "chat.completion",
  "created": 1710000000,
  "model": "gadgetron:ollama/llama3:70b",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "assistant",
        "content": "Rust의 소유권 시스템은 메모리 안전성을 컴파일 타임에 보장하는 핵심 기능입니다..."
      },
      "finish_reason": "stop"
    }
  ],
  "usage": {
    "prompt_tokens": 15,
    "completion_tokens": 256,
    "total_tokens": 271
  }
}
```

**에러 응답** (모델 미배포 시):

```json
{
  "error": {
    "type": "model_not_found",
    "code": "MODEL_NOT_SERVING",
    "message": "모델 'gadgetron:ollama/llama3:70b'이(가) 현재 서빙 중이 아닙니다.",
    "suggestion": "POST /api/v1/models/deploy를 통해 모델을 먼저 배포하세요.",
    "status": 404
  }
}
```

### 8.3 AgentaaS — Agent as a Service

AI 에이전트 생성 및 실행 관리.

#### POST /api/v1/agents/create

새로운 에이전트를 생성한다.

**요청 스키마**:

```json
{
  "name": "코드 리뷰 에이전트",
  "description": "PR 코드 리뷰를 수행하는 에이전트",
  "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
  "system_prompt": "당신은 전문 코드 리뷰어입니다. 코드의 품질, 보안, 성능을 검토하고 개선 제안을 제공하세요.",
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "read_file",
        "description": "파일 내용을 읽습니다",
        "parameters": {
          "type": "object",
          "properties": {
            "path": { "type": "string", "description": "파일 경로" }
          },
          "required": ["path"]
        }
      }
    },
    {
      "type": "function",
      "function": {
        "name": "search_code",
        "description": "코드베이스에서 검색합니다",
        "parameters": {
          "type": "object",
          "properties": {
            "query": { "type": "string", "description": "검색 쿼리" },
            "file_pattern": { "type": "string", "description": "파일 패턴" }
          },
          "required": ["query"]
        }
      }
    }
  ],
  "config": {
    "max_iterations": 20,
    "max_tokens_per_iteration": 4096,
    "timeout_seconds": 300,
    "tool_execution_timeout_seconds": 60,
    "allow_parallel_tool_calls": true,
    "stop_on_tool_error": false,
    "memory": {
      "type": "conversation",
      "max_history_messages": 50,
      "summarize_after": 30
    }
  },
  "routing": {
    "strategy": "quality_optimal",
    "fallback_chain": [
      "gadgetron:anthropic/claude-sonnet-4-20250514",
      "gadgetron:openai/gpt-4o",
      "gadgetron:ollama/llama3:70b"
    ]
  },
  "metadata": {
    "team": "engineering",
    "environment": "production"
  }
}
```

| 필드 | 타입 | 필수 | 설명 |
|---|---|---|---|
| `name` | string | 예 | 에이전트 이름 |
| `description` | string | 아니오 | 에이전트 설명 |
| `model` | string | 예 | 기본 모델 |
| `system_prompt` | string | 예 | 시스템 프롬프트 |
| `tools` | array | 아니오 | 사용 가능한 도구 목록 |
| `config.max_iterations` | integer | 아니오 | 최대 반복 횟수 (기본값: 10) |
| `config.max_tokens_per_iteration` | integer | 아니오 | 반복당 최대 토큰 (기본값: 4096) |
| `config.timeout_seconds` | integer | 아니오 | 전체 타임아웃 (기본값: 300) |
| `config.tool_execution_timeout_seconds` | integer | 아니오 | 도구 실행 타임아웃 (기본값: 30) |
| `config.allow_parallel_tool_calls` | boolean | 아니오 | 병렬 도구 호출 허용 (기본값: true) |
| `config.memory.type` | string | 아니오 | 메모리 유형: `conversation`, `sliding_window`, `summarized` |
| `routing.strategy` | string | 아니오 | 라우팅 전략 오버라이드 |
| `routing.fallback_chain` | array | 아니오 | 폴백 체인 오버라이드 |

**응답 스키마**:

```json
{
  "agent_id": "agent-def456",
  "name": "코드 리뷰 에이전트",
  "status": "ready",
  "model": "gadgetron:anthropic/claude-sonnet-4-20250514",
  "tools_count": 2,
  "created_at": "2026-04-11T10:00:00Z",
  "created_by": "gad-vk-acme-corp-a1b2c3"
}
```

#### POST /api/v1/agents/:id/run

에이전트를 실행한다.

**경로 파라미터**:

| 파라미터 | 타입 | 설명 |
|---|---|---|
| `id` | string | 에이전트 ID |

**요청 스키마**:

```json
{
  "input": "이 PR의 변경 사항을 코드 리뷰해주세요: https://github.com/org/repo/pull/123",
  "context": {
    "repository": "org/repo",
    "pr_number": 123,
    "branch": "feature/new-auth"
  },
  "parameters": {
    "temperature": 0.3,
    "max_tokens": 8192
  },
  "stream": true,
  "callback_url": "https://webhook.example.com/agent-result"
}
```

| 필드 | 타입 | 필수 | 설명 |
|---|---|---|---|
| `input` | string | 예 | 에이전트에 전달할 초기 입력 |
| `context` | object | 아니오 | 도구 실행에 활용할 컨텍스트 |
| `parameters` | object | 아니오 | 모델 파라미터 오버라이드 |
| `stream` | boolean | 아니오 | 스트리밍 응답 여부 (기본값: false) |
| `callback_url` | string | 아니오 | 완료 시 콜백 URL (비스트리밍 모드) |

**비스트리밍 응답 스키마**:

```json
{
  "run_id": "run-ghi789",
  "agent_id": "agent-def456",
  "status": "completed",
  "input": "이 PR의 변경 사항을 코드 리뷰해주세요: ...",
  "output": {
    "content": "코드 리뷰 결과:\n\n1. **보안**: auth 모듈에서 비밀번호 해싱이 누락되었습니다...\n2. **성능**: N+1 쿼리 문제가 발견되었습니다...\n3. **가독성**: 함수가 너무 깁니다...",
    "tool_calls": [
      {
        "iteration": 1,
        "tool": "read_file",
        "input": { "path": "src/auth/mod.rs" },
        "output": "파일 내용 (847줄)...",
        "duration_ms": 120
      },
      {
        "iteration": 2,
        "tool": "search_code",
        "input": { "query": "password hash" },
        "output": "3개의 결과 발견...",
        "duration_ms": 85
      }
    ]
  },
  "iterations": 3,
  "usage": {
    "prompt_tokens": 5000,
    "completion_tokens": 2000,
    "total_tokens": 7000,
    "tool_calls": 2
  },
  "cost": {
    "total_usd": 0.105
  },
  "timing": {
    "started_at": "2026-04-11T10:01:00Z",
    "completed_at": "2026-04-11T10:01:45Z",
    "duration_seconds": 45,
    "tool_execution_seconds": 0.205,
    "model_inference_seconds": 44.795
  },
  "model_used": "gadgetron:anthropic/claude-sonnet-4-20250514",
  "routing": {
    "strategy": "quality_optimal",
    "provider": "anthropic",
    "fallback_triggered": false
  }
}
```

**스트리밍 응답 (SSE)**:

```
data: {"type":"agent_start","run_id":"run-ghi789","agent_id":"agent-def456"}

data: {"type":"iteration_start","iteration":1,"model":"gadgetron:anthropic/claude-sonnet-4-20250514"}

data: {"type":"content_delta","content":"코드 리뷰 결과:\n\n"}

data: {"type":"tool_call","tool":"read_file","input":{"path":"src/auth/mod.rs"}}

data: {"type":"tool_result","tool":"read_file","output":"파일 내용 (847줄)...","duration_ms":120}

data: {"type":"iteration_start","iteration":2}

data: {"type":"content_delta","content":"1. **보안**: auth 모듈에서 비밀번호 해싱이 누락되었습니다...\n"}

data: {"type":"tool_call","tool":"search_code","input":{"query":"password hash"}}

data: {"type":"tool_result","tool":"search_code","output":"3개의 결과 발견...","duration_ms":85}

data: {"type":"iteration_start","iteration":3}

data: {"type":"content_delta","content":"2. **성능**: N+1 쿼리 문제가 발견되었습니다...\n3. **가독성**: 함수가 너무 깁니다..."}

data: {"type":"agent_end","run_id":"run-ghi789","status":"completed","usage":{"prompt_tokens":5000,"completion_tokens":2000,"total_tokens":7000},"cost_usd":0.105}
```

**에이전트 실행 에러 응답**:

```json
{
  "run_id": "run-ghi789",
  "agent_id": "agent-def456",
  "status": "failed",
  "error": {
    "type": "max_iterations_exceeded",
    "code": "AGENT_MAX_ITERATIONS",
    "message": "최대 반복 횟수(20)를 초과했습니다.",
    "iterations_completed": 20,
    "last_tool_call": {
      "tool": "search_code",
      "input": { "query": "error handling patterns" }
    }
  },
  "usage": {
    "prompt_tokens": 35000,
    "completion_tokens": 15000,
    "total_tokens": 50000
  },
  "cost": {
    "total_usd": 0.75
  }
}
```

---

## 부록 A: 공통 에러 응답 포맷

모든 API 엔드포인트는 다음 통일된 에러 응답 포맷을 사용한다.

```json
{
  "error": {
    "type": "invalid_request_error | authentication_error | rate_limit_error | model_not_found_error | provider_error | internal_error",
    "code": "ERROR_CODE",
    "message": "사용자에게 표시할 메시지",
    "detail": "추가 디버그 정보 (개발 모드에서만 표시)",
    "param": "오류가 발생한 파라미터 (해당 시)",
    "status": 400
  }
}
```

### 에러 코드 목록

| HTTP 상태 | 코드 | 설명 |
|---|---|---|
| 400 | `INVALID_REQUEST` | 잘못된 요청 형식 |
| 400 | `INVALID_MODEL` | 존재하지 않는 모델 |
| 400 | `INVALID_PARAMETER` | 잘못된 파라미터 값 |
| 400 | `CONTEXT_LENGTH_EXCEEDED` | 컨텍스트 길이 초과 |
| 401 | `AUTHENTICATION_REQUIRED` | 인증 필요 |
| 401 | `INVALID_API_KEY` | 잘못된 API 키 |
| 401 | `TENANT_SUSPENDED` | 정지된 테넌트 |
| 403 | `MODEL_NOT_ALLOWED` | 테넌트에 허용되지 않은 모델 |
| 403 | `INSUFFICIENT_PERMISSIONS` | 권한 부족 |
| 429 | `RATE_LIMIT_EXCEEDED` | 요청 속도 제한 초과 |
| 429 | `TOKEN_LIMIT_EXCEEDED` | 토큰 사용량 제한 초과 |
| 429 | `CONCURRENCY_LIMIT_EXCEEDED` | 동시 요청 제한 초과 |
| 429 | `BUDGET_EXCEEDED` | 예산 초과 |
| 404 | `MODEL_NOT_FOUND` | 모델을 찾을 수 없음 |
| 404 | `NODE_NOT_FOUND` | 노드를 찾을 수 없음 |
| 404 | `AGENT_NOT_FOUND` | 에이전트를 찾을 수 없음 |
| 500 | `PROVIDER_ERROR` | 프로바이더 오류 |
| 500 | `PROVIDER_TIMEOUT` | 프로바이더 타임아웃 |
| 500 | `PROVIDER_RATE_LIMITED` | 프로바이더 측 속도 제한 |
| 500 | `INTERNAL_ERROR` | 내부 서버 오류 |
| 503 | `ALL_PROVIDERS_FAILED` | 모든 프로바이더 실패 (폴백 체인 소진) |
| 503 | `SERVICE_OVERLOADED` | 서비스 과부하 |

## 부록 B: 공통 HTTP 헤더

### 요청 헤더

| 헤더 | 설명 |
|---|---|
| `Authorization` | `Bearer gad-k-v1-...` 또는 `Bearer gad-vk-...` |
| `Content-Type` | `application/json` |
| `Accept` | `text/event-stream` (스트리밍 시) |
| `X-Gadgetron-Model` | 요청 라우팅에 사용할 모델 오버라이드 |
| `X-Gadgetron-Provider` | 요청 라우팅에 사용할 프로바이더 오버라이드 |
| `X-Gadgetron-Strategy` | 라우팅 전략 오버라이드 |
| `X-Gadgetron-Fallback-Disabled` | `true` 설정 시 폴백 비활성화 |
| `X-Gadgetron-Metadata` | JSON 형식 메타데이터 (로깅/추적용) |
| `X-Request-ID` | 요청 ID (미지정 시 자동 생성) |

### 응답 헤더

| 헤더 | 설명 |
|---|---|
| `X-Request-ID` | 요청 식별자 |
| `X-Gadgetron-Provider` | 실제 응답 프로바이더 |
| `X-Gadgetron-Model` | 실제 응답 모델 |
| `X-Gadgetron-Strategy` | 사용된 라우팅 전략 |
| `X-Gadgetron-Fallback` | 폴백 발생 여부 (`true` / `false`) |
| `X-Gadgetron-Original-Provider` | 폴백 시 원래 프로바이더 |
| `X-Gadgetron-Fallback-Reason` | 폴백 사유 |
| `X-Gadgetron-Latency-Ms` | 총 응답 시간 (ms) |
| `X-Gadgetron-First-Token-Ms` | 첫 토큰까지의 시간 (ms) |
| `X-RateLimit-Limit` | 요청 제한 한도 |
| `X-RateLimit-Remaining` | 남은 요청 수 |
| `X-RateLimit-Reset` | 제한 초기화 시간 (Unix 타임스탬프) |

---

> **문서 끝** — Gadgetron API 게이트웨이 및 라우팅 모듈 v1.0.0-draft
