# local-router

一个本地 AI 网关：把你的所有模型服务商收进一个进程，给每个编码 agent
（Claude Code / Codex / Gemini CLI / OpenCode / 任何吃 base URL 的工具）
提供统一、可路由、可观测的模型接口。

- **Rust 单二进制**：网关 + React 控制台 + CLI，一个文件跑起来
- **四种协议互译**：OpenAI Chat / OpenAI Responses / Anthropic / Gemini，
  任意「入口 × 上游」组合（流式、工具调用、思考块、token 用量）
- **路由组**：order / rotate / usage / smart / weight，故障转移、会话粘性、嵌套组、多 key
- **控制台**：React + TypeScript + Vite 构建的服务商 / 模型 / 路由 / 用量 / 设置界面
- **Docker**：多阶段构建，`/data` 持久化，compose 一键起

本项目为独立实现，只做网页模式 + CLI，不做桌面客户端。

## 快速开始

### 本地运行

```sh
npm install && npm run build --prefix frontend   # 构建 React 控制台（cargo 也会自动调）
cargo build --release
./target/release/local-router                    # 网关 :3425 + 控制台 :3426
```

### Docker

```sh
docker compose up -d          # http://localhost:3426
# 或
docker build -t local-router . && docker run -p 3425:3425 -p 3426:3426 -v lr-data:/data local-router
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
export OPENAI_BASE_URL=http://127.0.0.1:3425/v1      OPENAI_API_KEY=local-router
export ANTHROPIC_BASE_URL=http://127.0.0.1:3425      ANTHROPIC_API_KEY=local-router
export GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:3425  GEMINI_API_KEY=local-router
```

## CLI

```sh
local-router                                   # 网关 + 控制台（前台）
local-router serve [--port N] [--web-port N] [--lan]
local-router web [--port N]

local-router provider add <preset|名字> --key sk-… [--url …] [--models a,b]
local-router provider list | rm | models | expose | test | key
local-router models                            # 模型目录
local-router group add <名字> --models a/m1,b/m2 --routing order | list | rm
local-router ls | usage [--csv] | doctor
local-router claude|codex|gemini|opencode <model>
```

## 配置

`~/.config/local-router/`（`LOCAL_ROUTER_HOME` 可覆盖，Docker 里是 `/data`）：

```
config.json    # providers、groups、settings、caller-keys（0600）
usage.jsonl    # 每请求一行：模型、provider、token、时延、状态
```

文件被 CLI / 另一个进程修改后，网关在下一个请求自动重载（mtime 探测），
不用重启。

### 环境变量

| 变量 | 作用 |
| --- | --- |
| `LOCAL_ROUTER_HOME` | 配置目录（Docker 里设为 `/data`） |
| `LOCAL_ROUTER_GATEWAY_PORT` / `LOCAL_ROUTER_WEB_PORT` | 端口（CLI 参数可再覆盖） |
| `LOCAL_ROUTER_GATEWAY_ADDR` / `LOCAL_ROUTER_WEB_ADDR` | 绑定地址 |
| `LOCAL_ROUTER_DEFAULT_KEY` | 运行时调用密钥（**不落盘**，优先于文件里的 key） |
| `LOCAL_ROUTER_LAN` | `1` 等价于 `--lan` |
| `LOCAL_ROUTER_LOG` | 日志过滤（如 `debug`） |

> 鉴权：网关请求需要 `Authorization: Bearer <key>`（也接受 `x-api-key` /
> `x-goog-api-key` / `api-key`），key 取 `LOCAL_ROUTER_DEFAULT_KEY` →
> 设置里的默认 key → caller keys 任一匹配；一把 key 都没有时拒绝一切请求。
>
> 控制台：回环地址上免鉴权（本机信任）；页面绑到非回环地址（`--lan`、
> 容器）时 `/api` 需要同一套 key，控制台会提示输入一次。所有写请求校验
> `Origin`，跨站写一律 403。`config.json` 以 0600 落盘；
> `LOCAL_ROUTER_DEFAULT_KEY` 只在进程内存。

## 网关端点

| 端点 | 说明 |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI Chat（流式 / 非流式） |
| `POST /v1/responses` | OpenAI Responses |
| `POST /v1/messages` · `/v1/messages/count_tokens` | Anthropic Messages |
| `POST /v1beta/models/{model}:generateContent` / `:streamGenerateContent` | Gemini |
| `GET /v1/models` | 模型目录（OpenAI list 形状） |
| `GET /health` · `/v1/local-router/status` | 健康检查 / 运行状态 |

任意「入口协议 × 上游协议」组合都支持：系统提示、多模态消息、工具定义 /
调用 / 结果、思考块（含 signature）、stop 原因、token 用量（缓存读写、
思考 token）双向翻译。上游忽略 `stream=true` 返回 JSON 时，网关自动把整段
回复转成事件流。

## 路由组

```sh
local-router group add "backup" --models deepseek/deepseek-chat,glm/glm-4.6 --routing order
local-router claude group/backup
```

- `order` 主备故障转移 · `rotate` 轮询 · `usage` 最少使用 · `smart` 额度感知 · `weight` 加权
- `stays=auto|session|turn|off|strict`：会话粘性（prompt cache 还值钱时留在同一个 key 上）
- 组内可嵌套组（带环检测）；一个 provider 配多把 key 会逐个尝试

## 开发

```sh
cargo fmt --all -- --check   # 格式
cargo clippy --all-targets --all-features -- -D warnings
cargo test                   # 47 个测试：协议翻译 / 路由 / HTTP 端到端 / 控制台守卫
cd frontend && npm run dev   # 控制台热更新（API 代理到 :3426）
```

`cargo build` 会通过 `build.rs` 自动构建 `frontend/`（检测到源码变化或
`dist` 缺失时）。CI / 无 npm 环境可设 `LOCAL_ROUTER_SKIP_FRONTEND=1` 用
桩页面代替。手工冒烟可配合 `scripts/mock_vendor.py` 起一个假厂商。

## 参与贡献

- [CONTRIBUTING.md](CONTRIBUTING.md)：环境搭建、提交前检查、怎么改
- [SECURITY.md](SECURITY.md)：漏洞私下报告渠道与安全模型
- [CHANGELOG.md](CHANGELOG.md)：每个版本的变更
- 行为准则：[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)

## 范围与取舍

核心：多服务商网关、协议互译、路由组、用量记录、网页管理、CLI、多 key
故障转移。有意不做：桌面菜单栏、TUI、订阅账号登录态、插件生态、本地
PII 脱敏。

## License

MIT
