# 可观测性与远程控制 Roadmap

> 范围：在有人提议「加 OTel + Prometheus 遥测」「加 RPC/远程下发配置（key、路由规则、限流）」之后，对
> 现状的对齐、取舍判断与分阶段计划。本文是**设计方向文档**，不是已发布的变更（发版变更仍记在
> `CHANGELOG.md`）。

---

## 0. 结论速览

建议里**有一半已经长在仓库里**，所以这不是「引入两套新系统」，而是「给已有的数据面和控制面各补一个出口」：

- **遥测的数据层已存在**：`usage.db` 的 `usage_records` + `trace_spans`（span 树）+ `traces.jsonl`，控制台
  Usage/Traces/Dashboard，API `/api/usage`、`/api/traces*`、CSV/JSONL 导出。**缺的是实时指标出口**——
  没有 counter/histogram/gauge，没有 `/metrics`，Cargo.toml 里零个 metrics 依赖。
- **遥控的通道已存在一半**：`/api/*` 本身就是一个 HTTP RPC（provider / key / models / groups /
  settings(GET+PUT) / caller-keys 全部在线改、经共享 AppState 立即生效），CLI 复用同一 API 与 daemon
  冷却状态。而且 **config.json 的外部修改已经有热重载**（`AppState::sync()` 用 mtime 探测，下一个请求
  自动重载；`sync()` 只在 `store.load()` 成功时才替换，坏配置自动保留旧值）。**缺的是**：完整配置快照的
  原子导入/导出 + 版本号，以及「要遥控的限流对象」本身——现状只有 per-key in-flight 并发，没有 RPM/TPM。

据此定调：

| 诉求 | 决策 |
|---|---|
| Prometheus `/metrics` | **P0 做**：拉模式零推送基建，回答「现在怎么样」 |
| OTel / OTLP | **P2 做、feature flag 默认关**：只导 span 树到 Tempo/Jaeger；不 import OTel SDK 进核心路径 |
| 日志→OTLP | **不做**：与「有意不做审计级日志」冲突 |
| 远程控制 | **扩展现有 HTTP API（快照+版本+热重载）**，不开新协议、不做节点同步 hub |
| gRPC / 专用二进制协议 | **不做**：违背「一个二进制、有意不做多用户/TLS」 |
| 限流 | **P1 做**：先有可遥控的对象，RPM+TPM 滑动窗口 + 每日 token 预算 |

---

## 1. 遥测的取舍分析

### 1.1 Prometheus（P0，优先于 OTel）

价值：

- usage.db 是**事后账本**（每请求一行），Prometheus 回答的是**现在的状态与速率**：in-flight 数、
  每 provider QPS、P95/P99 延迟直方图、冷却中的 provider 数、队列深度、429 率、token 吞吐。
- 配 Grafana = 免开发实时面板；配 Alertmanager = 「主网关全线冷却」「某 provider 连续 429」告警。
- 级联拓扑（香港主网关 + 美国出口，两台都跑 local-router）下，Prometheus 各抓一份就能看全；trace id
  已逐跳下传，链路本就连通。
- 拉模式零推送基础设施：本机一个 `/metrics` 端点，不抓就零成本。契合「有意不做」的克制哲学。

成本/风险与对策：

- **依赖**：优先 `metrics` + `metrics-exporter-prometheus`；若嫌重，**手写文本端点**（exposition 格式
  很简单，几百行零依赖）作为备选路径。
- **基数爆炸**（最危险）：label = model × provider × caller_key × status × api，放任组合一周吃爆
  Prometheus。对策：写死 label 政策——provider / api / status_class 开放，caller_key 只出
  `has_key=true|false`，model 不直接做 label（用 model-group 或 top-N 白名单），并加测试卡住上限。
- **鉴权旁路**：`/metrics` 必须并入现有安全模型——回环免鉴权、`--lan`/非回环要求 bearer key
  （Prometheus scrape 原生支持 bearer_token）。否则它是绕过鉴权的新入口；且 label 会暴露 model/provider 名。
- **双真相**：Prometheus counter 重启归零、易失。**纪律：metrics 只做实时与速率，长期账仍是 usage.db。**

