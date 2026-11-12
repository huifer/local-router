# 网关 API 参考

网关与控制台共用一个端口（默认 `3435`）。本页是端点与鉴权的参考；
调用示例与场景见 [USAGE.md § 2.5](../USAGE.md#25-调用-api)。

## 鉴权

网关推理端点接受四种鉴权头（任选其一）：

```
Authorization: Bearer <key>
x-api-key: <key>
x-goog-api-key: <key>
api-key: <key>
```

key 的取值顺序：`LOCAL_ROUTER_DEFAULT_KEY`（环境变量，**不落盘**）→
`settings.default_caller_key` → 任一 caller key。一把 key 都没有时，
网关**拒绝一切请求**。

- 回环地址上的控制台页面免鉴权（本机信任）；
- 绑到非回环地址（`--lan`、容器、反代）时，`/api` 与 `/metrics` 都需要
  同一套 key，写请求另校验 `Origin`（跨站写一律 403）；
- 跨域调用默认不允许，需在 `settings.cors_origins` 显式放行。

## 模型命名

| 写法 | 含义 |
| --- | --- |
| `provider/model` | 点名某个上游的某个模型 |
| `group/id` | 走一个路由组 |
| 裸模型名（如 `deepseek-chat`） | **有且仅有一个**上游提供时直接路由；两个都提供则报错，一个都没有报 404 |

provider 的 `models` 列表既是对外白名单，也是转发给上游的模型 id；
完整字段见 [CONFIG.md](CONFIG.md)。

## 推理入口

四种入口协议与上游协议自由组合；调用方不传 `stream` 时拿整段回复，
上游只支持流式时网关收流后拼整段。

| 端点 | 协议 |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI Chat（流式 / 非流式） |
| `POST /v1/responses` | OpenAI Responses |
| `POST /v1/messages` | Anthropic Messages |
| `POST /v1/messages/count_tokens` | Anthropic token 估算 |
| `POST /v1beta/models/{model}:generateContent` | Gemini |
| `POST /v1beta/models/{model}:streamGenerateContent` | Gemini 流式 |
| `GET /v1/models` | 模型目录（OpenAI list 形状，含能力元信息） |

翻译范围：系统提示、多模态消息、工具定义 / 调用 / 结果、思考块
（含 signature）、stop 原因、token 用量（缓存读写、思考 token）。
上游忽略 `stream=true` 返回 JSON 时，网关自动转成事件流。

## 媒体端点

图片与视频走厂商原生端点，网关做转发与统一入口。模型要先声明
（`local-router provider media <id> <模型...>`），声明后会出现在
`/v1/models` 并带 `modalities` / `native_endpoints` 元信息。

| 端点 | 用途 |
| --- | --- |
| `POST /v1/images/generations` | 图片生成 |
| `POST /v1/images/edits` | 图片编辑（multipart） |
| `POST /v1/videos` | 创建视频任务（异步） |
| `GET /v1/videos/{id}` | 查询任务状态 |
| `GET /v1/videos/{id}/content` | 取成品文件 |
| `DELETE /v1/videos/{id}` | 取消任务 |

厂商自己的媒体路径可用 provider 的 `media_paths` 覆盖（如 WorkBuddy
把媒体放在 `/v2` 且视频无任务状态接口，网关会自己兜住状态与下载）。
媒体用量不计入 chat token 统计，但计入用量请求数。

## 内部状态端点

| 端点 | 用途 |
| --- | --- |
| `GET /health` | 健康检查 |
| `GET /v1/local-router/status` | 运行状态 |
| `GET /v1/local-router/quotas` | 各上游额度窗口（路由视角） |
| `GET /v1/local-router/usage?period=today\|24h\|7d\|30d\|all` | 用量汇总（含 `by_provider_key` / `by_caller_key`） |
| `GET /v1/local-router/usage/requests?limit=N` | 逐请求明细 |
| `GET /v1/local-router/route` | 这次会路由到哪（带 `X-Session-ID` 可看粘性） |

## 控制面（`/api/*`）

控制台页面与 CLI 共用这组 HTTP API，全部在线生效：

| 端点 | 用途 |
| --- | --- |
| `GET/PUT /api/config` | 完整配置快照（`?include_keys=true` 才带明文）；写入带 `revision` 乐观并发，过期写 409 |
| `GET/POST/PATCH/DELETE /api/providers…` | 上游 CRUD；`POST /api/providers/{id}/test` 直接测试对话；`POST /api/providers/{id}/refresh` 拉厂商模型目录 |
| `GET/POST/PATCH/DELETE /api/groups…` | 路由组 CRUD |
| `GET /api/models` · caller-keys · settings | 模型目录 / 调用方 key / 全局设置 |
| `GET /api/cooldowns` | 当前 429 冷却状态 |
| `GET /api/rate-limits` | 限流配置与窗口用量 |
| `GET /api/usage` · `/api/usage/...` | 账单与明细（见 [BILLING.md](BILLING.md)） |
| `GET /api/traces` · `/api/traces/{id}` · `/api/traces/sessions…` · `/api/traces/export` | 链路查询与导出（见 [TRACING.md](TRACING.md)） |
| `GET /metrics` | Prometheus 文本指标（实时状态；长期账本仍是 `usage.db`，见 [BILLING.md](BILLING.md)） |

## 相关文档

- 调用示例与 agent 接入：[USAGE.md](../USAGE.md)
- 配置字段：[CONFIG.md](CONFIG.md)
- 链路模型：[TRACING.md](TRACING.md) · 账单口径：[BILLING.md](BILLING.md)
