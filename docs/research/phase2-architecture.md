# Phase 2 (v0.2) architecture additions and PR plan

> Drafted by Codex (read-only, `xhigh`) on 2026-10-04 and reviewed by the orchestrator. The owner was away and authorized the orchestrator to set specifications and proceed; the decisions taken are listed in [Orchestrator decisions](#orchestrator-decisions-2026-10-04) at the end, and the merged plan is in `HANDOFF.md`. Since this draft, Phase 1 was completed: S1.2, S1.3, S1.4, S6.2, S6.4 and S8.3 are merged (#19–#21), and the Phase 1 gate passed (agent-verified), so rows 1–6 of the PR plan below are already done.

## 1. Scope and verified baseline

Phase 2 should add OpenCode, a dormant Antigravity adapter, a loopback-only OpenAI-compatible adapter, provider detection and `pumice doctor`. Reuse the delivered Claude/Codex adapters, process runner, prompt composition, cleanup and fallback chain. **S2.4 Kimi remains on standby and has no implementation PR.**

The investigation inspected clean `main` on 2026-10-04. No files were changed, no inference calls were made, and no builds or tests were run because they create artifacts.

| Component | Current state |
|---|---|
| Provider interface | `Provider`, `FormatInput`, safe errors and `CliAdapter` already exist. |
| Registry | `register_providers!(claude, codex)`; descriptors own defaults, option validation and construction. |
| Process execution | Direct argv, fresh empty workspace, external control files, bounded output, deadlines and process-tree termination. |
| Configuration | Located YAML validation with `serde-saphyr`; provider settings already have `enabled`, `binary`, `model`, `timeout`, `env` and `options`. |
| Pipeline | Selected provider plus `fallback_order`, one active request, shared deadline, cleanup and exact raw fallback. |
| API | Chat completions and model listing exist. Models currently come from **enabled configuration**, without availability detection. |
| Health | S1.3 remains **Not started** in `HANDOFF.md`; `/health` is absent. |
| Debug logging | S1.4 remains **Not started**; settings exist, but the payload sink is not wired. |
| Isolation tests | `tests/isolation.rs` checks both existing adapters; protocol and reliability tests are spread across other files. |
| CI | Already runs tests, Clippy and release builds on Linux, Windows and macOS, with Windows static-CRT verification. |

Installed versions were checked using permitted version/help commands:

- OpenCode **1.18.34**; `opencode run --help` confirms `--pure`, `--agent`, `--format json`, `--title`, `--model provider/model` and `--variant`.
- Claude Code **2.1.288**.
- Codex CLI **0.160.0**.
- Antigravity was never executed. S0.2 records documented version **1.2.14**; its installed version remains **unverified**.

The Phase 1 Handy gate is not recorded as passed in `HANDOFF.md`. Its Windows GUI checks remain owner-only. Phase 2 planning can finish now; formally advancing the phase or release requires evidence of that gate or an explicit owner exception.

## 2. Common architecture

Preserve the existing runtime interface:

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    fn format<'a>(
        &'a self,
        input: FormatInput<'a>,
        deadline: tokio::time::Instant,
    ) -> ProviderFuture<'a>;
}
```

OpenCode and Antigravity implement the existing `CliAdapter` and run through `CliProvider<A>`. The generic adapter implements `Provider` directly.

Proposed additions:

```text
src/providers/opencode.rs
src/providers/antigravity.rs
src/providers/generic.rs
src/providers/discovery.rs
src/doctor.rs
src/logging.rs

