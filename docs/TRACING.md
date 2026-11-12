# 链路追踪：会话 ↔ 上游调用

一个大任务往往被拆成很多次模型调用，分布在多个厂商上。local-router 是
唯一同时看到「agent 的请求」和「厂商的回答」的地方，所以它把两边的关联
记下来。日常查询用法见 [USAGE.md 场景 L](../USAGE.md#场景-l想知道一条会话到底花了多少上游调用)，
本页讲模型与落盘格式。

## span 树模型

```text
session  ses_…          agent 的会话 —— 一次会话就是一条完整的链路
 ├─ request  req_…       agent 发的一次请求
 │   └─ upstream up_…    对某个厂商的一次尝试（含失败、降级、被跳过的）
 └─ request  req_…
     └─ upstream up_…
```

两个视图，别混：

- **按会话（默认）**：一个会话 = 一条完整链路。同一会话的请求可能带不同的
  trace id（编排层给每个子任务起一个），会话视图把它们跨 trace 收成一棵树。
- **按任务（trace）**：一条 trace = 一个任务自己的树。trace id 优先用调用方
  自带的 `x-trace-id`（或 body 里的 `metadata.trace_id`），没有就由会话派生
  （`trc_<hash>`）。

## id 的下传与反查

**trace id 逐层下传**。每个上游请求都带这些头发出去：

```
x-trace-id / x-span-id / x-session-id / x-request-id / x-task-id
```

厂商（或厂商前面的中转）自己的日志里就有这个 id，可以从一条上游日志反查回
本地的会话与步骤。调用方自己带 `x-trace-id` 时网关沿用它，编排层的 trace
与网关的 trace 是同一条；`x-task-id` 是可选的子任务名，用于在链路里分出
「第几步」。

**厂商自己的 request id 也留着**。上游返回的 `chatcmpl-…` / `msg_…` 会写进
span——厂商账单或日志里的一条记录，可以反查回是哪次会话的哪一步。

`trace_metadata` 设置可以把 trace id 写进请求体 metadata（默认关，避免厂商
拒绝未知字段）；请求头里始终带着。

## 失败也留痕

每次厂商尝试都开一个 upstream span：失败、降级、被 429 冷却**跳过**的尝试
都单独标记（`skipped` 不算失败），所以一条会话链路能看到完整的尝试序列。

## 查询入口

```sh
local-router traces                       # 最近的链路（按任务）
local-router traces --session sess-42     # 只看某个会话
local-router traces --provider anthropic  # 只看碰过某厂商的链路
local-router trace --session sess-42      # 一个会话的完整链路（树）
local-router trace trc_16c1f6a3d4ba4a08   # 一条任务的树
```

控制台 Traces 页默认「按会话」——每个会话一行，点开是完整链路；切「按任务」
看单条 trace。HTTP API：`GET /api/traces`、`/api/traces/{id}`、
`/api/traces/sessions`、`/api/traces/sessions/{id}`、`/api/traces/export`
（csv / jsonl）。

## 落盘

- SQLite `usage.db` 的 `trace_spans` 表 + 同目录 `traces.jsonl`
  （每行一个 span，超过 `LOCAL_ROUTER_TRACE_MAX_BYTES` 翻页）。
- `record_usage` 关掉时链路也不记——账本与链路是同一条写入路径。
- 级联拓扑下 trace id 逐跳下传：两台 local-router 的链路是同一条，
  在任意一边都能查全；Prometheus 各抓一份、OTLP 指向同一收集器即可看全。

## 相关文档

- 账本与用量：[BILLING.md](BILLING.md) · 实时指标：[API.md](API.md) 的 `/metrics`
- 复现与压测口径：[BENCHMARK.md](BENCHMARK.md)
