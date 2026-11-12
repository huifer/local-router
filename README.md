# local-router

English · [中文](README.zh-CN.md)

A local AI gateway that puts all of your model providers behind one process and
gives every coding agent — Claude Code, Codex, Gemini CLI, OpenCode, or any
client that accepts a base URL — one routed, observable API.

## Features

- **One Rust binary**: gateway, React console, and CLI.
- **Protocol translation**: OpenAI Chat, OpenAI Responses, Anthropic Messages,
  and Gemini — any "inbound × upstream" combination, including streaming,
  tools, reasoning blocks, token usage, and image/video endpoints.
- **Routing groups**: order / rotate / usage / smart / weight, with failover,
  sticky sessions, nested groups, and multi-key rotation.
- **Stability**: global 429 cooldown with exponential backoff, per-key
  in-flight queueing, per-provider / per-caller RPM/TPM rate limits and daily
  token budgets.
- **Session traceability**: every session and every upstream call it makes are
  recorded as a span tree (session → request → upstream); the trace id travels
  downstream to the vendor, and the vendor's own request id is kept for
  lookups.
- **Usage & billing**: one JSONL line plus SQLite per request, USD cost and
  subscription credits, split by provider key / caller key / agent.
- **Observability exports**: Prometheus `/metrics` for live state; optional
  OTLP export (`cargo build --features otlp`) sends the span tree to
  Tempo / Jaeger.
- **Multi-node deployment**: run one gateway per machine and cascade them — an
  upstream node is just another provider. Config snapshots (export/import with
  revision-based optimistic concurrency) move between nodes;
  `local-router remote` queries a remote node's state.
- **Session-backed providers**: WorkBuddy, QwenWork, and Trae CN without API
  keys — multi-account rotation, auto renewal, auto check-in.
- **Console**: providers, models, routes, usage, sessions, projects, context,
  quotas, traces, and settings — all editable live, effective on the next
  request.
- **Docker**: multi-stage image with `/data` persistence.

This project is an independent web and CLI implementation. It does not provide
a desktop client.

## Quick start

From source (Rust 1.89+, Node.js 24+ — Node is only needed to build the
console):

```sh
npm install && npm run build --prefix frontend   # first build (cargo also runs this automatically)
cargo build --release
./target/release/local-router                    # gateway + console on the same port :3435
```

Docker:

```sh
docker compose up -d          # http://localhost:3435
# or
docker build -t local-router . && docker run -p 3435:3435 -v lr-data:/data local-router
```

## Point your agents at it

```sh
# Option 1: let the CLI rewrite each agent's own config
local-router provider add deepseek --key sk-xxx
local-router claude deepseek/deepseek-chat     # writes ~/.claude/settings.json
local-router codex  kimi/kimi-k2               # writes ~/.codex/config.toml
local-router gemini glm/glm-4.6                # writes ~/.gemini/.env
local-router opencode deepseek/deepseek-chat   # writes opencode.json

# Option 2: environment variables (works with any tool)
export OPENAI_BASE_URL=http://127.0.0.1:3435/v1      OPENAI_API_KEY=local-router
export ANTHROPIC_BASE_URL=http://127.0.0.1:3435      ANTHROPIC_API_KEY=local-router
export GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:3435  GEMINI_API_KEY=local-router
```

## CLI in one screen

```sh
local-router serve [--port N] [--addr A] [--lan] [--no-console]
local-router daemon start|stop|status|logs|install     # background / autostart
local-router provider add <preset|name> --key sk-… [--url …] [--protocol anthropic]
local-router provider list | rm | models | expose | media | test | key
local-router models                                    # exposed model catalog
local-router group add <name> --models a/m1,b/m2 --routing order
local-router usage [--period today] [--csv]            # usage and cost
local-router traces | trace <id>                       # tracing
local-router dashboard                                 # terminal dashboard
local-router doctor                                    # self-check
local-router remote status --host <url> --key <k>      # query a remote node
local-router workbuddy status|credits|login|checkin    # session-backed providers
local-router qwen status|credits|login
```

The full command reference lives in the Chinese manual,
[USAGE.md § 2.3](USAGE.md#23-命令行速查).

## Documentation

| Document | Contents |
| --- | --- |
| [USAGE.md](USAGE.md) | Manual (Chinese): setup in five minutes, daily commands, 11 scenarios, troubleshooting |
| [docs/API.md](docs/API.md) | Gateway endpoints, auth, model naming (Chinese) |
| [docs/TRACING.md](docs/TRACING.md) | Session ↔ upstream trace model and storage format (Chinese) |
| [docs/BILLING.md](docs/BILLING.md) | Usage records, cost/credits semantics, bill dimensions, SQLite queries (Chinese) |
| [docs/PROVIDERS.md](docs/PROVIDERS.md) | Session-backed providers: WorkBuddy / QwenWork / Trae CN mechanics (Chinese) |
| [docs/CONFIG.md](docs/CONFIG.md) | Field-level `config.json` reference (Chinese) |
| [docs/BENCHMARK.md](docs/BENCHMARK.md) | Throughput baseline and benchmark reproduction (Chinese) |
| [examples/multi-machine](examples/multi-machine/README.md) | Runnable two-node cascade example |
| [examples/monitoring](examples/monitoring/README.md) | Ready-made Prometheus + Grafana stack |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Contribution workflow |
| [SECURITY.md](SECURITY.md) | Security model and vulnerability reporting |
| [CHANGELOG.md](CHANGELOG.md) | Changes per version |

## Security notes

- Binds to loopback by default; with `--lan` or container exposure, `/api` and
  `/metrics` require a caller key, and write requests validate `Origin`.
- Provider keys are masked in the console; `config.json` is written with 0600;
  prompt secret masking (`masking`) stops `sk-…` / `ghp_…` / long hex strings
  from leaking upstream.
- Runtime credentials, sessions, and usage data live under `LOCAL_ROUTER_HOME`
  on your machine — nothing is committed.

Read [SECURITY.md](SECURITY.md) before exposing the service beyond loopback.

## Scope and trade-offs

**Core**: multi-provider gateway, protocol translation, routing groups, usage
recording, session/project/context observability, session-backed providers
(WorkBuddy / QwenWork / Trae CN), web console, CLI, multi-key failover,
multi-node cascade with remote operations, Prometheus/OTLP exports.

**Intentionally out of scope**: desktop menu-bar apps, a desktop client form
factor, plugin ecosystems, local PII scrubbing, QwenWork enterprise
(VPC / PAT).

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets            # unit tests in src/ + integration tests in tests/
cd frontend && npm run dev          # console hot reload (API proxied to :3435)
```

`cargo build` builds `frontend/` automatically through `build.rs`; set
`LOCAL_ROUTER_SKIP_FRONTEND=1` in npm-less environments to use a stub page.

## License

MIT
