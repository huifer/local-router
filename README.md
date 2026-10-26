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
local-router provider list | rm | models | expose | media | test | key
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

`config.json` 里的 `settings` 还能改几件事：

| 字段 | 作用 |
| --- | --- |
| `cors_origins` | 允许哪些浏览器源跨域调用网关。默认空（不允许）；填 `["*"]` 或 `["http://localhost:3000"]` 才开 |
| `default_caller_key` | 调用方默认用的 key |
| `record_usage` | 是否记录用量 |
| `masking` | 是否在发往上游前遮蔽提示词里的密钥 |
| `workbuddy_auto_checkin` | 是否自动领 WorkBuddy 每日签到（默认开） |
| `send_agent_user_agent` | 是否把 agent 的 User-Agent 转发给上游 |

`usage.jsonl` 跟 `config.json` 放在同一个目录；超过 8 MB 会自动翻页成
`usage.jsonl.1`（可用 `LOCAL_ROUTER_USAGE_MAX_BYTES` 改这个上限）。

### 额度与开销统计

每个请求的记录里都带钱：

```json
{"model":"workbuddy/hy3","input_tokens":15,"output_tokens":128,
 "cost_usd":0.000148,"credits":0.0172,"priced":true}
```

- **cost_usd**：按厂商公开价目表算的美元成本（每百万 tokens）。
- **credits**：订阅制厂商（如 WorkBuddy）自己额度单位的估算消耗。
- **priced=false** 表示该模型没有价格记录，钱无法估算（用量页会计数）。

看总账：

```sh
local-router usage --period today    # 按 provider / 按模型，含 $ 与 credits 列
local-router usage --csv             # 机器可读，含 cost_usd / credits 列
local-router prices                  # 当前价目表（29+ 模型）
```

HTTP API（`period=today|24h|7d|30d|all`）：

```sh
curl -H "Authorization: Bearer <key>" \
  'http://127.0.0.1:3425/v1/local-router/usage?period=7d'
# → cost_usd / credits / unpriced 汇总 + per_provider[] 每个平台的账单
```

控制台 Usage 页有「BY PLATFORM」面板：每个平台一段时期的请求数、token、
美元成本、credits 消耗一目了然。

**改价格 / 加新厂商**：写 `~/.config/local-router/prices.json`（不用重启）：

```json
{"hy3": {"input": 0.5, "output": 2.0, "credits_per_million": 200.0}}
```

价目是估算：订阅制厂商的 credit 折算率来自套餐额度与公开价的比例，可能与
厂商实际扣减有偏差；`prices.json` 可随时校准。

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
| `POST /v1/images/generations` · `/v1/images/edits` | 图片生成 / 编辑（multipart 编辑） |
| `POST /v1/videos` · `GET/DELETE /v1/videos/{id}` · `GET /v1/videos/{id}/content` | 视频任务：创建 / 查状态 / 取文件 / 取消 |
| `POST /v1/responses` | OpenAI Responses |
| `POST /v1/messages` · `/v1/messages/count_tokens` | Anthropic Messages |
| `POST /v1beta/models/{model}:generateContent` / `:streamGenerateContent` | Gemini |
| `GET /v1/models` | 模型目录（OpenAI list 形状） |
| `GET /health` · `/v1/local-router/status` | 健康检查 / 运行状态 |

任意「入口协议 × 上游协议」组合都支持：系统提示、多模态消息、工具定义 /
调用 / 结果、思考块（含 signature）、stop 原因、token 用量（缓存读写、
思考 token）双向翻译。上游忽略 `stream=true` 返回 JSON 时，网关自动把整段
回复转成事件流。

## WorkBuddy：用本机登录态当上游

不想买 API key，但本机装了 WorkBuddy？local-router 可以直接复用 WorkBuddy 的
登录态，把它当成一个上游：

```sh
local-router provider add workbuddy      # 不需要 key
local-router workbuddy status            # 登录状态、账号、token 有效期
local-router workbuddy credits           # 积分（资源包 + 签到）
local-router workbuddy login             # 浏览器登录（打印链接 + 自动接入）
local-router workbuddy checkin           # 手动领当日签到（通常自动完成）
local-router workbuddy models            # 账号可用的模型
local-router claude workbuddy/hy3        # 让 Claude Code 走 WorkBuddy
```

要点：

