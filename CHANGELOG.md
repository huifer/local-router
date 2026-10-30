# Changelog

本项目的所有显著变更都会记录在这个文件里。
格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added

- **直接测试对话**：控制台 provider 抽屉新增「测试对话」，会发送一条最小 ping 并显示耗时；命令行使用 `local-router provider test <provider-id>`，输出耗时与上游回复。QwenWork/Trae CN 会走各自专用协议，不再被错误地当成普通 OpenAI 上游。
- **API Key 级用量统计**：Usage/HTTP API/CLI 现在分开统计上游 Provider API Key 与 local-router 调用方 API Key，包含请求数、失败数、输入/输出/缓存/推理 token、美元成本与 credits；CSV 和最近请求明细也带上 Key 标识。
- **单 Key 最大并发**：Provider API Key 可配置最大同时进行的请求数，超过限制的请求排队；支持 `provider concurrency <provider> <key-id|session> <limit>`，`0` 取消限制。会话型 provider 的 `session` 限制按账号生效。
- **429 全局冷却**：任何上游返回 HTTP 429 后，该 provider 默认冷却 60 秒；连续 429 会指数退避，最长 10 分钟。冷却期间网关在发送前跳过它并尝试下一个路由目标，控制台/CLI 的测试也不会绕过冷却。可用 `LOCAL_ROUTER_COOLDOWN_SECS` 与 `LOCAL_ROUTER_COOLDOWN_MAX_SECS` 调整。

- **每个账号一份独立额度 + 一键签到**：会话型 provider 的账号池里，每个账号
  都有自己的额度与签到状态，现在逐账号读、逐账号领：
  - 新接口 `GET /api/providers/{id}/accounts/{name}/quota`：读**某一个**账号的
    额度（WorkBuddy 读资源包与签到状态，QwenWork 读套餐余额与钱包池，Trae CN
    没有额度可言就明说）；
  - 新接口 `POST /api/providers/{id}/accounts/{name}/checkin`：给**某一个**账号
    领当天的签到额度（只有 WorkBuddy 有签到）；厂商回答“今天已签到”不算错误；
  - 控制台 provider 抽屉的账号行里：这一行的剩余额度、资源包、签到状态，
    行尾一个 **刷新额度**；抽屉右上角一个 **刷新全部**，一次更新所有账号的
    额度与签到状态（打开抽屉时也会自动刷一遍）；
  - 总览页的 WorkBuddy 卡片上也加了一键签到与刷新额度（作用在正在用的账号上）。
- **控制台的用户名打码**：账号昵称显示成 `张*` / `嘘***`，uid 显示成 `47d4…`
  ——控制台会展示账号持有人，截图不该带出人名。
- **Trae CN（Cloud-IDE）接入**：新增 `src/trae.rs` 模块与内置预设 `trae-cn`。
  Trae 不是普通的 OpenAI 兼容上游——它要 `Cloud-IDE-JWT` 鉴权头、发自己的
  SSE 事件词汇，所以走网关里的专用通道（`Protocol::TraeCn` 的 provider 不进
  通用 HTTP 管道）：
  - 会话存成本地账号池 `trae-session.json`（多账号 + `active`），
    access token 临期自动用 refresh token 换新并回写磁盘；
  - 账号池支持列出、切换（`use_account`）、删除，网关发请求时按账号轮转，
    分摊额度；
  - 当前只支持**导入的会话**（从 Magpie 迁移过来的 Cloud-IDE 登录态），
    浏览器登录尚未接通（控制台会明确说明这一点）。
- **会话型 provider 的多账户池**（WorkBuddy / QwenWork / Trae CN 统一一套模型）：
  - **槽位在「登录开始」时选、在「登录完成」时落盘**：不给账号名就取第一个
    空闲槽（`default` → `{provider}-2` → `{provider}-3`），所以第二次登录
    不再覆盖第一个；还在途的登录也算占位，两次并发登录不会抢同一个槽；
  - **同 uid 不合并**：每次登录都存成独立槽位（浏览器登录的是谁由用户决定），
    若这次的 uid 和池里已有的某个相同，界面会提示「池里已有同一个 uid 的
    账号」（厂商登录页上点「切换账号」即可换人，不用先退出浏览器）；
  - **网关轮转**：请求按原子游标轮流用不同账号，多个账号分摊而不是一个吃满；
  - **账号管理**：控制台抽屉里可切换（`POST /api/providers/{id}/accounts/active`）
    与删除（`DELETE /api/providers/{id}/accounts/{name}`），删掉正在用的账号会
    自动切到剩下的第一个；WorkBuddy 桌面端自己的登录文件不允许删（避免把
    客户端登出）；
  - **CLI**：`qwen accounts` 列出、`qwen use <名字>` 切换、`qwen login [名字]`
    登录到指定槽位（不给名字即取第一个空闲槽）。
