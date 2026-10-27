# Changelog

本项目的所有显著变更都会记录在这个文件里。
格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [0.1.1] - 2026-10-27

### Added

- **使用说明 USAGE.md**：面向使用者的完整操作手册——五分钟跑起来、CLI 命令
  速查、四种入口协议的 curl 示例、11 个常见场景（自动故障转移 / 轮询分摊 /
  最闲优先 / 会话粘性 / 局域网共享 / 开机自启 / 项目计费 / 密钥遮蔽 /
  改价即时生效 / 自动纳新模型）、配置与文件清单、排障问答与卸载步骤。
  README 顶部与「参与贡献」区块均已指向它。

- **QwenWork（千问办公）登录态上游**：新增 `qwenwork` provider，走千问办公
  个人公有云的设备流登录（与桌面客户端同款 COSY 信封协议）。
  - `local-router qwen login` 打印登录链接，浏览器完成后自动存会话文件
    （0600）并配好 provider；`local-router qwen status|credits|models`
    查看登录状态、额度余额（钱包分日常/月度/长期池）与模型。
  - 每个请求由网关自行 RSA 包 AES 密钥 + MD5 签名 + 乱序 base64 编码，
    调用方看到的仍是标准入口协议；token 自动续期且不回写磁盘。
  - 仅支持个人公有云（gateway.qwenwork.cn）；企业版 VPC / PAT 有意不做。
- **会话与项目观测**：
  - 会话标题簿：每个会话 id 记住标题（首个用户消息第一行），Usage 页
    Sessions 子页按会话出账。
  - 项目注册表：按 agent 上报的工作目录分组（`x-project` / `metadata.cwd`），
    控制台可改名、配色、设预算、合并重复目录；合并会把 JSONL 与 SQLite
    里的历史记录一起重键。
  - 上下文拆解：每个请求记录系统提示 / 工具定义 / 附件 / 对话各占多少
    token，窗口填充率与缓存友好度评分，Usage 页 Context 子页可视化。
  - `/api/usage` 返回 `by_session` / `by_agent` / `by_project` / `by_key` /
    `requests_timeline` / `context` / `quota` 全量字段；前端类型同步对齐
    （含 `qwenwork` 协议与 `auth` 枚举、settings 新字段）。

### Fixed

- **`settings.masking` 一直是空配置**：README 声称「发往上游前遮蔽提示词里的
  密钥」，但网关从未读取该字段，实测 `sk-…` 明文直通上游。现已实现：开启后
  对系统/消息文本与工具参数做遮蔽（前缀型 key `sk-`/`ghp_`/`AKIA`/`AIza`/
  `xox`/JWT、长十六进制串、名字含 key/token/secret/password 的工具参数），
  并有单元测试与行为级冒烟守护。
- **`auto_include` 一直是死配置**：provider 的 glob 字段可保存但无消费点。
  现在「刷新模型」会把 vendor 新列出且匹配 glob 的模型自动并入 provider
  （落盘、无需重启），响应带 `auto_included` 说明新增了哪些。
- **`stays=turn` / `stays=strict` 语义未实现**：此前所有粘性模式都只是
  「把上次的目标提到最前」。现在 `turn` 在新对话（请求不含 assistant/tool
  历史）时清除粘性重新选；`strict` 只返回会话的目标（目标消失时报错，
  拒绝回退其它成员）。
- **QwenWork 额度未进额度视图**：`/v1/local-router/quotas` 与控制台
  Usage 页 Quota 面板此前只看 WorkBuddy；qwenwork 登录态的余额、钱包池与
  重置时间现已并入。
- **控制台「按模型」聚合一直是空的**：`usage.db` 的 by-model 查询里
  `ORDER BY requests` 引用了不存在的列别名（`COUNT(*)` 未命名），SQL 准备
  失败后被静默吞掉返回空列表；加 `AS requests` 后修复，并有聚合单元测试
  覆盖（totals / by_day / by_provider / by_model / recent）。
- **流式上游的整段回复丢了工具调用 id**：`ChatResponse::from_events` 重组
  tool_use 时未回填 call id，后续 tool_result 无法配对；已修复并有
  单元测试覆盖。
- **QwenWork 额度重置时间永远为空**：服务端答 `YYYY-MM-DD HH:MM:SS` 无时区
  的时间串，此前用要求带时区的 `DateTime::parse_from_str` 解析必然失败；
  改为按 UTC 读 `NaiveDateTime`。
- 控制台保存设置时会把后端新字段（`cors_origins`、`workbuddy_auto_checkin`）
  静默重置：前端 `Settings` 类型补齐这两个字段。
- **内置预设泄迭代入了一个密钥模板占位符**：`qwenwork` 预设的默认模型列表
  里混入了 `{{SECRET_…}}` 占位符（编辑工具的模板残留），展开到 provider
  后会成为一个不存在的模型名；已替换为真实模型 `qwen3-coder-plus`，
  并有预设清单完整性单元测试守护。

### Changed

- **网关与控制台合并为一个端口**：以前网关监听 3435、控制台另开 3426，
  现在是**同一个 HTTP 服务、同一个端口**（默认 3435）——页面、静态资源、
  控制台 `/api` 与模型端点（`/v1/*`、`/v1beta/*`、`/health`）都在一处，
  浏览器同源，不必再记第二个地址。
  - 路由按路径判定：网关端点优先，其余落到控制台；控制台的守卫
    （回环免鉴权、写请求校验 Origin）**只作用于控制台路径**，不会误伤
    模型端点（`tests/web.rs` 有对应用例）。
  - `--web-port` / `LOCAL_ROUTER_WEB_PORT` / `settings.web_port` 保留为
    兼容项（值被忽略）；只想提供 API 时用新的 `--no-console`
    （配置文件里是 `settings.console: false`）。
  - Docker 只暴露 3435；前端 dev 代理与文档同步更新。
- 测试从 79 个增加到 228 个：src/ 内新增 129 个 Rust 原生单元测试
  （`#[cfg(test)]`，覆盖协议 IR 与四协议往返、COSY 信封、配置、价目、预设、
  会话、项目、用量聚合与估算、SQLite 存储、URL/帧解析、仪表盘渲染、
  控制台守卫源校验），tests/ 内新增控制台 API 形状用例（对齐前端
  types.ts 的每个面板字段）。
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

[Unreleased]: https://github.com/huifer/local-router/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/huifer/local-router/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/huifer/local-router/releases/tag/v0.1.0