- **不落盘**：access token 只在内存里。`config.json` 里只有
  `"auth": "workbuddy"` 这样的标记，没有任何密钥；日志和 `usage.jsonl` 里也
  不会出现 token。
- **自动续期**：token 快到期时会用 refresh token 换一个新的（refresh token
  放在 `X-Refresh-Token` 头里，由 WorkBuddy 客户端同样的接口换发）。
- **协议**：上游只接受流式请求（`POST /v2/chat/completions`）。local-router
  会在调用方要整段回复时把它流式收回来再拼好，所以 OpenAI / Anthropic /
  Responses / Gemini 四种入口都能用。
- **自动签到**：网关会自己盯签到活动，到点未签就代签（默认开；
  `config.json` 里 `"workbuddy_auto_checkin": false` 关掉）。每日 +100 credits
  就不用手动领了。也可手动：`local-router workbuddy checkin`。
- **登录**：不用装客户端，直接

  ```sh
  local-router workbuddy login
  ```

  它会打印一个登录链接（并自动打开浏览器），登录完成后 token 自动保存到
  local-router 自己的会话文件（0600），provider 自动加上——WorkBuddy 客户端
  与它的登录文件完全不受影响。链接五分钟内有效，也可以复制到别的机器的
  浏览器里打开。
  已有本机 WorkBuddy 登录态时无需此步，直接可用；登录完成后可用检查脚本
  验证：

  ```sh
  bash scripts/workbuddy-login-check.sh   # 五步检查：客户端/登录态/服务端/接入/真实推理
  ```
- 容器里看不到宿主机的登录态，除非你把会话文件挂进去
  （`LOCAL_ROUTER_WORKBUDDY_SESSION`）。

### 图片与视频

图片与视频走各厂商自己的原生端点，网关只做转发与统一入口：

```sh
# 用 WorkBuddy 的图像模型画一张图
curl http://127.0.0.1:3425/v1/images/generations \
  -H "Authorization: Bearer local-router" \
  -H "content-type: application/json" \
  -d '{"model":"workbuddy/hunyuan-image-v3.0-art","prompt":"一只戴帽子的猫"}'

# 视频是异步任务：创建 → 查状态 → 取文件
curl http://127.0.0.1:3425/v1/videos \
  -H "Authorization: Bearer local-router" \
  -H "content-type: application/json" \
  -d '{"model":"workbuddy/kling-video-v3","prompt":"一只猫在跑"}'
```

要点：

- **模型要声明**：给 provider 配上能做图/视频的模型
  （`local-router provider media <id> <模型...>`），它们会出现在 `/v1/models`
  里，并带 `modalities` / `native_endpoints` 元信息，客户端据此发现能力。
- **厂商原生路径可配**：默认走 OpenAI 的 `/v1` 家族；像 WorkBuddy 把媒体放在
  `/v2` 且视频一次生成完（没有任务状态接口），在 provider 的 `media_paths` 里
  写明即可，网关会自己兜住状态与下载。
- 图片/视频的用量不计入 chat 的 token 统计，但同样计入 `usage.jsonl` 的请求数。

### WorkBuddy 相关的环境变量

| 变量 | 作用 |
| --- | --- |
| `LOCAL_ROUTER_WORKBUDDY_SESSION` | 登录态文件的完整路径（找不到时不再猜别的位置） |
| `LOCAL_ROUTER_WORKBUDDY_ENDPOINT` | 换用别的 WorkBuddy 端点（私有化 / 测试） |
| `WORKBUDDY_HOME` | 登录态所在目录（默认自动查找） |

## 路由组

```sh
local-router group add "backup" --models deepseek/deepseek-chat,glm/glm-4.6 --routing order
local-router claude group/backup
```

- `order` 主备故障转移 · `rotate` 轮询 · `usage` 最少使用 · `smart` 额度感知 · `weight` 加权
- `stays=auto|session|turn|off|strict`：会话粘性（prompt cache 还值钱时留在同一个 key 上）
- 组内可嵌套组（带环检测）；一个 provider 配多把 key 会逐个尝试
- `hold_when_exhausted`：账户配额窗口空了就不再路由到该上游（默认关；
  实测 WorkBuddy 在额度为 0 时仍可服务，所以不要对它开）

## 开发

```sh
cargo fmt --all -- --check   # 格式
cargo clippy --all-targets --all-features -- -D warnings
cargo test                   # 67 个测试：协议翻译 / 路由 / HTTP 端到端 / 控制台守卫 / 登录态
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
