# 配置参考

local-router 的配置是**一个 JSON 文件 + 若干运行时文件**，没有数据库、没有
配置中心、没有 YAML/TOML 变体。原则：

- **一份真源**：`config.json` 存所有「人配的东西」（上游、路由组、设置、
  调用方 key）；
- **其它文件各管一摊**（价目表、会话簿、项目登记簿、登录态、用量账本），
  互相不混；
- **改完即生效**：进程用 mtime 探测文件变化，**下一个请求**自动重载，
  不用重启；
- **三层覆盖**：CLI 参数 > 环境变量 > 配置文件 > 内置默认值。

---

## 1. 文件都在哪

`~/.config/local-router/`（`LOCAL_ROUTER_HOME` 可覆盖，Docker 里是 `/data`）：

| 文件 | 装什么 | 谁写 | 格式 |
| --- | --- | --- | --- |
| `config.json` | **主配置**：providers / groups / settings / caller_keys | CLI、控制台、你手改 | JSON（0600） |
| `prices.json` | 价目表覆盖（按厂商模型 id） | 你手改 | JSON |
| `sessions.json` | 会话标题簿（会话 id → 标题/计数） | 网关自动 | JSON |
| `projects.json` | 项目登记簿（目录 → 名称/颜色/预算） | 网关 + 控制台 | JSON |
| `usage.jsonl` | 每请求一行的用量账本 | 网关自动 | JSONL（>8 MB 翻页） |
| `usage.db` | 同一份用量的 SQLite 视图 | 网关自动 | SQLite（可关） |
| `workbuddy-session.json` | WorkBuddy 登录态 token | 浏览器登录时 | JSON（0600） |
| `qwen-session.json` | QwenWork 设备流 token | 设备流登录时 | JSON（0600） |
| `daemon/daemon.log` | 守护进程日志 | 自动 | 文本 |

> 只有 `config.json` 是「配置」，其余是**运行数据**——备份配置只需要备份
> `config.json`（+ `prices.json`）。

---

## 2. 配置模型

```
              ┌─ CLI 参数        （local-router serve --port 8080）
优先级 高 ──→ ├─ 环境变量        （LOCAL_ROUTER_GATEWAY_PORT=8080）
              ├─ config.json     （settings.gateway_port）
              └─ 内置默认值      （3435）
```

- **CLI 参数**只在启动时生效（端口、地址、是否开控制台）。
- **环境变量**同理（容器里最常用）。
- **`config.json`** 是运行时唯一会被热重载的那层：改了之后，下一个请求
  就用新配置（`AppState::sync()` 比较 mtime）。
- 配置文件里**没有**的字段一律走内置默认值（serde `#[serde(default)]`），
  所以老配置文件能直接被新版本读懂。

---

## 3. `config.json` 格式

顶层四个键，缺一个都能补默认值：

```json
{
  "providers":   [ /* 上游 */ ],
  "groups":      [ /* 路由组 */ ],
  "settings":    { /* 全局设置 */ },
  "caller_keys": [ /* 别的机器能用的调用 key */ ]
}
```

### 3.1 providers[] —— 一个上游

```json
{
  "id": "deepseek",
  "name": "DeepSeek",
  "kind": "deepseek",
  "enabled": true,
  "protocol": "openai_chat",

  "base_url": "https://api.deepseek.com/v1",
  "anthropic_base_url": null,
  "responses_base_url": null,
  "gemini_base_url": null,

  "keys": [{ "id": "7f6724dd", "key": "sk-…", "enabled": true, "label": null }],
  "models": ["deepseek-chat", "deepseek-reasoner"],
  "auto_include": [],
  "catalog": "deepseek",

  "headers": {},
  "user_agent": null,
  "forward_headers": [],
  "remove_headers": [],
  "header_order": [],

  "group_only": false,
  "stream_only": false,
  "hold_when_exhausted": false,
  "media_models": [],
  "media_paths": null,
  "proxy": null,
  "auth": null
}
```

