# agent

## What it is

Part of AI: the model-agnostic turn runner every AI surface in the product
builds on — agent sessions, document AI, and background AI features all
run a turn through this crate's loop, with a shared model/speed vocabulary
and usage metering.

## Start here

- `src/agent_loop.rs`: `AgentLoop`/`Session` — runs a turn, dispatches tool
  calls, loops until a final response.
- `src/model/`: provider adapters (`anthropic.rs`, `openai.rs`, `gemini.rs`)
  behind `model/router.rs` (the Anthropic, OpenAI and Gemini clients' hosts
  are `macro_service_urls`' `AnthropicApiUrl`, `OpenaiApiUrl` and
  `GoogleGenerativeAiApiUrl`, so a local stack can point Gemini at
  `crates/model_emulator` and a self-host can point Claude or GPT at a
  compatible gateway), `model/local.rs` (the operator's own model server,
  below), plus `predefined_model.rs`/`speed.rs` (the
  product-facing model/speed vocabulary) and `metering.rs` (usage recording
  hook into `ai_usage`).
- `AgentLoop::with_streamed_tool_calls` opts hosts into early tool-call
  announcements and argument deltas; the accumulator persists only final calls.
- `src/system_prompt.rs`: `SystemPrompt`, built from named `PromptSection`s
  in a shared and a per-session part; every model call records each
  section's bytes and hash (and those of text the turn sends elsewhere, such as
  its SDK docs, through `SystemPrompt::record`) (`macro.genai.prompt.<section>.*`), plus tool
  schema and history sizes, on rig's `chat_streaming` span whatever the
  content policy (`src/telemetry.rs`).
- `src/stream.rs`: `StreamPart`/`ChatCompletionStream`, the wire shape
  callers consume.
- `src/completion.rs`: one-shot, non-looping completions (no tools).
- `src/tool_result_limit.rs`: `ToolResultLimit` cuts tool results too long for
  the model (in the hook and on replay through `to_rig_messages_limited`) and
  keeps them for the `ReadToolResult` pager tool; opt in with
  `AgentLoop::with_tool_result_limit`.
- `src/structured_output.rs`: prompted JSON generation with offline schema
  preflight and result validation; external schema references are rejected.
- `src/tool_media.rs`: promotes explicit top-level `modelMedia` results to
  bounded native PNG blocks for both tool adapters and history replay; nested
  content stays JSON and remote image URLs are rejected. Keep the public JSON
  envelope in streamed results/history while excluding pixels from model text.

## Scope

- Owns: the turn loop, provider routing/model selection, streaming parts,
  and metering integration.
- Not here: tool definitions and schemas → `ai_toolset`, `ai_tools`; usage
  pricing/billing → `ai_usage`, `ai_billing`.

## Boundaries

- Depends on: `ai_usage` (metering), `ai_toolset` (tool adapter trait),
  `attachment`, `genai_telemetry`, `prompt`, `rig-core`/`rig-agent`.
- Used by: `agent_inmem`, `ai_tools`, and most AI-surface crates that run a
  turn (directly or via `ai_tools`).

## Recipes

- Add a model: `src/model/predefined_model.rs` (enum variant) →
  `src/model/router.rs` (dispatch) → provider adapter in `src/model/` →
  pricing in `ai_usage`.
- Add a stream part / tool-call shape change: `src/stream.rs` → `src/convert.rs`
  (rig message conversion) → `src/accumulator.rs` (accumulation across chunks).

## Checks

- `cargo test -p agent` (`SQLX_OFFLINE` unset).

## Gotchas

- A `Session` lives for one turn. Before its first request it reloads the
  catalog tools the history called or loaded, reading `SearchTools`/`LoadTools`
  results through `ai_toolset::tool_search::tools_loaded_by`; change those
  response types there, not in `ai_tools`.
- `rig-core`/`rig-agent` are pinned exact versions (not workspace deps);
  bump both together and check `test-utils` feature compatibility.
- `AgentLoop::with_provider_api_key` rekeys only the loop's own model calls
  (native Anthropic/OpenAI); call it after `with_model`. Tools and
  `complete*` still use the deployment's keys.
- Rig records a streamed call's usage onto the `chat` span once per chunk,
  and the first write is the one Datadog shows. Gemini's interim chunks lack
  cache reads, so `MeteredHttpClient` strips their usage
  (`src/model/metering_http/interim_usage.rs`). Read billed Gemini usage
  from `ai_usage` if spans and billing disagree.
- `MeteredHttpClient` logs every provider response that means Macro's
  account is suspended or out of credit (`genai_telemetry::provider_account`),
  which pages on-call. A client rekeyed for an agent's own key is built
  `with_agent_api_key()` so its billing failures never do.

- The `local` provider (`model/local.rs`) is an operator's own OpenAI-compatible
  Chat Completions server (Ollama, `mlx_lm.server`, LM Studio, llama.cpp,
  vLLM). `LOCAL_LLM_BASE_URL` (with `/v1`) registers it, so `local/<model>`
  routes to it; optional `LOCAL_LLM_API_KEY` (a placeholder bearer is sent
  without one). With `LOCAL_LLM_MODEL` too, `ModelRouter::route` (the one
  place an id is resolved, also behind `route_or_default` and the fallback
  model) rewrites every well-formed id, whatever its provider, to
  `local/<LOCAL_LLM_MODEL>`: product tiers and background features then run
  on it. A model without a base URL fails startup. All three are optional
  process environment, so they need no Doppler key or `BootStubEnv` entry.
  Gotchas: the local client is `MeteredHttpClient::unmetered()`, so its
  requests skip admission, pricing and journaling even with billing on, and its
  failures never page as a Macro account outage; nothing guards its address
  (a LAN or `host.docker.internal` host is the point); requests to it carry no
  Anthropic `thinking`/`temperature`, OpenAI `reasoning` or speed parameters
  (`into_agent`, `complete_with_history`), because local servers may reject
  unknown fields; tool calling, long system prompts and image tool results
  depend on the model, since Chat Completions carries no tool images.
- `AnthropicApiUrl`/`OpenaiApiUrl` (`OVERRIDE_ANTHROPIC_API_URL`,
  `OVERRIDE_OPENAI_API_URL`) apply to the deployment's clients and to those
  `with_provider_api_key` rebuilds.

## Related

- Rules: `.agents/rules/rust/architecture.md` · Skill: `upgrade-model`

Canvas native image results remain native through tool output and history replay.
Compatible Chat Completions provider requests omit those unsupported tool images
with an explicit explanation; they never stringify encoded pixels into prompts.
Model-request telemetry records native image counts without requiring content
capture; image pixels remain excluded by genai_telemetry.
