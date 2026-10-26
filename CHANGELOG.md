# Changelog

本项目的所有显著变更都会记录在这个文件里。
格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added

- **WorkBuddy 登录态上游**：新增 `workbuddy` provider，复用本机 WorkBuddy 客户端
  的登录态作为上游凭据。
  - `local-router provider add workbuddy`（无需 key），
    `local-router workbuddy status|credits|models` 查看登录状态、积分与模型。
  - access token 只在内存中：`config.json`、日志、`usage.jsonl` 均不落盘。
  - token 快到期时自动用 refresh token 续期（`X-Refresh-Token` 头）。
  - 上游只接受流式请求，调用方要整段回复时网关会收流再拼好，四种入口协议
    都能用。
  - 控制台 Overview 新增「WorkBuddy session」面板：账号、剩余积分、签到状态、
    token 有效期与资源包明细；接口为 `GET /api/workbuddy`。
  - 新增环境变量 `LOCAL_ROUTER_WORKBUDDY_SESSION`（登录态文件路径）、
    `LOCAL_ROUTER_WORKBUDDY_ENDPOINT`（换端点）、`WORKBUDDY_HOME`（登录态目录）。
- **浏览器跨域**：`settings.cors_origins` 控制哪些源可以跨域调用网关。默认
  拒绝（网关是给本地工具用的）；填 `["*"]` 或具体 origin 后放行。
- 用量日志跟 `config.json` 同目录（此前写死在全局配置目录，多实例时会串）。

- **上游配额与路由洞察**（对齐同类网关的观察能力）：
  - `GET /v1/local-router/quotas`：每个上游的配额窗口（名称/已用/剩余/重置时间/
    是否被扣住），WorkBuddy 直接来自其账单接口。
  - `GET /v1/local-router/usage?period=` 与 `/v1/local-router/usage/requests?limit=`：
    周期汇总与逐请求明细的 HTTP API（此前只有控制台内页和 CLI）。
  - `GET /v1/local-router/route` 带 `X-Session-ID` 头可查该会话最近一次路由：
    计划、实际服务者、原因、时间。会话 ID 也接受 `X-Magpie-Session`。
  - Provider 新增 `hold_when_exhausted`：配额窗口空了就不再路由到它（默认关；
    WorkBuddy 实测 0 额度仍可用，故默认不挡）。

### Added

- **图片与视频**：新增 OpenAI 形状的媒体端点，图片生成/编辑、视频任务全生命周期
  （创建 / 状态 / 取文件 / 取消）。
  - `POST /v1/images/generations`、`POST /v1/images/edits`（multipart 编辑）。
  - `POST /v1/videos`、`GET|DELETE /v1/videos/{id}`、`GET /v1/videos/{id}/content`。
  - provider 新增 `media_models`（能做图/视频的模型）与 `media_paths`（厂商原生
    媒体路径）；`local-router provider media <id> <模型...>` 可配置。
  - `/v1/models` 增加 `modalities` 与 `native_endpoints` 元信息，客户端据此发现
    哪些模型能画图/拍视频。
  - 厂商把视频一次生成完（无任务状态接口）时，网关自己保管结果并提供状态与下载。

- **WorkBuddy 自动签到**：网关启动后每半小时检查一次签到活动，到点未签就
  代签（每日 +100 credits 自动到账）。`workbuddy_auto_checkin` 默认开；
  `local-router workbuddy checkin` 可手动领。

- **WorkBuddy 浏览器登录**：`local-router workbuddy login` 打印登录链接并
  自动打开浏览器，登录完成后 token 存入 local-router 自己的会话文件（0600）、
  provider 自动配置；无需安装 WorkBuddy 客户端，也不改动客户端的登录文件。
  登录协议与客户端一致（`/v2/plugin/auth/state` → 浏览器 → 轮询
  `auth/token` / `login/account`）。

- **每平台额度/开销统计**：每条用量记录新增 `cost_usd`（按厂商公开价目折算
  的美元成本）、`credits`（订阅制厂商如 WorkBuddy 的额度消耗估算）与
  `priced`（该模型是否有价格记录）。
  - CLI：`local-router usage` 新增按 provider / 按模型的 $ 与 credits 汇总，
    `--csv` 增加两列；`local-router prices` 查看价目表。
  - HTTP：`/v1/local-router/usage` 返回 `cost_usd` / `credits` / `unpriced`
    汇总与 `per_provider[]` 每平台账单；`/usage/requests` 同步。
  - 控制台 Usage 页新增「BY PLATFORM」面板。
  - 内置 29+ 模型价目（OpenAI / Anthropic / DeepSeek / Moonshot / Zhipu /
    MiniMax 公开价 + WorkBuddy credit 折算率）；`prices.json` 可覆盖或新增，
    无需重启。
  - 顺带修复：CLI 的 `usage` 读的是全局目录的 usage.jsonl（多实例时看不到
    本实例的记录），现与网关写入路径一致。