| 字段 | 含义 | 备注 |
| --- | --- | --- |
| `id` | 模型名里的前缀（`deepseek/deepseek-chat`） | 唯一 |
| `protocol` | 上游说哪种话 | `openai_chat` / `openai_responses` / `anthropic` / `gemini` / `qwenwork` |
| `base_url` 等四个 | 同一家的不同协议各一个地址 | 留空则回落到 `base_url` |
| `keys` | 这个家的多把 key | 故障时逐把试 |
| `models` | **对外暴露的模型白名单** | 路由到没列出的模型会 404 |
| `auto_include` | glob（只支持 `*`） | 刷新模型时匹配的自动并入 `models` |
| `headers` | 每个请求都带的静态头 | 值里 `{version}`/`{platform}`/`{arch}` 会展开 |
| `user_agent` | UA 模板 | 同样支持占位符；不设则按 `send_agent_user_agent` 透传 agent 的 |
| `forward_headers` | 把**调用方**的同名头原样转发给上游 | 没带就不造一个 |
| `remove_headers` | 发送前删掉这些头 | **最后执行**，能删掉鉴权头 |
| `header_order` | 上游头发送顺序 | 纯提示，多数厂商不在乎 |
| `stream_only` | 上游只收流式 | 网关会收流再拼成整段回复 |
| `hold_when_exhausted` | 配额空了就不往这家发 | 默认关（有些账号 0 额度仍可用） |
| `group_only` | 只允许通过路由组访问 | |
| `media_models` | 这家能做图/视频的模型 | 会出现在 `/v1/models` 里 |
| `media_paths` | 厂商自己的媒体路径 | 不设就走 OpenAI `/v1` 家族 |
| `auth` | `"workbuddy"` / `"qwenwork"` | 用本机登录态，免 key |

### 3.2 groups[] —— 一个路由组

```json
{
  "id": "mixed",
  "name": "mixed",
  "models": ["deepseek/deepseek-chat", "glm/glm-4.6"],
  "routing": "weight",
  "weights": { "deepseek/deepseek-chat": 3, "glm/glm-4.6": 1 },
  "stays": "auto",
  "listed": true,
  "note": null
}
```

| 字段 | 取值 | 含义 |
| --- | --- | --- |
| `routing` | `smart` / `order` / `rotate` / `usage` / `weight` | 怎么挑成员 |
| `stays` | `auto` / `session` / `turn` / `off` / `strict` | 一段对话粘在谁身上 |
| `weights` | 成员 → 整数（1–1000） | 只在 `routing: weight` 时用 |
| `listed` | true/false | 组名是否出现在模型目录里（`group/mixed`） |
| `models` | 成员 | 可以是 `provider/model`，也可以是 `group/别的组`（嵌套） |

### 3.3 settings —— 全局设置

```json
{
  "gateway_port": 3435,
  "console": true,
  "lan": false,
  "gateway_addr": "127.0.0.1",
  "default_caller_key": "local-router",
  "send_agent_user_agent": false,
  "masking": false,
  "record_usage": true,
  "locale": "zh-CN",
  "cors_origins": [],
  "workbuddy_auto_checkin": true
}
```

| 字段 | 作用 |
| --- | --- |
| `gateway_port` | 网关与控制台**共用**的端口（`0` = 让系统挑） |
| `console` | 要不要提供控制台页面（false = 只提供模型 API） |
| `lan` / `gateway_addr` | 绑定地址；`lan: true` 等价于 `0.0.0.0` |
| `default_caller_key` | 调用方没带 key 时用哪把 |
| `send_agent_user_agent` | 把 agent 的 User-Agent 转给上游 |
| `masking` | 遮蔽提示词里的密钥再发出去 |
| `record_usage` | 是否记账 |
| `locale` | 控制台语言 |
| `cors_origins` | 允许哪些浏览器源跨域调网关（默认空 = 不允许） |
| `workbuddy_auto_checkin` | 是否自动领 WorkBuddy 每日签到 |

> `web_port` / `web_addr` 是历史字段：控制台已与网关同端口，值被忽略，
> 只是留着让老配置文件能读。

