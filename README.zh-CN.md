# local-router

[English](README.md) · 中文

一个本地 AI 网关：把你的所有模型服务商收进一个进程，给每个编码 agent
（Claude Code / Codex / Gemini CLI / OpenCode / 任何吃 base URL 的工具）
提供统一、可路由、可观测的模型接口。

## 特性

- **Rust 单二进制**：网关 + React 控制台 + CLI，一个文件跑起来
- **四种协议互译**：OpenAI Chat / OpenAI Responses / Anthropic / Gemini，
  任意「入口 × 上游」组合（流式、工具调用、思考块、token 用量、图片视频）
- **路由组**：order / rotate / usage / smart / weight，故障转移、会话粘性、
  嵌套组、多 key 轮试
- **稳定性**：上游 429 全局冷却（指数退避）、per-key 并发排队、
  per-provider / per-caller 的 RPM/TPM 限流与日预算
- **链路追踪**：每个会话与它的每次上游调用都记成 span 树
  （session → request → upstream），trace id 随请求下发到厂商，
  厂商自己的 request id 也留档可反查
- **用量与账本**：每请求一行 JSONL + SQLite，成本（USD）与订阅 credits
  双口径，按上游 key / 调用方 key / agent 三个维度分账
- **可观测性出口**：Prometheus `/metrics` 实时指标；OTLP 导出
  （`cargo build --features otlp`）把 span 树送到 Tempo / Jaeger
- **多节点部署**：每台机器跑一个网关，级联起来——上游节点就是一个普通
  provider，配置快照（导出/导入 + revision 乐观并发）在节点间迁移，
  `local-router remote` 直接查远端状态
- **登录态上游**：WorkBuddy、QwenWork（千问办公）、Trae CN 免 API key
  接入，多账号轮转、自动续期、自动签到
- **控制台**：服务商 / 模型 / 路由 / 用量 / 会话 / 项目 / 上下文 / 额度 /
  链路 / 设置，全部在线改、下一个请求即生效
- **Docker**：多阶段构建，`/data` 持久化，compose 一键起

本项目为独立实现，只做网页模式 + CLI，不做桌面客户端。

## 快速开始

本地运行（需要 Rust 1.89+ 与 Node.js 24+，Node 仅构建控制台用）：

```sh
npm install && npm run build --prefix frontend   # 首次构建控制台（cargo 也会自动调）
cargo build --release
./target/release/local-router                    # 网关 + 控制台，同一个端口 :3435
```

Docker：

```sh
docker compose up -d          # http://localhost:3435
# 或
docker build -t local-router . && docker run -p 3435:3435 -v lr-data:/data local-router
```

## 把 agent 指过来

```sh
# 方式一：CLI 改写各 agent 自己的配置
local-router provider add deepseek --key sk-xxx
local-router claude deepseek/deepseek-chat     # 写 ~/.claude/settings.json
local-router codex  kimi/kimi-k2               # 写 ~/.codex/config.toml
local-router gemini glm/glm-4.6                # 写 ~/.gemini/.env
local-router opencode deepseek/deepseek-chat   # 写 opencode.json

# 方式二：环境变量（任何工具）
export OPENAI_BASE_URL=http://127.0.0.1:3435/v1      OPENAI_API_KEY=local-router
export ANTHROPIC_BASE_URL=http://127.0.0.1:3435      ANTHROPIC_API_KEY=local-router
export GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:3435  GEMINI_API_KEY=local-router
```

## CLI 速览

```sh
local-router serve [--port N] [--addr A] [--lan] [--no-console]
local-router daemon start|stop|status|logs|install     # 后台常驻 / 开机自启
local-router provider add <preset|名字> --key sk-… [--url …] [--protocol anthropic]
local-router provider list | rm | models | expose | media | test | key
local-router models                                    # 对外模型目录
local-router group add <名字> --models a/m1,b/m2 --routing order
local-router usage [--period today] [--csv]            # 用量与账单
local-router traces | trace <id>                       # 链路追踪
local-router dashboard                                 # 终端仪表盘
local-router doctor                                    # 自检
local-router remote status --host <url> --key <k>      # 查远端节点状态
local-router workbuddy status|credits|login|checkin    # 登录态上游
local-router qwen status|credits|login
```

完整命令速查见 [USAGE.md § 2.3](USAGE.md#23-命令行速查)。

## 文档

| 文档 | 内容 |
| --- | --- |
| [USAGE.md](USAGE.md) | 使用手册：五分钟跑起来、日常命令、11 个常见场景、排障 |
| [docs/API.md](docs/API.md) | 网关端点、鉴权、模型命名参考 |
| [docs/TRACING.md](docs/TRACING.md) | 会话 ↔ 上游调用的链路追踪模型与落盘格式 |
| [docs/BILLING.md](docs/BILLING.md) | 用量记录、成本/credits 口径、账单维度、SQLite 查询 |
| [docs/PROVIDERS.md](docs/PROVIDERS.md) | 登录态上游（WorkBuddy / QwenWork / Trae CN）机制 |
| [docs/CONFIG.md](docs/CONFIG.md) | `config.json` 字段级配置参考 |
| [docs/BENCHMARK.md](docs/BENCHMARK.md) | 吞吐基线与压测复现步骤 |
| [examples/multi-machine](examples/multi-machine/README.md) | 两节点级联的可运行示例 |
| [examples/monitoring](examples/monitoring/README.md) | Prometheus + Grafana 现成栈 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 贡献流程 |
| [SECURITY.md](SECURITY.md) | 安全模型与漏洞报告渠道 |
| [CHANGELOG.md](CHANGELOG.md) | 每个版本的变更 |

## 安全要点

- 默认只绑回环地址；`--lan` / 容器暴露时 `/api`、`/metrics` 需要 caller key，
  写请求校验 Origin
- Provider key 在控制台打码；`config.json` 以 0600 落盘；提示词密钥遮蔽
  （`masking`）可拦住 `sk-…` / `ghp_…` / 长 hex 误发上游
- 运行时凭据、会话与用量数据都在 `LOCAL_ROUTER_HOME` 本地目录，不入库

暴露到局域网或公网前请先读 [SECURITY.md](SECURITY.md)。

## 范围与取舍

**核心**：多服务商网关、协议互译、路由组、用量记录、会话/项目/上下文观测、
订阅账号登录态接入（WorkBuddy / QwenWork / Trae CN）、网页管理、CLI、
多 key 故障转移、多节点级联与远程运维、Prometheus/OTLP 可观测性出口。

**有意不做**：桌面菜单栏、桌面客户端形态、插件生态、本地 PII 脱敏、
QwenWork 企业版（VPC / PAT）。

## 开发

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets            # src/ 内单元测试 + tests/ 内集成测试
cd frontend && npm run dev          # 控制台热更新（API 代理到 :3435）
```

`cargo build` 会通过 `build.rs` 自动构建 `frontend/`；无 npm 环境可设
`LOCAL_ROUTER_SKIP_FRONTEND=1` 用桩页面代替。

## License

MIT