### 1.2 OTLP 导出（P2，feature flag）

- 真价值：内部 span 树（trace.rs）接 `opentelemetry-otlp` 投进 Tempo/Jaeger；级联两跳 trace id 已下传，
  理论能拼完整分布式链路。
- 成本：OTel Rust SDK 体积大、0.x API 剧烈变动、exporter batch/flush 与进程退出时机、async 集成复杂度。
- 形态：`telemetry.otlp.endpoint` 配置项 + feature flag（默认关）；关 feature 时二进制体积与行为不变。
- **不做**：logs 外发、采样策略研究（全量导出，量不大）。

---

## 2. 远程控制的取舍分析

### 2.1 方案对比

| 方案 | 判断 | 理由 |
|---|---|---|
| gRPC / 专用 RPC 协议 | ❌ 不做 | tonic/prost 依赖 + codegen，TLS/mTLS 又得自建（明确不做），新端口新面，违反「一个二进制」 |
| 配置中心 hub + 节点拉/推 | ❌ 不做 | 与 multi-machine-topology 决策冲突：有意级联而非节点机制，个人规模 N≤3 手动即可，发明节点协议是重复造轮子 |
| **扩展现有 HTTP API** | ✅ 做 | console API 即 RPC，补齐「快照 + 版本 + 校验」最后一公里 |
| SSH/Tailscale + 现有 CLI/API | ✅ 文档化 | `--lan` + caller key + 反代今天已可用，写成「远程运维手册」 |

核心论点：**控制平面已存在，别造第二个。** `/api/settings` 的 PUT、provider/groups 的 CRUD、daemon
共享 AppState 立即生效——这就是遥控。真正缺的拼图：

1. **完整快照 + 版本**：`GET /api/config/export`（全量配置，key 默认掩码，`include_keys` 显式才带出）+
   `PUT /api/config/import`（schema 版本 + 校验 + 原子写盘）；config 加 `revision`，PUT 带期望 revision
   做乐观并发（冲突 409）。用途：备份、两机迁移、git-ops 式管理、灾备。
2. **热重载**：config.json 外部修改已通过 `AppState::sync()` 的 mtime 探测自动生效；本阶段只需保证
   快照 import 走原子写（写临时文件再 rename）+ 校验失败保留旧配置（sync 已具备），并覆盖所有子配置。
3. **遥控对象要先存在**：要「远程下发限流」，先得有限流。现状只有 per-key in-flight 并发排队。最小集：
   per-provider / per-caller-key 的 RPM+TPM 滑动窗口 + 每日 token 预算（超限告警/软限），挂在现有 429
   冷却路径旁边。这是新 gateway 功能，工作量最大，独立成阶段。

### 2.2 安全边界

- 远程下发 key = 控制面传输秘密。**沿用既有模型，不引入新通道**：回环免鉴权、非回环要 bearer key、
  写请求 Origin 校验、config.json 0600；不开第二个端口、不搞 mTLS（公网走反代 + caller key）。
- export 兜底：`include_keys` 默认 false，防止有人把 `/api` 反代裸奔后整包配置泄漏（与 SECURITY 掩码模型一致）。

---

## 3. 分阶段计划

原则：每阶段独立可发布、可回退；先做「复用已有资产、零新协议」的，再引入新依赖；每阶段带验收。

### M1 — Prometheus `/metrics`（P0）

- `/metrics` 文本端点；指标集：请求计数 / 延迟直方图 / 错误率（by provider、api、status_class）、
  in-flight gauge、冷却中 provider gauge、队列深度、token 吞吐；**label 政策写进文档**；鉴权并入现有模型。
- 验收：curl 有值；mock 上游打量后 counter 正确变化；`--lan` 下无 key 请求 `/metrics` 返回 401；
  基数上限有单元测试。
- 备选：依赖嫌重则手写端点。

### M2 — 配置快照 + revision（P0，遥控地基）✅ 已实现

- `GET/PUT /api/config`（export/import，`include_keys` 默认 false）、`revision` 乐观并发、原子写（临时文件
  + rename）、schema 版本校验、结构校验（id 唯一、group 引用存在）；外部编辑继续经 mtime 热重载生效。