- **控制台可看完整密钥**：provider 详情抽屉里列出该 provider 的所有 key，
  默认打码，点「显示」才去后端取原文并可复制。新接口
  `GET /api/providers/{id}/keys/{key_id}/secret` **只允许本机访问**——绑定在
  非回环地址时，即使带着网关 key 也一律 403（网关 key 是用来调模型的，不是
  用来读供应商密钥的）。
- **`scripts/import_magpie_keys.py`**：magpie 磁盘上的密钥是不可逆的
  `{{API_KEY_xxx}}` 占位符，真实值只在运行中的 magpie 进程内存里。脚本先让
  magpie 把网页开在本机（Settings → 网页 →「此电脑」），再逐个问真实密钥并
  写进 local-router 同名 provider 的 keys。

### Fixed

- **QwenWork 签名路径修正**（`code 101 Signature invalid` 的根因）：COSY 签名
  覆盖的是 URL 的**路径部分**，此前把 query string（`?FetchKeys=…&Encode=1`）
  一起算进去了，并且多留了一层 `/algo` 前缀——两处都会让 MD5 对不上。修完
  有自检测试钉住签名。
- **WorkBuddy / QwenWork 续期后的 token 回写磁盘**：以前刷新出来的新 token
  只在内存里用，进程一退就丢，池里的文件还写着过期的那一份。
- **daemon 不再谎报启动成功**：`daemon start` 现在检查子进程是否真的活着，
  起不来（端口被占之类）直接报错并指向日志；`daemon run` 只在 pid 文件仍然
  属于自己时才删它，不会把后来启动的 daemon 的 pid 文件带走。

### Removed

- **控制台的「账号」页（`/accounts`）下线**：账号、额度、签到都收进供应商
  provider 的抽屉里（每个账号一行），不再单独开一页；后端
  `GET /api/accounts` 与 `POST /api/checkin` 一并去掉（逐账号的新接口取代
  了它们）。

### Changed

- **配置文件与会话文件的权限放宽到 0666**：`config.json` 与
  `workbuddy-session.json` / `qwen-session.json` / `trae-session.json`
  之前是 0600。它们装的是供应商密钥与登录 token，但容器（uid 10001）与
  其它本地工具经常需要读写同一个目录（例如把 `~/.config/local-router`
  直接挂进容器），0600 会让容器读不了也写不了。现在这些文件对本机其他
  进程可读写——**注意：同一台机器上的其它用户也能读到这些密钥**。
- **预设文案**：`WorkBuddy` / `QwenWork` 的名字去掉「用本机登录态」后缀
  （现在是浏览器登录），提示文案改为「(无需 key，使用浏览器登录)」。

## [0.1.1] - 2026-10-27

### Added

- **控制台国际化（English / 简体中文）**：所有界面文案抽到 `src/i18n/messages.ts`
  的双语词典，组件通过 `useT()` 取文案（`{name}` 变量插值）；侧边栏底部
  `EN` / `中` 一键切换，选择持久化到网关设置 `settings.locale`，未手动选择时
  按系统语言自动判定。
- **供应商详情改为右侧抽屉**（shadcn Sheet）：点击卡片从右侧滑出，展示状态、
  端点、凭据来源、对话路径、暴露模型、媒体模型与全部请求头规则。

- **上游 OAuth 浏览器登录**：provider 新增 `oauth` 配置（授权端点 / 令牌端点 /
  client_id / scopes / 交换风格），网关内置**通用 OAuth 2.0 + PKCE 引擎**：
  - 控制台「Add provider」里，带 OAuth 的厂商（OpenRouter 已内置）显示
    「Sign in with …」按钮——点一下，浏览器打开厂商登录页，签完凭证自动
    落回 provider，不用粘贴 API key；
  - 厂商回调落在网关自己的 `/oauth/callback`（与控制台同源），交换完成自动
    存 0600 配置；API 形状的路径仍返回 JSON 404，不会被 SPA 兜底吞掉；
  - 引擎配置驱动：同一引擎兼容 OAuth 2.0 标准表单交换与 OpenRouter 的
    JSON/`key` 变体；新增一家 OAuth 上游只需在预设里写几个 URL 字段。
  - 端点：`POST /api/providers/{id}/oauth`（发起）、`GET /oauth/callback`
    （回调）。
- **控制台 Add provider 搜索**：预设列表加搜索框，按名称 / id / 协议 / 地址
  过滤；带 OAuth 的厂商标 `OAUTH` 徽标。
- **路由组从已有模型里选成员**：Groups 新建面板不再要求手敲 `provider/model`，
  改为按供应商分组的复选列表（可搜索、显示所选顺序= failover 顺序）；
  没有模型时明确提示先加供应商。

- **七牛云（Qiniu AI）内置预设**：`provider add qiniu --key sk-…` 直接可用
  （base_url https://api.qnaigc.com/v1，OpenAI 兼容）。预设带
  `auto_include: ["*"]`，`provider models qiniu` 拉到的模型会**全部自动
  暴露**，厂商改模型名也不用手改配置。
