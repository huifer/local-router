# 用量与账单

每个请求都记一笔账：token、时延、状态、美元成本、订阅 credits。
本页讲记录路径与账单口径；日常命令见 [USAGE.md § 2.3](../USAGE.md#23-命令行速查)。

## 记录路径

- `usage.jsonl`：每请求一行（模型、provider、token、时延、状态、成本），
  超过 8 MB 自动翻页成 `usage.jsonl.1`（`LOCAL_ROUTER_USAGE_MAX_BYTES` 可调）。
- `usage.db`：同一份记录的 SQLite 视图。写入走单连接 + WAL，本地开销可忽略；
  控制台 / HTTP API 的聚合（按天、按平台、按模型、总账）直接 SQL 查询，
  不扫全量日志。
- `LOCAL_ROUTER_DISABLE_SQLITE=1` 可关掉 SQLite，退回纯 JSONL。
- `record_usage: false` 时两条路径都不写——账本与链路追踪是同一条写入路径。

**双真相纪律**：`usage.db` 是事后账本（每请求一行，重启不丢）；
Prometheus `/metrics` 只做实时与速率（counter 重启归零）。两者不是同一套
真相，不要互相替代。

## 每条记录的钱

```json
{"model":"workbuddy/hy3","input_tokens":15,"output_tokens":128,
 "cost_usd":0.000148,"credits":0.0172,"priced":true}
```

- **cost_usd**：按厂商公开价目表（每百万 token）估算的美元成本。
- **credits**：订阅制厂商（如 WorkBuddy）自己额度单位的估算消耗。
- **priced=false**：该模型没有价格记录，钱无法估算（用量页会计数提示）。

价目是估算：订阅制厂商的 credit 折算率来自套餐额度与公开价的比例，可能与
厂商实际扣减有偏差。改价 / 加新厂商写 `~/.config/local-router/prices.json`
（热生效，不用重启）：

```json
{"hy3": {"input": 0.5, "output": 2.0, "credits_per_million": 200.0}}
```

## Token 消耗口径

控制台 **Usage → Token 消耗** 是专门看 token 的页面（总览自身的两张图也能在
「请求数 / TOKEN」间切换）：按小时、按天的堆叠图拆成 **输入 / 缓存 / 输出** 三层，
下面接按模型、按平台的消耗排行（可按总量 / 输入 / 输出 / 缓存排序），以及最近
200 条请求里最耗 token 的明细。

口径写死在一处，避免各面板互相矛盾：

```
total_tokens = input_tokens + output_tokens + cache_read_tokens + cache_write_tokens
```

- **缓存读取、缓存写入也是消耗**：缓存命中只是单价低，不是没消耗。缓存密集的
  agent 里它往往占一半以上，漏算会让按天曲线远低于总账。
- **推理 token 不计入总量**：它是 `output_tokens` 的细分，单独一格展示，
  重复相加会翻倍。
- **失败请求计请求数、不计 token**：与 `usage::totals` 一致。

`/api/usage` 的 `requests_timeline[]` 每个小时点带全部 token 字段与
`total_tokens`；`by_day[]` / `by_model[]` / `by_provider_costs{}` 都返回完整
`Totals`。`GET /v1/local-router/usage` 与 `/v1/local-router/usage/requests`
额外返回 `total_tokens`，其 `per_provider[]` 也带缓存与推理 token。

## 账单的三个维度

`/api/usage` 返回 `by_provider_key`、`by_caller_key` 与 `recent` 明细；
`local-router usage --csv` 包含同样维度：

- **上游 Provider key**：`provider / key_label`（无 label 显示 `key_id`）——
  请求数、失败数、输入/输出/缓存/推理 token、成本、credits。
- **调用方 caller key**：哪个 agent / 客户端用了网关的哪把 key，
  同样的 token 与成本合计。
- **agent**：从调用方 User-Agent 识别。

上游 key 与调用方 key 是**两套不同凭据**，账单刻意分开统计，不合并。

## 查询入口

```sh
local-router usage --period today    # 按 provider / 模型，含 $ 与 credits
local-router usage --csv             # 机器可读
local-router prices                  # 当前价目表

curl -H "Authorization: Bearer <key>" \
  'http://127.0.0.1:3435/v1/local-router/usage?period=7d'
# → cost_usd / credits / unpriced 汇总 + per_provider[] 每个平台的账单
```

控制台：

- **Usage → Token 消耗**：token 总量、逐小时 / 逐天堆叠图、按模型与平台的
  消耗排行、最耗 token 的请求明细（口径见上节）。
- **Usage → Bills**：按上游 key / 调用方 key / agent 三个维度分账。
- **Overview**：顶部指标含总 token 与缓存读取；请求时间线、每日图可在
  请求数与 token 间切换；每平台 / 每模型横条带 token 总量；模型占比甜甜圈；
  页面顶部的 `SQLITE` / `JSONL` 标签标明当前存储。

## 直接用 SQL

```sql
sqlite3 ~/.config/local-router/usage.db \
  "SELECT provider, count(*), sum(cost_usd), sum(credits)
   FROM usage_records WHERE time >= '2026-10-26' GROUP BY provider;"
```

## 会话 / 项目 / 上下文

数据全部来自请求本身，与账单同源：

- **Sessions**：每个会话一行——标题取首个用户消息第一行；agent 带
  `x-session-id`（或 Anthropic `metadata.user_id`）即被记录。
- **Projects**：按工作目录分组。agent 上报 CWD（`x-project` / `x-project-dir` /
  `x-cwd` 头，或 body 的 `metadata.cwd`）后自动建档；控制台可改名、配色、
  设 token/费用预算、合并重复目录（合并会重键两个存储里的历史记录）。
- **Context**：每个请求的上下文窗口拆解——系统提示 / 工具定义 / 附件 /
  对话各占多少 token（按 ~3.6 字符/token 估算），窗口填充率与缓存友好度
  评分（窗口大小未知时填充率显示 0）。
- **Quota**：登录态上游的额度窗口（见 [PROVIDERS.md](PROVIDERS.md)），
  路由视角另有 `GET /v1/local-router/quotas`。

## 相关文档

- 链路追踪模型：[TRACING.md](TRACING.md) · `/metrics` 指标：[API.md](API.md)
- 压测口径（账本是吞吐瓶颈）：[BENCHMARK.md](BENCHMARK.md)