### 3.4 caller_keys[] —— 别的机器能用的 key

```json
[{ "id": "k1", "name": "laptop", "key": "sk-…", "enabled": true,
   "daily_tokens": null, "monthly_cost": null }]
```

---

## 4. 其它文件的格式

### `prices.json` —— 价目表覆盖

按**厂商的模型 id**（不是 `provider/model`），所以一家模型被多个上游卖时
只写一次：

```json
{
  "hy3": {
    "input": 0.5,
    "output": 2.0,
    "cache_read": 0.1,
    "cache_write": null,
    "credits_per_million": 200.0
  }
}
```

单位都是「每百万 token」。`credits_per_million` 是订阅制厂商自己的额度
折算率。写完下一个请求就按新价算（不用重启）。

### `sessions.json` —— 会话标题簿

```json
{
  "sessions": {
    "s-abc": { "id": "s-abc", "title": "帮我重构网关模块", "first_seen": "…", "last_seen": "…", "requests": 12 }
  }
}
```

### `projects.json` —— 项目登记簿

```json
{
  "projects": {
    "/Users/me/work/acme": {
      "id": "/Users/me/work/acme",
      "name": "Acme",
      "color": "#9ef0bf",
      "created_at": "…",
      "budget_tokens": null,
      "budget_cost": null
    }
  }
}
```

---

## 5. 什么不能配（写死在代码里）

诚实起见，这些是**行为契约**，不是配置项：

| 行为 | 说明 |
| --- | --- |
| 四种入口协议的端点与形状 | `/v1/chat/completions`、`/v1/responses`、`/v1/messages`、`/v1beta/models/{model}:generateContent` —— 协议本身就是契约 |
| 协议互译规则 | 工具调用怎么翻、思考块怎么带、stop reason 怎么映射 |
| 遮蔽的密钥模式 | 前缀型（`sk-`/`ghp_`/`AKIA`/`AIza`/`xox`/JWT）+ 长 hex（≥24 且含字母数字） |
| `auto_include` 的 glob 语法 | 只支持 `*`（任意位置），不支持 `?`/字符类/正则 |
| 会话粘性的判定 | `stays: turn` 看请求里有没有 assistant/tool 轮次 |
| 用量估算口径 | 约 3.6 字符/token |
| 安全模型 | 回环免鉴权、Origin 校验、`config.json` 0600 |

---

## 6. 改配置的三种方式

```sh
# ① 命令行（写回 config.json）
local-router provider add deepseek --key sk-…
local-router group add mixed --models a/m1,b/m2 --routing weight
local-router group weights mixed "a/m1=3,b/m2=1"
local-router provider set-headers deepseek "x-profile=gw-{version}"
local-router provider forward-headers deepseek "x-opencode-session"
local-router provider remove-headers deepseek "anthropic-version"
local-router provider header-order deepseek "user-agent,authorization"
local-router provider user-agent deepseek "my-relay/{version} ({platform} {arch})"

# ② 控制台（PATCH /api/providers/{id}、/api/groups/{id}）

# ③ 手改 config.json（改完下一个请求自动生效）
```

交互模式（`local-router console`）里也能用这些能力，命令见
[USAGE.md](../USAGE.md)。

---

## 7. 排障

**改了 config.json 没反应？**
网关在**下一个请求**才重载（mtime 探测）。直接看 `local-router doctor`
或敲一次 `/v1/models` 就会重载。

**配置写坏了？**
`config.json` 不是合法 JSON 时，网关**不会崩**：它会警告并退回默认配置
（`config at … is not valid json`）。CLI 侧则会报错退出。

**密钥会落盘吗？**
会——`config.json` 里就是明文 key，文件权限 0600。想让某个 key 只在内存，
用登录态型上游（WorkBuddy / QwenWork）或 `LOCAL_ROUTER_DEFAULT_KEY`
（后者只在进程内存）。

**想看当前生效的配置？**
`local-router ls`（概览）、`local-router doctor`（自检）、
`local-router provider headers <id>`（某个上游的规则）。