tests/support/adapter_contract.rs
tests/support/fake_http.rs
tests/adapter_contract.rs
tests/opencode.rs
tests/antigravity.rs
tests/generic.rs
tests/discovery.rs
tests/doctor.rs
tests/privacy.rs
tests/fixtures/opencode/*
tests/fixtures/antigravity/*
tests/fixtures/generic/*
```

Each adapter owns its invocation/request construction, parser, option validation and restriction recipe. Core code continues to use string provider IDs.

Add text-free errors where existing CLI-specific categories are insufficient:

```rust
pub enum ProviderErrorCode {
    // Existing variants remain.
    InputTooLarge,
    EndpointUnavailable,
    HttpStatus(u16),
    SafetyUnavailable,
}
```

Change shared error descriptions from “CLI” to “provider” where necessary. Keep CLI-specific installation/login hints in adapter diagnostics. Never include captured output, HTTP error bodies or dictated text in `Display`, `Debug` or normal logs.

## 3. Per-adapter design

### 3.1 OpenCode — S2.6

**Default:** disabled. An explicitly configured `model` and `enabled: true` are both required. Configuring a model alone does not enable it.

**Transport:** one fresh process per dictation; user message on stdin, followed by EOF. No server/attach mode in v0.2.

Exact baseline argument array:

```rust
[
    "run",
    "--pure",
    "--agent", "pumice",
    "--format", "json",
    "--title", "Pumice",
    "--model", configured_model,
]
```

When `options.variant` is configured, append:

```rust
["--variant", configured_variant]
```

The stdin bytes are exactly:

```rust
[
    input.user_prompt.before_text,
    input.text,
    input.user_prompt.after_text,
].concat().into_bytes()
```

Stdin support was runtime-verified in S0.2. The official **v1.18.34** source also reads non-TTY stdin. The installed help confirms the proposed argument flags. [Versioned run implementation](https://raw.githubusercontent.com/anomalyco/opencode/v1.18.34/packages/opencode/src/cli/cmd/run.ts)

#### System prompt and restrictions

Retain S0.2’s inline agent configuration, serialized programmatically into the child-owned `OPENCODE_CONFIG_CONTENT`:

```json
{
  "share": "disabled",
  "snapshot": false,
  "autoupdate": false,
  "permission": "deny",
  "agent": {
    "pumice": {
      "description": "Formats dictation without taking actions.",
      "mode": "primary",
      "prompt": "COMPOSED_SYSTEM_PROMPT",
      "permission": "deny"
    }
  }
}
```

The current permission schema accepts scalar `"deny"`, and custom agents accept a prompt and primary mode. This is a documented restriction recipe; complete removal of tool definitions remains **unverified**. [Permissions](https://opencode.ai/docs/permissions/), [Agents](https://opencode.ai/docs/agents/)

Add these adapter-owned safeguards:

- Set the selected model in the inline configuration as well as argv.
- Disable automatic compaction and OpenTelemetry in inline configuration.
- Set `OPENCODE_DISABLE_PROJECT_CONFIG=1`, `OPENCODE_DISABLE_AUTOUPDATE=1`, `OPENCODE_DISABLE_MODELS_FETCH=1` and `OPENCODE_DISABLE_AUTOCOMPACT=1`.
- Remove inherited `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR` and `OPENCODE_PERMISSION`.
- Keep `--pure`; do not expose permissions, plugins, agents, tools or raw configuration as options.

The listed disable variables exist in the tagged source. Their combined enforcement in the installed binary is **unverified by runtime testing**. Do not rely on `OPENCODE_DISABLE_GLOBAL_CONFIG`: it is absent from the checked flag module. [Versioned flags](https://raw.githubusercontent.com/anomalyco/opencode/v1.18.34/packages/core/src/flag/flag.ts)

**Protect configuration substitution.** OpenCode expands `{env:…}` and `{file:…}` in raw configuration text before JSON parsing. Ordinary JSON serialization leaves those patterns intact. After serialization, encode literal opening braces inside the dynamic prompt string as `\u007b`. JSON decoding restores the original prompt while preventing the preceding substitution pass from interpreting it. Test literal file/env references, Unicode and backslashes. This defense is inferred from the tagged implementation; installed-runtime confirmation remains **unverified**. [Substitution implementation](https://raw.githubusercontent.com/anomalyco/opencode/v1.18.34/packages/opencode/src/config/variable.ts), [Configuration loading](https://raw.githubusercontent.com/anomalyco/opencode/v1.18.34/packages/opencode/src/config/config.ts)

**Isolation limits:** inline configuration merges with other configuration; managed settings can override it. `--pure` addresses external plugins, without proving suppression of global MCP startup, instructions, skills or CLI-owned transcript storage. A pre-existing `pumice` agent can also contribute merged settings. The implementation must review that collision and use a fresh adapter-owned agent name if necessary, with the same inline deny-all recipe and corresponding argv value. [Configuration precedence](https://opencode.ai/docs/config/)

Do not claim that an empty `mcp` object clears inherited servers.

#### Output parser

```rust
pub fn parse_output(
    output: &ProcessOutput,
) -> Result<String, ProviderError>;
```

Parse strict UTF-8 JSONL:

1. Require exit success and a nonempty, valid event stream.
2. Reject any `error` event, even with exit code zero.
3. Reject `tool_use` and any tool part as `UnexpectedToolActivity`.
4. Collect completed `text` parts for the final assistant message, preserving part order and avoiding duplicate part IDs.
5. Require a corresponding successful final `step_finish`; reject tool-call or truncated completion reasons.
6. Ignore reasoning, token usage and progress.
7. Reject malformed/truncated JSONL, inconsistent session/message IDs and ambiguous completion.

The checked implementation emits completed text parts, tool events, step events and structured errors; it does not emit Codex’s `turn.completed`. Completion ordering and exact final-reason fixtures need further source-level confirmation during implementation. Synthetic fixtures must be marked **documentation-derived**, not recorded real output.

#### Error classification and allow-lists

| Condition | Classification |
|---|---|
| Resolver cannot find executable/runtime | `NotInstalled` |
| Structured `ProviderAuthError` indicating missing authentication | `NotLoggedIn` |
| Structured 401/403 rejection | `AuthenticationRejected` |
| Structured 429 | `RateLimited` |
| Recognized structured quota code | `QuotaExceeded` |
| Missing/invalid model configuration | `InvalidConfiguration` |
| Runner deadline | `Timeout` |
| Malformed/incomplete output | `InvalidOutput` |
| Tool activity | `UnexpectedToolActivity` |
| Unknown failure | `NonzeroExit` |

Authentication/API error schemas are documented in tagged source; provider-specific runtime signatures remain **unverified**. Classify only failure records, never successful text. [Versioned error schemas](https://raw.githubusercontent.com/anomalyco/opencode/v1.18.34/packages/core/src/v1/session.ts)

Allow only:

- Common settings: `enabled`, `binary`, `model`, `timeout_secs`.
- Options: `variant`.
- YAML environment overrides: **none**.
- Adapter-owned environment: the fixed configuration and disable variables above.

Reject arbitrary argv, attach/session/continue, file attachments, custom agents, sharing and permission overrides. Prefer native OpenCode executables; refuse Windows wrappers until their exact layout has checked fixtures.

Zen free models are valid only through an explicit provider/model setting. Never discover or select a hosted default. OpenCode/Kimi remains outside real verification, and explicit model selection does not resolve upstream subscription terms.

### 3.2 Antigravity — S2.5

**Default:** disabled, with a prominent risk warning. No `agy` invocation is permitted during investigation, automated testing, detection or the unattended gate.

Build the protocol adapter against the fake CLI from S0.2 and public documentation. **Do not silently equate fake protocol coverage with verified safe execution.**

#### Preferred transport

Use one fresh process and one stream-JSON input message:

```rust
[
    "--input-format", "stream-json",
    "--output-format", "stream-json",
    "--print-timeout", "30s",
    "--sandbox",
    "--model", configured_model,
    "--effort", "low",
]
```

Serialize one newline-terminated event:

```json
{
  "event": "user",
  "message": {
    "content": "COMPOSED_SYSTEM_INSTRUCTIONS_AND_USER_MESSAGE"
  }
}
```

Close stdin after writing. Never reuse the conversation.

This documented transport avoids dictated text in argv and Windows command-line limits. EOF/process-exit behavior under the complete proposed invocation remains **unverified**. Do not retry through another transport automatically. [Headless documentation](https://antigravity.google/docs/cli/headless/)

No verified separate system-prompt replacement flag exists. Use E3.3’s merged-input fallback, with the composed system instructions preceding the existing delimited user message. Custom-agent loading and complete base-prompt replacement remain **unverified**.

#### Tool restrictions and execution eligibility

S0.2’s proposed policy is:

```json
{
  "toolPermission": "strict",
  "permissions": {
    "deny": [
      "read_file(*)",
      "write_file(*)",
      "read_url(*)",
      "execute_url(*)",
      "command(*)",
      "unsandboxed(*)",
      "mcp(*)"
    ]
  }
}
```

The public docs define these action namespaces and deny precedence. They place CLI permissions in global settings. `strict` and `--sandbox` alone do not remove tools or deny workspace reads/writes. [CLI permissions](https://antigravity.google/docs/permissions?tab=cli), [Settings reference](https://antigravity.google/docs/cli/reference/)

**No verified per-launch settings override was found.** Writing this policy into an unused control file would provide no protection.

Recommended production behavior:

- Ship the adapter disabled.
- Keep real execution ineligible until a documented, implementable policy-loading preflight exists.
- Reject attempted enablement at the YAML `enabled` location with a fixed safety explanation.
- Implement invocation construction and parsing independently so fake tests exercise the protocol.
- Do not add a public `test_mode`, unsafe bypass or global-settings mutation.

An owner-configured global policy could be evaluated later, but its loading, startup customizations and race with changing settings require separate acceptance. This is an explicit unresolved S2.5 condition.

#### Parser and errors

Parse strict NDJSON and require:

- One initialization record.
- Exactly one terminal result for the submitted turn.
- Exit success, terminal `status == "SUCCESS"` and string `result.response`.
- No tool step.
- No malformed stream, contradictory results or incomplete terminal state.

Return only `result.response`; ignore text deltas, reasoning, checkpoints and usage.

Use `NotInstalled` from the resolver, `Timeout` from the runner, documented “authentication required” failure evidence for `NotLoggedIn`, and conservative structured failure mapping. Quota signatures remain **unverified**. Unknown errors stay text-free `NonzeroExit`.

Allow common settings and, initially, no provider options or YAML environment overrides. Keep effort fixed at `low`. Reject permission bypass, resume, agent/plugin customization and arbitrary argv.

#### Argument-mode alternative

If stream stdin cannot later be supported, the documented argument candidate is:

```rust
[
    "-p", merged_input,
    "--output-format", "stream-json",
    "--print-timeout", "30s",
    "--sandbox",
    "--model", configured_model,
    "--effort", "low",
]
```

Before spawning, enforce **30,000 UTF-16 code units for the entire quoted Windows command line**, including executable, translated-runtime prefix arguments, separators and terminating NUL. Count the serialized command line, not transcript characters. Reject NUL-containing arguments.

On overflow, return `InputTooLarge`; the existing pipeline tries configured fallbacks and eventually returns the exact raw transcript. Never truncate. Windows’ documented maximum is 32,767 characters. [CreateProcessW limit](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)

### 3.3 Generic OpenAI-compatible adapter — S2.7

**Default:** disabled. Require explicit `model` and `options.base_url` when enabled.

**Policy finding:** loopback HTTP is still a direct outbound call under the current AGENTS.md wording. The proposed restriction narrows the conflict but does not eliminate it. Prepare the implementation plan and fake tests; merge executable HTTP support only after the owner adopts the narrowly scoped exception described below.

#### HTTP client choice

Use Hyper directly over a validated Tokio socket:

```toml
hyper = {
    version = "1.11.1",
    default-features = false,
    features = ["client", "http1"],
}
hyper-util = {
    version = "0.1.21",
    default-features = false,
    features = ["tokio"],
}
http-body-util = {
    version = "0.1.5",
    default-features = false,
}
```

These versions already appear in `Cargo.lock`. Enable Hyper’s client feature and use `hyper-util` only for `TokioIo`; no legacy client, proxy, pool, HTTP/2 or TLS features. The client feature adds dependencies, including `want`, so the dependency graph still needs review. [Hyper features](https://docs.rs/crate/hyper/1.11.1/features), [Tokio adapter](https://docs.rs/hyper-util/0.1.21/hyper_util/rt/tokio/index.html)

| Alternative checked | Recommendation |
|---|---|
| Hyper 1.11.1 + hyper-util 0.1.21 + http-body-util 0.1.5 | Preferred: existing stack, maintained HTTP framing, explicit socket ownership. |
| reqwest 0.13.5 with defaults disabled | Viable, but adds more client, URL, redirect and proxy machinery to constrain. |
| Hand-written HTTP/1.1 | Reject: unnecessary responsibility for chunking, truncation, conflicting lengths, headers and connection lifecycle. |

The reqwest comparison was checked against its documented features and builder behavior. Binary-size differences remain **unmeasured**. [reqwest features](https://docs.rs/crate/reqwest/0.13.5/features), [ClientBuilder](https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html)

#### Endpoint validation

```rust
pub struct LoopbackEndpoint {
    pub addr: std::net::SocketAddr,
    pub authority: String,
    pub chat_path: &'static str,
}

pub fn parse_endpoint(
    value: &str,
) -> Result<LoopbackEndpoint, EndpointError>;
```

Initial accepted forms:

```text
http://127.0.0.1:<port>/v1
http://localhost:<port>/v1
http://[::1]:<port>/v1
```

Allow an optional trailing slash. Require an explicit decimal port, 1–65535.

Map literal `localhost` directly to `127.0.0.1`; never consult DNS or the hosts file. IPv6 uses `[::1]` explicitly. Validate the raw authority, rejecting userinfo, percent-encoded hosts, alternate numeric IP spellings, zone identifiers, other hosts, query, fragment, backslashes and control characters.

Connect using `TcpStream::connect(SocketAddr)`. Do not follow redirects, use proxies, retry inference, negotiate upgrades or pool connections. Reject the configured Pumice listening endpoint to prevent recursive requests.

This constrains Pumice’s destination. It does **not** guarantee local inference: Ollama supports cloud models through its local endpoint. Document owner-controlled local-only configuration without reading or modifying it. [Ollama compatibility](https://docs.ollama.com/api/openai-compatibility), [Ollama local-only settings](https://docs.ollama.com/faq)

#### Exact request and prompt separation

Send:

```text
POST /v1/chat/completions HTTP/1.1
Host: <validated authority>
Content-Type: application/json
Accept: application/json
Accept-Encoding: identity
Connection: close
Content-Length: <serialized byte length>
```

Body:

```json
{
  "model": "CONFIGURED_MODEL",
  "stream": false,
  "messages": [
    {"role": "system", "content": "COMPOSED_SYSTEM_PROMPT"},
    {"role": "user", "content": "BEFORE_TEXT_AND_DICTATION_AND_AFTER_TEXT"}
  ]
}
```

Omit tools, functions, integrations and assistant/tool histories. Never execute returned tool calls. Omitting tool definitions avoids depending on differing `tool_choice` implementations; LM Studio documents client-executed tool calls, while Ollama’s retrieved compatibility documentation and converter differ on `tool_choice`. [LM Studio tool contract](https://lmstudio.ai/docs/developer/openai-compat/tools), [Ollama converter](https://raw.githubusercontent.com/ollama/ollama/main/openai/openai.go)

Allow only `options.base_url`, plus common enable/model/timeout settings. Reject `binary`, nonempty `env`, API keys, custom headers, proxy configuration and arbitrary request-body options.

#### Lifecycle, parser and errors

```rust
pub struct GenericProvider {
    endpoint: LoopbackEndpoint,
    model: String,
    timeout: std::time::Duration,
}

impl Provider for GenericProvider {
    // Existing Provider signatures.
}
```

Use `hyper::client::conn::http1::Builder` with a 64-header limit and 64 KiB read buffer. Drive its connection future inside the provider call, alongside the exchange future. Cancellation must drop the connection and socket; do not detach a driver task. [HTTP/1 builder](https://docs.rs/hyper/1.11.1/hyper/client/conn/http1/struct.Builder.html)

Proposed bounds:

- Serialized request: 10 MiB, enforced by a capped serialization writer.
- Successful response: 10 MiB.
- Error response: 64 KiB.
- Absolute deadline covers serialization, connect, upload, headers, body and parsing.
- Optional two-second connect cap sits inside that deadline.
- Count actual body bytes regardless of `Content-Length`; support normal chunked framing through Hyper.
- Reject compressed/SSE responses, unexpected trailers, malformed framing and truncated bodies.

Accept HTTP 200 with one assistant choice, string content and successful final completion. Reject missing/null content, refusal, truncation and nonempty tool/function calls. Return content unchanged for pipeline cleanup.

| Condition | Safe classification |
|---|---|
| Refused/unreachable endpoint | `EndpointUnavailable` |
| Deadline or HTTP 408/504 | `Timeout` |
| HTTP 401/403 | `AuthenticationRejected`; keyed endpoints are unsupported |
| HTTP 429 | `RateLimited`, unless a recognized structured quota code applies |
| Redirect or other non-200 | `HttpStatus(status)` |
| Malformed/truncated response | `InvalidOutput` |
| Oversized response | `OutputTooLarge` |
| Oversized serialized request | `InputTooLarge` |
| Returned tool/function call | `UnexpectedToolActivity` |

`NotInstalled` and `NotLoggedIn` are CLI-specific and do not describe this transport.

## 4. Registry and configuration

### Descriptor additions

Extend descriptors without replacing the registry:

```rust
pub struct ProviderDescriptor {
    // Existing fields remain.
    pub disabled_by_default: bool,
    pub risk_warning: Option<&'static str>,
    pub probe: ProbeSpec,
    pub validate_settings: ValidateSettingsFn,
}

pub type ValidateSettingsFn =
    fn(&ProviderSettings, &ProviderLocations) -> Result<(), ConfigError>;
```

`ProviderLocations` retains the provider key, `enabled`, `model`, `binary` and option locations. Run full-settings validation after applying defaults and overrides, including when no YAML entry exists.

The existing `validate_options` hook cannot detect “enabled OpenCode with omitted model”; full-settings validation must see both fields.

| Provider | Disabled by default | Required when enabled |
|---|---:|---|
| Claude | No | Existing defaults |
| Codex | No | Existing defaults |
| OpenCode | Yes | Explicit nonempty `provider/model` |
| Antigravity | Yes | Supported safety preflight; recommend explicit model |
| Generic | Yes | Explicit model and validated loopback base URL |

For OpenCode, validate a nonempty provider prefix and model suffix separated by `/`; reject whitespace/control characters and leading option syntax. Preserve model suffixes rather than assuming exactly one slash. Unknown upstream models must fail, without trying another model.

Missing required fields should point to `enabled: true`; malformed supplied values should point to their own value. Validate adapter constructors again to protect direct library use.

Recommended warnings:

- **OpenCode:** “Disabled by default. Configure an explicit provider/model. Upstream terms vary; startup customizations and CLI-owned transcript retention remain unverified.”
- **Antigravity:** “Experimental and disabled by default. Third-party integration may risk account access. Tool/startup isolation and transcript suppression are unverified.”
- **Generic:** “Disabled by default. Connects only to an unauthenticated loopback HTTP endpoint. The local server controls inference routing and retention.”

Show warnings in `doctor`, `check-config`, documentation and startup when relevant. Never return them as formatted dictation.

### Enabled versus available

Keep configuration intent separate from detection:

```rust
pub struct ProviderStatus {
    pub enabled: bool,
    pub availability: Availability,
    pub version: VersionStatus,
    pub execution: ExecutionStatus,
}
```

Recommended states:

- Availability: `Found`, `Missing`, `UnsupportedShim`, `EndpointReachable`, `EndpointUnavailable`, `NotProbed`.
- Version: parsed version, probe failure/timeout, intentionally skipped, not applicable.
- Execution: eligible or a safe reason such as disabled, unsupported restrictions or pending policy exception.

`/v1/models` lists only **enabled, available and execution-eligible** provider IDs. It never advertises backend model names.

Finding a CLI never enables it. Supplying an OpenCode model never enables it. Antigravity discovery never makes it executable.

## 5. Auto-detection and `doctor` — S2.8

### Startup detection

Reuse `resolve_program` from `src/process/resolve.rs`, including absolute PATH entries and supported Windows shim translation.

Add descriptor-owned probe metadata:

```rust
pub enum ProbeSpec {
    Cli {
        program: fn(&ProviderSettings) -> ProgramSpec,
        version_args: &'static [&'static str],
    },
    PathOnly {
        program: fn(&ProviderSettings) -> ProgramSpec,
    },
    LoopbackHttp,
}
```

Use:

- Claude, Codex and OpenCode: `["--version"]`.
- Antigravity: **PATH lookup only; never spawn `agy`**.
- Kimi: outside the active registry; optionally display “standby” as a static doctor note.
- Generic: when enabled and authorized by the loopback exception, a short TCP-connect check; no inference request.

Version probes use the existing process-tree lifecycle, an empty workspace, closed stdin, a **two-second timeout** and small output limits, proposed at 4 KiB. Extract only a bounded version token; never print arbitrary probe stderr.

Cache detection once per service startup. `/v1/models`, health and request selection read the cache. `doctor` performs a fresh scan. No background polling or persistent cache file is needed.

Retain the resolved program so startup detection and later execution use the same selected path. Reuse a narrow runner entry point rather than implementing another spawning path:

```rust
pub async fn run_resolved(
    &self,
    program: &ResolvedProgram,
    invocation: CliInvocation,
    deadline: Instant,
) -> Result<ProcessOutput, ProviderError>;
```

A failed version probe means “found, version unavailable,” not “missing.” Version output proves neither login nor upstream model availability. Unknown versions require a compatibility warning; adapters still enforce mandatory restrictions.

### Selection and fallback

Discovery must not accidentally break Phase 1 behavior.

Currently, omitting a built provider makes the pipeline treat it as disabled and return raw immediately. Therefore:

- Unknown selected ID → immediate raw fallback.
- Disabled selected ID → immediate raw fallback.
- Enabled selected provider with a missing executable → safe `NotInstalled` failure, then the existing fallback chain.
- Enabled HTTP provider unavailable → safe endpoint failure, then fallback.
- Missing/disabled fallback entries → skip without spawning.
- Provider disappears after startup → runner failure still enters fallback.
- Explicitly unsupported safety configuration → startup config error.

Keep `attempts` counting actual formatting attempts; availability-only skips should not count as spawned calls.

### Doctor interface

```text
pumice doctor [--config <path>]
pumice doctor [--config <path>] --login-check [--provider <id>]
```

Default output shows provider, found/missing state, version, enabled state, eligibility, restriction warnings and official install hints. Use static links to official installation docs; never install or log in automatically.

`--login-check` is the sole quota-bearing diagnostic path. It should:

- State that inference spends quota.
- Use the adapter’s normal restrictions, workspace and timeout.
- Perform one fixed formatting probe per explicitly selected eligible provider.
- Report a text-free result without printing the generated response.
- Avoid fallback, which would spend quota on another provider unexpectedly.
- Skip Antigravity and standby Kimi.
- Skip the currently configured OpenCode/Kimi route.

For the unattended Phase 2 verification, only Claude and Codex are eligible for optional real probes, and only in a separately authorized verification session.

## 6. Shared adapter contract suite

Extend the existing fake infrastructure; do not replace it.

Use generic test functions with a macro that creates separate named tests per adapter:

```rust
pub trait ContractFactory {
    fn id() -> &'static str;
    fn capabilities() -> ContractCapabilities;

    fn harness(
        case: ContractCase,
    ) -> ContractHarnessFuture;
}

pub async fn run_case<F: ContractFactory>(
    case: ContractCase,
);

adapter_contract!(claude, ClaudeContract);
adapter_contract!(codex, CodexContract);
adapter_contract!(opencode, OpenCodeContract);
adapter_contract!(antigravity, AntigravityContract);
adapter_contract!(generic, GenericContract);
```

Each factory supplies protocol-specific responses and assertions. The common tests own transport, timeout, privacy and fallback expectations.

CLI factories use the existing disposable `FakeCli`, scenario JSON and reports. HTTP factories use owned ephemeral listeners: axum for valid JSON responses and raw Tokio TCP for malformed framing. No test discovers or calls real providers.

| Contract | Required evidence |
|---|---|
| Final text only | Reasoning, progress, wrappers and diagnostics never enter parsed output. |
| No tools / restricted exception | Assert complete restriction recipe; reject observed tool activity. State whether tool removal or restricted mode applies. |
| Empty workspace | Every CLI call starts empty; successive calls use different roots; controls remain outside cwd. |
| Transport | Exact stdin/argument bytes, Unicode, shell metacharacters, long input and EOF behavior. |
| System separation | Separate file/config/message for supported transports; explicit merged-input test for Antigravity. |
| Missing backend | CLI `NotInstalled`; HTTP endpoint unavailable equivalent. |
| Authentication | Protocol-specific missing-login or authentication-rejection fixtures; no heuristic on successful text. |
| Timeout | Unread stdin/upload, delayed output, trickle body and retained pipes remain bounded. |
| Invalid output | Malformed, incomplete, contradictory and empty output fails safely. |
| Cleanup | Temporary roots/control files removed; descendants terminated; HTTP sockets/tasks closed. |
| Fallback | Each adapter works as selected provider and fallback; failure advances the chain; all failures return exact raw text. |
| Privacy | Unique markers in input, stdout, stderr and error bodies never appear in safe errors, normal logs or debug formatting. |
| Disabled behavior | Zero formatting processes/connections. |
| Config protection | Missing model, forbidden options/env and security overrides rejected with located errors. |

For HTTP, do not create a meaningless temporary cwd or claim control over the external server’s workspace. The equivalent assertions are stateless request construction, no tools/credentials and no client-owned persisted state.

Antigravity needs two distinct checks:

1. Invocation/parser contract against the fake CLI.
2. Production safety preflight refuses execution while policy loading is unsupported.

The suite must record that limitation explicitly. Passing the second check proves refusal is safe, not that real Antigravity tool isolation works.

Add focused HTTP tests for redirects with a second listener receiving zero requests, proxy bypass, raw-host validation, malformed/chunked framing, overflow, compressed/SSE responses, recursion and cancellation.

Reuse synthetic dictations for language, lists, corrections and command-as-data cases. They seed S8.1; they do not complete its owner-provided real-sample requirement.

**Implemented location (S8.2 follow-up).** The suite lives in `tests/support/adapter_contract.rs` — the `ContractAdapter` trait (one small struct per adapter supplying protocol fixtures, system-prompt capture keys and restriction assertions), one generic async function per contract case, and the `adapter_contract!` macro that expands to one named `#[tokio::test]` per case inside an adapter-named module, so failures stay individually named (`claude::timeout`, `codex::privacy`, …). It runs from `tests/adapter_contract.rs`, which today holds `adapter_contract!(claude, ClaudeContract);` and `adapter_contract!(codex, CodexContract);`. The trait is named `ContractAdapter` rather than the `ContractFactory` sketch above, and its fixture builders return the fake CLI's `(stdout, exit_code)` pair because CLI adapters are driven through the existing `FakeCli`; later HTTP adapters can implement the same trait against fake listeners. A new adapter plugs in by implementing `ContractAdapter` and adding one macro line. `tests/isolation.rs` keeps only what is adapter-specific: the exact argv lists and the Windows shim tests.

## 7. Ordered PR plan

Each PR covers one story, uses Conventional Commits and updates permitted `HANDOFF.md` status/notes. Do not edit AGENTS.md or either product/spec document. Record contradictions in research **Questions for the owner** and the relevant PR description.

Completed stories below are explicitly scoped follow-ups, not duplicate implementations.

| Order | Story and branch | Scope and files | Acceptance tests | Dependencies |
|---:|---|---|---|---|
| 1 | **S1.3**, `s1.3-health` | `/health` in `src/api/*`; `tests/api.rs`. | HTTP 200 while a fake provider hangs; zero provider calls. | Existing API; close S1.1 review status. |
| 2 | **S1.4**, `s1.4-debug-log` | Opt-in payload sink in `src/logging.rs`, API wiring, `tests/privacy.rs`. | Off means no payload file/text; on captures intended payloads; normal errors/logs remain text-free; credentials redacted. | Existing config/API. |
| 3 | **S1.2**, `s1.2-provider-models` | Finish configured model-list contract and tests. | Enabled configured IDs listed; disabled IDs omitted; listing invokes no CLI. | Existing API. |
| 4 | **S6.2**, `s6.2-provider-selection` | Finish selection contract using existing pipeline; avoid rewriting it. | Explicit IDs, empty model/default, unknown/disabled raw behavior; fake backend chosen correctly. | 3. |
| 5 | **S6.4**, `s6.4-example-config` | Phase 1 example and README. | Example parses; defaults, prompts, routing and debug settings agree with behavior. | 1–4. |
| 6 | **S8.3**, `s8.3-phase1-ci` | Close CI/story status and document remaining Phase 1 gate evidence. | Three-platform suite/release/static-CRT checks; no real CLI in tests. | Phase 1 implementation. Owner Handy gate remains separate. |
| 7 | **S8.2 follow-up**, `s8.2-adapter-contract` | Shared harness in `tests/support/adapter_contract.rs`; migrate common assertions from isolation/adapter tests. | Claude and Codex run every applicable contract case; meaningful failures remain individually named. | Existing fake CLI. |
| 8 | **S2.1 follow-up**, `s2.1-provider-capabilities` | Descriptor defaults/warnings/probes/full-settings validator; located config context. | Registry/default consistency; required-field locations; existing Claude/Codex behavior preserved. | Existing registry/config. |
| 9 | **S2.8, part 1**, `s2.8-provider-detection` | `discovery.rs`, resolved-path reuse, bounded probes, cached status, models/selection integration. | Missing/version-timeout/shim cases; models intersection; missing selected provider still permits fallback; Antigravity never spawned. | 7–8. |
| 10 | **S2.6**, `s2.6-opencode-adapter` | New module/registry entry, explicit-model validation, inline restrictions, JSONL parser and fixtures. | Entire contract suite; no-model rejection; substitution literals; error/tool events; long stdin; no hosted-default path. | 7–9. |
| 11 | **S2.5**, `s2.5-antigravity-adapter` | Documentation-derived invocation/parser, dormant registry entry, warning and safety preflight. | Fake protocol suite, long stream input, tool/error rejection, enablement refusal, zero real `agy` execution. | 7–9. Mark unresolved execution eligibility in status notes. |
| 12 | **S2.7**, `s2.7-loopback-adapter` | `generic.rs`, Hyper client features, endpoint validator and fake HTTP support. | HTTP contract, address bypasses, redirects/proxies, bounds, deadlines, cancellation and mixed fallback. | 7–9; owner-approved outbound exception required before merge. |
| 13 | **S2.8, part 2**, `s2.8-doctor` | `doctor.rs`, CLI parsing, install hints, warning/status output and explicit login-check path. | Default doctor performs version-only probes; explicit checks use one selected fake; risk/standby providers skipped. | 9; adapter descriptors from 10–12 as merged. Completes S2.8. |
| 14 | **S6.4 follow-up**, `s6.4-phase2-config` | Disabled Phase 2 examples, explicit model/local endpoint instructions, doctor/privacy documentation. | Examples validate; unsupported/keyed/remote variants fail; no example silently enables a provider. | 10–13. |
| 15 | **S8.3 follow-up**, `s8.3-phase2-gate` | Final contract matrix, CI integration and gate evidence/status notes. | All eligible implementation contracts pass across three OSes; disabled/default and privacy assertions; preserved static CRT. | All Phase 2 implementation; unresolved gate conditions recorded. |

Parallel work:

- S1.3, S1.4 and S1.2 can develop independently.
- Shared contract work and descriptor/config metadata can proceed in parallel.
- After their foundations, OpenCode, Antigravity and generic protocol work can develop in parallel.
- Doctor formatting/CLI parsing can develop alongside adapters against agreed status types.
- Serialize registry, manifest/lockfile and HANDOFF merges.
- Do not add new Claude/Codex adapter or fallback-chain stories; those are delivered.

## 8. Verification without owner action

### Automated engineering gate

In a future implementation session with write authorization, run:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked --bin pumice
```

CI repeats relevant checks on Linux, Windows and macOS and preserves the static-CRT inspection.

Gate evidence must include:

- Shared contract matrix for every implemented adapter.
- Fake-only API/model-selection/fallback integration.
- Default-disabled providers produce zero calls.
- Doctor/detection are bounded and quota-free.
- Text-free errors and normal logs.
- Cleanup, cancellation and total-budget evidence.
- Explicit **documentation-derived/unverified** labels for new CLI protocols.

No real inference belongs in CI or ignored tests that run automatically.

### Optional agent-run real verification

This investigation authorizes **zero prompt calls**. In a separately authorized verification session, allow at most:

| Adapter | Maximum real calls | Conditions |
|---|---:|---|
| Claude | 1 | Existing verified restrictions and configuration; no login or routing changes. |
| Codex | 1 | Existing verified restrictions and explicit routing; no login changes. |
| OpenCode | 0 | Current configured upstream is standby Kimi. |
| Antigravity | 0 | Never execute `agy`, including version/help. |
| Kimi | 0 | Standby. |
| Generic | 0 required | Fake HTTP server establishes the automated contract. |

Use one short synthetic Portuguese sample with a list, technical term and command-like sentence. Record version, elapsed time and redacted outcome. No retries, deliberate quota exhaustion or real-provider fallback experiments.

### What the gate can establish

The unattended gate can establish **implementation and transport conformance**. It cannot establish new-provider formatting quality, complete startup isolation or account safety.

Under the literal “all adapters pass the same criteria” requirement, Antigravity’s unresolved policy loading prevents claiming full executable conformance. Report its protocol coverage and safe disabled behavior separately. Do not mark the full Phase 2 product gate complete by relabeling that gap as a pass.

### Later owner-only checks

- Record the Phase 1 Handy gate with Claude, Codex and raw fallback.
- Approve the narrowly scoped loopback HTTP exception.
- Select an authorized OpenCode upstream and confirm explicit model routing.
- Decide OpenCode startup-isolation and CLI-owned transcript acceptance.
- Decide whether Antigravity remains dormant or receives a separately authorized safety evaluation.
- Check real Windows CLI layouts and Handy behavior.
- Validate local model quality and local-server retention/cloud settings.
- Supply S8.1’s real dictation samples.

## 9. Decisions proposed in the owner’s absence

These recommendations are reportable design decisions; policy exceptions remain unapproved.

| Decision | Recommendation |
|---|---|
| Phase 2 scope | Add OpenCode, dormant Antigravity, loopback generic and detection/doctor. Keep Kimi on standby; retain delivered Claude/Codex/fallback. |
| Generic outbound conflict | Approve a narrow exception for explicit, credential-free HTTP requests to validated loopback endpoints. Defer remote/keyed APIs. Until then, prepare but do not merge executable HTTP support. |
| HTTP implementation | Direct Hyper HTTP/1 client with checked locked versions; no TLS, proxy, redirects or handwritten framing. |
| New-provider activation | Explicit `enabled: true`; configuration or detection never enables a provider automatically. |
| OpenCode defaults | No default model. Require explicit provider/model; Zen free models require the same explicit choice. |
| Generic defaults | No guessed model/backend. Require model and base URL on enablement. |
| Generic endpoint scope | HTTP, explicit port, `/v1`, exact supported loopback spellings; pin `localhost` to IPv4. |
| Antigravity transport | Prefer one-shot stream JSON; use complete-command-line length validation only if argument transport later becomes necessary. |
| Antigravity safety | Keep production execution ineligible until supported policy loading is established; fake protocol tests may proceed. |
| Antigravity detection | PATH-only, with version deliberately skipped, overriding the spec’s general version-probe wording. |
| Availability semantics | Distinguish enabled, found and execution-eligible; models list their intersection. Preserve fallback for enabled missing providers. |
| Version-probe behavior | Two-second bounded probes cached at startup; version failure is distinct from missing installation and login state. |
| Doctor login checks | Explicit quota-bearing path, one selected provider, no fallback; risky/standby routes remain skipped. |
| Gate interpretation | Publish an automated conformance matrix and unresolved conditions. Do not infer complete real isolation or formatting quality from fake tests. |
| Phase sequencing | Finish pending Phase 1 stories first; do not assert its Handy gate passed without evidence. |

Record these in research **Questions for the owner** and PR descriptions. Product/spec changes remain owner-controlled.

## 10. Risks and open questions

| Risk or question | Handling |
|---|---|
| OpenCode settings merge can retain or override restrictions. | Review agent collisions and tagged merge behavior; keep disabled by default; document unresolved startup isolation. |
| OpenCode config expansion can read files or environment values through prompt literals. | Protect serialized dynamic strings; add hostile-literal fixtures. Installed-runtime behavior remains unverified. |
| Antigravity has no verified per-launch deny-policy transport. | Refuse production enablement until a supported mechanism exists; do not invent a flag or unused control file. |
| CLI-owned transcripts violate a broad reading of privacy requirements. | Distinguish Pumice logging from CLI persistence; keep the finding open for owner acceptance. |
| Loopback server can relay to cloud inference or retain dictation. | Promise destination restriction only; document owner-managed server behavior. |
| Fake fixtures may lag real event schemas. | Pin fixtures to examined versions and mark documentation-derived evidence; fail safely on ambiguous output. |
| Unknown versions or managed configuration can alter behavior. | Report compatibility uncertainty; mandatory restrictions remain enforced, without silently relaxing them. |
| Cleanup after cancellation can race Windows file handles. | Extend contract assertions to root removal and descendant termination; fix demonstrated failures before the gate. |
| Bounded synchronous JSON work can still consume deadline time. | Check deadlines before/after serialization/parsing and measure worst-case fixture behavior. |
| Cached availability becomes stale. | Preserve runtime failure/fallback; restart refreshes service state and doctor performs a fresh scan. |
| Current Phase 1 status and spec phasing lag delivered work. | Update permitted status/notes accurately; leave headings and product/spec corrections to the owner. |
| Full gate completion conflicts with unresolved dormant-adapter safety. | Seek an explicit scope/gate decision later; retain the unresolved status now. |
## Orchestrator decisions (2026-10-04)

The owner authorized setting specifications in their absence, to be reported afterwards. Each decision below is binding for Phase 2 until the owner says otherwise.

1. **Scope:** OpenCode (S2.6), dormant Antigravity (S2.5), the loopback-only generic adapter (S2.7), detection and `pumice doctor` (S2.8), the shared adapter contract suite, and the Phase 2 gate. Kimi (S2.4) stays on standby.
2. **New providers are off by default.** Only `enabled: true` turns one on. Detection or a configured model never enables a provider.
3. **OpenCode:** no default model. When enabled, an explicit `provider/model` is required, and Zen free models need the same explicit choice. It uses the inline deny-all agent, `--pure`, and the adapter-owned disable variables. Config-substitution literals (`{env:…}`, `{file:…}`) in dynamic prompt text are escaped.
4. **Antigravity:** the adapter (invocation and parser) is built and tested only against the fake CLI. Turning it on is **refused** with a fixed safety message until a supported per-launch tool-policy mechanism exists. `agy` is never spawned, not even for `--version`: detection is PATH-only.
5. **Generic adapter (S2.7):** it conflicts with AGENTS.md ("No outbound calls other than the ones the AI CLIs make themselves"), and agents may not edit AGENTS.md. Its PR is **built and opened but not merged** until the owner approves a narrow exception for unauthenticated loopback endpoints. The design is Hyper HTTP/1, loopback only, with no TLS, proxies or redirects.
6. **Detection:** version probes have a 2 s timeout, never spend quota, and are cached at startup. `/v1/models` lists providers that are enabled, found and eligible to run. An enabled provider that is missing still fails at runtime, so the fallback chain still applies.
7. **`pumice doctor --login-check`** is the only quota-bearing diagnostic. It checks one explicitly selected provider, with no fallback. Antigravity and Kimi are skipped.
8. **Phase 2 gate** (unattended): the full contract suite passes for every implemented adapter on 3 OSes, plus at most one real call each for Claude and Codex. Antigravity counts as **protocol coverage plus safe refusal**, not executable conformance. That gap is recorded rather than relabeled as a pass.