- **CLI 的 `provider models` 也会应用 auto_include**（此前只有控制台的
  「刷新模型」会），并把新模型落盘。

- **登录后全自动装配（不再需要用户敲命令）**：WorkBuddy / QwenWork 的浏览器
  登录一完成，网关自动依次完成——① 创建并配置 provider、② 拉取厂商模型目录
  并并入 provider、③ 领取当日签到；控制台逐条提示（「已导入 N 个模型」
  「已自动签到 +N 额度」），CLI 的 `workbuddy login` 同样一次跑完。
  - 顺带修**账号身份没有保存**：`poll_login` 拿到 token 后没有查账号接口，
    会话文件里 `uid` / `nickname` 一直是空字符串，界面上看不到是谁登录的。
    现在登录当场补全身份并落盘；对已存在的空身份会话，`/api/accounts`
    会在读取时补拉一次并回写。
  - workbuddy / qwenwork 预设加 `auto_include: ["*"]`，厂商新增模型自动纳入。
- **WorkBuddy 登录修正（对照 magpie-community/plugins 的实现）**：
  - 登录链接补上两个必需参数——`version`（登录页读它决定走哪套流程）与
    `loginSessionId`（把这次浏览器标签与轮询绑定）；没有这两项，登录页
    能打开但流程不会完成；
  - `/v2/plugin/login/account` 现在会**重试 pending 码 12151**（token 与
    账号两个接口不同步，签完立刻查账号常收到「还没好」）；
  - 支持 **WorkBuddy AI（www.workbuddy.ai）**：与国内 CodeBuddy 同协议、
    不同站点，登录时按站点选（`platform=workbuddy` / `workbuddy-ai`）；
  - 原有的 `LOCAL_ROUTER_WORKBUDDY_ENDPOINT` 覆盖仍生效。
- **开机自启**：`local-router daemon install`（macOS 用 launchd、Linux 用
  systemd）把网关装成登录自启且崩溃自愈——实测杀掉进程 8 秒内自动拉起，页面
  照常可用；`daemon uninstall` 卸掉。新增 `daemon run`（前台运行并写 pid
  文件）供 launchd / systemd 调用。

- **WorkBuddy 多账号**：登录态文件改成账号池（`workbuddy-session.json`），
  可登录多个账号（`workbuddy login --account <名字>`）、列出
  （`workbuddy accounts`）、切换（`workbuddy use <名字>`）；网关的请求在
  多个账号之间**轮流走**，分摊额度，续期逻辑照旧。
- **登录链接可复制**：`workbuddy login` / `qwen login` 的授权链接始终打印在
  终端里（远程 shell、无桌面环境都能自己复制到浏览器打开），并新增
  `--no-open` 表示不自动唤起本机浏览器；交互模式同样支持
  `workbuddy login --no-open`。
- **交互模式增强**：`local-router console` 新增 `clear`（`clean`/`cls` 同义）
  清屏；`add` 支持 `--protocol`（openai_chat 默认 / anthropic / gemini /
  openai_responses）自定义协议；密钥与 URL 会自动去掉粘贴时带的中英文引号；
  `workbuddy` / `qwen` 支持子命令（login / status / credits / models /
  checkin / accounts / use）。

- **请求头重写**（对齐 [icebear0828/codex-proxy](https://github.com/icebear0828/codex-proxy)
  的同名能力）：provider 新增五组请求头规则——
  - `headers`：静态头注入，值支持 `{version}` / `{platform}` / `{arch}`
    模板占位符（未知占位符原样保留，JSON 花括号不误伤）；
  - `forward_headers`：把调用方发来的同名头**原样转发**给上游
    （Console Go 上游要求调用方自己的 `x-opencode-session`；没带就不造）；
  - `remove_headers`：发送前删除命名的头，最后执行、赢过其它规则
    （可删掉厂商不认识的默认头）；
  - `user_agent`：UA 模板，占位符同样展开；
  - `header_order`：上游头的发送顺序提示。
  CLI：`provider set-headers / forward-headers / remove-headers /
  header-order / user-agent / headers`；控制台 PATCH 同字段；6 个单元测试
  + 行为级冒烟（mock 记录收到的头逐一比对）。

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

- **WorkBuddy 桌面版的新版本把 token 加密存了**：客户端登录文件里的
  `accessToken` / `nickname` 变成 `{"$wbEncrypted": …}` 对象，旧解析直接报
  「not valid json」。现在能容错读取，并给出可行动的错误（提示改用
  `local-router workbuddy login`）。
- **中文厂商名会变成 id "custom"**：slug 只保留 ASCII 字母数字，中文名
  （如「七牛云」）被清空后全部落到 "custom" 并撞名；现在保留 Unicode 字母。
- **`local-router workbuddy login` 存了却读不到**：读取优先找桌面版客户端
  的文件，浏览器登录写进自己文件的会话因此被忽略；现在自己的文件优先。

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