### Added

- **SQLite 用量存储**：新增 `usage.db`（跟 config 同目录，rusqlite bundled，
  WAL 模式），每条用量记录（含 cost_usd / credits）双写 SQLite + JSONL；
  聚合查询（总账/按天/按平台/按模型/最近请求）改走 SQL，数据量大也不卡。
  `LOCAL_ROUTER_DISABLE_SQLITE=1` 可退回纯 JSONL。数据库文件可直接用
  sqlite3 客户端查询。
- **控制台可视化**：Usage 页新增三张图（纯 SVG，零前端依赖）——按天
  请求/成本柱状图、每平台美元+credits 成本横条、模型占比甜甜圈；页面
  标明当前存储（SQLITE / JSONL）。

- **终端仪表盘**：`local-router dashboard` —— 与控制台同源的实时数据
  （providers / 7 天流量与开销 / 每平台条形 / 按天趋势 / 最近请求），ANSI
  绘制、2 秒刷新、`q` 退出，零 UI 依赖。
- **守护进程**：`local-router daemon start|stop|status|logs` —— 后台运行
  （网关 + 控制台同一进程），pid 文件管理，start 等端口就绪才返回，stop
  SIGTERM 优雅退出（卡死自动 SIGKILL），logs 即 `tail -f`。
- **端口与绑定全量可配**：`serve`/`daemon` 均支持 `--port` / `--web-port`
  （0 = 关闭页面）/ `--addr` / `--web-addr`（控制台与网关分开绑定）/
  `--lan`，环境变量与 config.json 同序。

### Fixed

- 流式请求的用量记录此前 token 恒为 0（绝大多数 agent 走流式，用量页看不到
  真实消耗）：现在从流事件里取用量。
- `local-router opencode` 此前会覆盖整个 `provider` 段，把用户自己配的其它
  provider 一起删掉；现在只替换 local-router 那一项。
- 绑在 `0.0.0.0` 时，写给 agent 的地址改为 `127.0.0.1`（之前写的是
  `0.0.0.0`）。
- `doctor` 不再把「登录态型」和「本地免 key 型」provider 误报为缺 key；并会
  报告 WorkBuddy 登录状态与 token 剩余时间。
- 控制台的「刷新模型」「测试 provider」现在支持登录态型 provider。
- `usage.jsonl` 会自动翻页（默认 8 MB），不再无限增长。

### Changed

- 测试从 47 个增加到 79 个：新增 WorkBuddy 会话、续期、agent 配置写入、用量
  日志与「流式上游 → 整段回复」的端到端用例。

## [0.1.0] - 2026-10-26

首个公开发布版本。

### Added

- 本地 AI 网关（Rust 单二进制）：一个进程提供 OpenAI Chat / OpenAI
  Responses / Anthropic / Gemini 四种入口协议，任意「入口 × 上游」互译
  （流式、工具调用、思考块含 signature、stop 原因、token 用量双向翻译）。
- 路由组：`order` / `rotate` / `usage` / `smart` / `weight` 五种策略，
  故障转移、会话粘性（`stays`）、嵌套组（带环检测）、单 provider 多 key。
- React + TypeScript 控制台：服务商 / 模型 / 路由 / 用量 / 设置 / caller
  key 管理；浏览器密钥只做掩码展示。
- CLI：`provider` / `group` / `models` / `usage` / `doctor`，以及
  `claude` / `codex` / `gemini` / `opencode` 一键改写各 agent 自己的配置。
- 用量记录：每请求一行 `usage.jsonl`（模型、provider、key、token、时延、
  状态），`UsageGuard` 保证客户端提前断开也落盘。
- 配置热重载：CLI / 其他进程改了 `config.json`，网关在下一个请求自动
  感知（mtime 探测），不用重启。
- Docker：多阶段构建（node 构建控制台 → rust 构建二进制 → alpine 运行时），
  非 root 运行、`/data` 持久化、自带 healthcheck，compose 一键起。
- 安全：
  - 网关所有端点要求 `Authorization: Bearer <key>`（或 `x-api-key` /
    `x-goog-api-key` / `api-key`）。
  - 控制台在回环地址免鉴权；绑到非回环地址时 `/api` 需要同一套 key。
  - 所有写请求校验 Origin，拒绝跨站驱动控制台。
  - `config.json` 以 0600 落盘；`LOCAL_ROUTER_DEFAULT_KEY` 只在内存。

[Unreleased]: https://github.com/TODO/local-router/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TODO/local-router/releases/tag/v0.1.0