- 实现要点：`Config` 新增 `revision`/`schema_version`；每个写操作（含 `provider add` 等增量改动）
  都经 `AppState::update` 自增 `revision`；导入走 `AppState::commit_config`（save + 内存替换 + mtime）。
- 验收：A 机 export → B 机 import 后 `models` / `provider list` 一致；并发 PUT revision 冲突返回 409；
  坏配置 import 后服务不中断、旧配置保留。已通过 187 个 lib 测试 + 实机验证。

### M3 — 限流与预算（P1，先有可遥控的对象）✅ 已实现

- per-provider / per-caller-key RPM+TPM 滑动窗口（60s）、每日 token 预算（UTC 零点翻日）；
  上游级=跳过换目标（不冷却），调用方级=硬 429；CLI `provider limit <id> --rpm/--tpm`、
  控制台 Provider 卡片字段（PATCH，null/0 清除）；状态进 `/api/rate-limits`、
  `/v1/local-router/status`、`/metrics`（`local_router_rate_limited_total{scope}`、
  `local_router_rate_limits_active`）。
- 实现要点：`src/gateway/rate_limit.rs`（同步注册表，短临界区；`persist_usage`
  记真实 token 成本）；TPM 在请求完成后才入窗（发出前不知道成本）。
- 验收已过：RPM=2 时第 3 个请求 429（含正确文案）、调用方 rpm=1 时第 2 个请求 429、
  TPM 打满即拒、日预算超限拒；193 个 lib 测试通过。

### M4 — OTLP 导出（P2，feature flag）✅ 已实现

- **手写 OTLP/HTTP 导出**（不引 OTel SDK、零新依赖）：`build --features otlp` + `settings.otlp.endpoint`，把
  `session → request → upstream` span 树以 OTLP/HTTP 二进制 protobuf 发到 `{endpoint}/v1/traces`。
- 实现要点：`src/telemetry.rs`；字符串 trace/span id 用 SHA-256 确定性映射到 16/8 字节 W3C id
  （父/子链接不变），原始 id 以 `local_router.*` attribute 随行；按批导出（1s / ≤1024 span），
  关 feature 编译为 no-op。**为何手写**：构建镜像缺 `opentelemetry-sdk` crate，且项目哲学是
  「不引 OTel SDK 进核心路径」。
- 验收已过：194 个 lib 测试（含 span 编码单测）；实机以本地 OTLP collector 验证——带 session
  的请求导出 3 个 span（kind 1/2/3），共享同一 trace id，父/子链接一致，id 宽度正确。

### M5 — 运维文档 + 监控 compose 示例 + 可选 `local-router remote`（P3）✅ 已实现

- `examples/monitoring/`：Prometheus + Grafana + 网关 compose（含 scrape 配置、现成面板）、
  README；USAGE.md 新增「远程运维」一章（导出/导入、限流、遥控拓扑、可观测性出口）。
- `local-router remote status|models|providers|rate-limits|cooldowns|config --host <url> --key <k>`：
  薄封装直接打远端网关的 `/api`（不是新协议）；实机验证四个读操作 + 环回免 key。

- `compose.yaml` 监控示例（gateway + prometheus + grafana，dashboards json 放 `examples/`）；
- USAGE.md 新章「远程运维」（`--lan` + caller key + 反代拓扑下遥控两台机器、export/import 迁移手册）；
- `local-router remote` 薄 HTTP 封装（可选，不做也行，SSH + curl 足够）。

---

## 4. 明确不做清单（防 scope creep）

- gRPC / 自定义二进制控制面协议
- 节点注册 / 服务发现 / 配置自动同步 hub（违反 multi-machine-topology 决策）
- 内置 TLS / mTLS
- 审计级日志与日志外发管道
- 内置时序库或长周期聚合（实时归 Prometheus，账本归 usage.db）

---

## 5. 依赖关系

```
M1  /metrics            （独立先行）
   ↓
M2  配置快照+revision    （遥控地基）
   ↓
M3  限流与预算           （依赖 M2 才有配置落点）
M4  OTLP 导出            （依赖 M1 的指标定义稳定，可并行于 M3）
M5  文档+监控示例+remote （收尾）
```
