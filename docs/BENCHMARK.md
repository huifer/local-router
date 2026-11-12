# 基准测试（Benchmark）

> 范围：local-router 网关在本机回环下的吞吐与延迟基线，覆盖 1 → 2000 并发、
> 流式、慢上游、账本开关与行为验证（冷却 / 故障转移 / 本地限流）。
> 本文是**实测记录**，不是承诺值：你的数字会随机器、上游与请求形状变化。

---

## 0. 结论速览

- **默认配置（`record_usage` 开）下，网关吞吐封顶在 ~2.5–3k rps**；并发越高，
  延迟线性上涨（排队），这是设计内的行为，不是故障。
- 瓶颈是**用量账本的落库路径**：单条 SQLite 连接 + 全局互斥锁，每请求串行写
  1 条 usage + 2 条 trace span。关掉 `record_usage` 后同一台机器跑到 **13–19.5k rps**，
  差距 5–7×。
- 网关本身在 2000 并发、40 万次请求下**零错误**、无警告、RSS 稳定在 ~200MB。
- 行为特性（上游 429 全局冷却、组内故障转移、本地 RPM 限流不触发冷却）全部
  实测符合 [USAGE.md](../USAGE.md) 第 8 章的设计。

| 场景 | 吞吐 | p50 |
|---|---|---|
| 默认配置，c=50 | ~3.0k rps | 16ms |
| 默认配置，c=1000 | ~2.6k rps | 380ms |
| 关账本，c=100 | **~19.5k rps** | 5ms |
| 关账本，c=2000 | ~16.7k rps | 108ms |

---

## 1. 测试环境

| 项 | 值 |
|---|---|
| 机器 | Apple M5 Pro（15 核），macOS，48GB |
| 网关 | local-router 0.1.1，release 构建（`opt-level=3, lto=thin`），rustc 1.97 |
| 拓扑 | 负载发生器 → 网关(127.0.0.1:18100) → 上游桩(127.0.0.1:18101) ，全部本机回环 |
| 负载 | `POST /v1/chat/completions`，约 60B 请求体 / ~200B 响应，keep-alive |
| 上游桩 | Node 单进程，延迟 0ms（另测 100ms 档） |
| 冷却参数 | `LOCAL_ROUTER_COOLDOWN_SECS=5`（只为冷却场景；吞吐场景无 429） |

**口径**：以下数字全部是「同一台 15 核机器上，网关 + 上游桩 + 负载发生器挤在
一起」的保守值。生产里网关独享 CPU、上游在远端，吞吐只高不低；但延迟分位数
不含真实网络与模型推理时间（那部分通常比网关开销大几个数量级）。

---

## 2. 台架（harness）

仓库自带两个零依赖脚本：

- `scripts/benchmark.mjs` —— 负载发生器。**多进程**：Node 单进程的事件循环
  会在几千连接时先于网关饱和，还会撞上 macOS 临时端口耗尽（`EADDRNOTAVAIL`）。
  它把并发预算切成 N 个 worker 进程（默认 4 个），各自 keep-alive 连接池，
  最后合并延迟样本算分位数。
- `scripts/mock_vendor.mjs` —— 高并发上游桩。HTTP/1.1 keep-alive、backlog 8192、
  `--delay-ms` 可模拟慢上游、`--fail` 可模拟上游报错。

> 为什么不用 `scripts/mock_vendor.py` 做大并发：那个桩是 HTTP/1.0 + 每响应
> 断连 + 线程模型 + 默认 backlog=5。拿它压大并发，502 会先在**桩**上出现
> （网关报 `error sending request for url`），压的是桩不是网关。小并发冒烟
> 它仍然够用。

---

## 3. 吞吐 / 延迟阶梯（非流式）

### 3.1 上游直连基线（负载发生器 → 上游桩，不经网关）

| 并发 | rps | p50 | p95 | p99 | 错误 |
|---|---|---|---|---|---|
| 200 | 36,792 | 5.0ms | 8.2ms | 11.2ms | 0 |
| 1000 | 35,099 | 22.6ms | 34.5ms | 41.5ms | 82（桩自身拒绝，见 §7） |

台架上限 ~35–37k rps，是网关默认配置吞吐的 10 倍以上——网关才是被测对象。

### 3.2 默认配置（`record_usage: true`）

| 并发 | rps | p50 | p95 | p99 | 最大 | 错误 | 网关 CPU 均值 |
|---|---|---|---|---|---|---|---|
| 1 | 1,122 | 0.6ms | 2.4ms | 6.0ms | — | 0 | — |
| 10 | 2,619 | 2.5ms | 8.8ms | 13.3ms | — | 0 | — |
| 50 | 3,024 | 15.8ms | 23.1ms | 33.3ms | — | 0 | — |
| 100 | 2,850 | 33.6ms | 49.0ms | 61.5ms | 91ms | 0 | 150% |
| 200 | 2,714 | 69.5ms | 103.9ms | 205.3ms | 327ms | 0 | 145% |
| 500 | 2,243 | 200.4ms | 357.1ms | 491.1ms | 624ms | 0 | 133% |
| 1000 | 2,593 | 379.9ms | 478.7ms | 536.1ms | 610ms | 0 | 136% |
| 2000 | 1,349 | 1488ms | 1789ms | 1941ms | 2187ms | 0 | 72%（客户端先饱和） |

读法：吞吐封顶后，**延迟 ≈ 并发 ÷ 吞吐**（Little's law），即纯排队。
c=100 → 33ms、c=1000 → 380ms，全部是等锁时间。

### 3.3 关闭账本（`record_usage: false`，热重载切换）

| 并发 | rps | p50 | p95 | p99 | 错误 | 网关 CPU 均值 |
|---|---|---|---|---|---|---|
| 100 | **19,529** | 4.9ms | 6.9ms | 9.2ms | 0 | 314% |
| 200 | 17,079 | 10.6ms | 15.7ms | 24.9ms | 0 | 286% |
| 500 | 13,832 | 33.2ms | 60.7ms | 110.9ms | 0 | 238% |
| 1000 | 13,216 | 64.9ms | 122.3ms | 266.6ms | 1302¹ | 235% |
| 2000 | 16,742 | 107.9ms | 167.2ms | 265.1ms | 1473¹ | 282% |

¹ c≥1000 时的 502 来自**上游桩**（Node 单进程在万级 rps + 突发下偶发拒绝），
不是网关逻辑错误；错误体为 `error sending request for url`。见 §7。

`record_usage` 开与关之间 5–7× 的差距就是账本落库的代价（分析见 §5）。

### 3.4 流式（SSE，默认配置）

| 并发 | rps（请求/秒） | p50 | p95 | p99 | 错误 |
|---|---|---|---|---|---|
| 100 | 2,579 | 37.1ms | 57.3ms | 75.6ms | 0 |
| 200 | 1,836 | 84.0ms | 232.6ms | 373.3ms | 0 |
| 500 | 1,402 | 389.8ms | 549.3ms | 623.7ms | 0 |

流式比非流式慢约 30–40%：每请求逐帧转发 + SSE 成帧。对真实 agent 场景
（一次会话一两个流式请求）这个差距无关紧要。

### 3.5 慢上游（桩延迟 100ms，默认配置）

| 并发 | rps | p50 | p99 |
|---|---|---|---|
| 200 | 1,618 | 113.5ms | 204.8ms |

吞吐 ≈ 并发 ÷ (上游延迟 + 网关开销) = 200 ÷ 0.113s ≈ 1760，实测 1618——
**网关在慢上游下没有额外的排队病态**，它只是老老实实等上游。

---

## 4. 行为验证（功能性）

用同样的台架验证过三条核心路径（数字来自 5s 冷却档）：

| 验证 | 方法 | 结果 |
|---|---|---|
| 上游 429 → 全局冷却 | 路由组 `[会 429 的上游, 健康上游]`，观察首个请求与冷却期 | 冷却注册后 40 个连续请求全部跳过故障上游、0 错误、均值 1.45ms；直连冷却中的上游得到快速 429；`local_router_cooldowns_active` 1→0（到期自动恢复） |
| 本地 RPM 限流 | provider `rpm=10`，c=5 打 6s | 窗口满后 29,584 次快速 429，**零上游接触**，`cooldowns_active` 保持 0（本地限流绝不触发冷却）；`GET /api/rate-limits` 可见窗口计数 |
| 配置热重载 | 外部改 `config.json` 的 mtime | 下一个请求生效（压测中途靠它切换 `record_usage` 与 rpm，全程无重启） |

---

## 5. 瓶颈：账本落库

默认配置下，每个请求在 `src/store.rs` 里串行穿过**一把全局互斥锁**
（`Mutex<Option<Connection>>`），写 1 条 `usage_records` + 2 条
`trace_spans`（request span + upstream span）。WAL + `synchronous=NORMAL`
已经打开，锁才是约束：

- 默认配置 CPU 只有 ~150%（15 核机器），吞吐却封顶——CPU 不缺，缺的是
  过锁的带宽；
- 关掉 `record_usage` 后同一代码路径立刻跑到 13–19.5k rps、CPU 235–314%，
  证明其余路径（axum、路由、协议翻译、HTTP 客户端）余量充足；
- 落库规模：本次压测累计 ~100 万请求 → `usage.db` 1.2GB（≈1.2KB/请求），
  span 数恰好是请求数的 2 倍。

**建议**：个人/小团队规模（agent 场景，rps 个位数到几十）完全无感；若要把
网关当共享基础设施打到千级 rps，第一步是**账本批量写**（独立 writer 任务 +
攒批提交），不要简单关掉 `record_usage`——那会同时失去用量账本和 trace 链路。

---

## 6. 复现步骤

```bash
# 0) 构建并准备隔离目录（绝不碰 ~/.config/local-router）
cargo build --release
mkdir -p /tmp/lr-bench && cd /tmp/lr-bench

# 1) 写一份基准配置（providers 指向下面两个桩端口；gateway_port 18100；
#    record_usage 默认 true）。字段说明见 docs/CONFIG.md。

# 2) 起上游桩（两个端口；--delay-ms 可选）
node <repo>/scripts/mock_vendor.mjs --port 18101 &
node <repo>/scripts/mock_vendor.mjs --port 18102 &

# 3) 起隔离网关实例
LOCAL_ROUTER_HOME=/tmp/lr-bench \
  <repo>/target/release/local-router daemon start --port 18100

# 4) 先测上游直连基线，再测网关阶梯
node <repo>/scripts/benchmark.mjs --url http://127.0.0.1:18101 \
  --model mock-small --concurrency 200 --seconds 15 --processes 4
node <repo>/scripts/benchmark.mjs --url http://127.0.0.1:18100 \
  --key bench-caller --model mock/mock-small \
  --concurrency 1000 --seconds 20 --processes 4

# 5) 对照组：热重载关账本，重复第 4 步
python3 - <<'EOF'
import json; p='/tmp/lr-bench/config.json'
c=json.load(open(p)); c['settings']['record_usage']=False
c['revision']=c.get('revision',1)+1; json.dump(c,open(p,'w'),indent=2)
EOF

# 6) 流式：加 --stream；慢上游：桩换 --delay-ms 100

# 7) 收尾
LOCAL_ROUTER_HOME=/tmp/lr-bench <repo>/target/release/local-router daemon stop
```

请求要带 `Authorization: Bearer <default_caller_key>`（网关要求显式 key，
见 §4 的鉴权说明）。

---

## 7. 已知台架限制（读数时注意）

1. **同机挤占**：网关、桩、负载发生器共享 15 核。默认配置下网关 CPU 只有
   ~150%，瓶颈是锁不是 CPU，所以结论不受影响；关账本后 CPU 235–314%，
   客户端/桩开始分走算力，绝对值偏保守。
2. **c≥2000 时负载发生器先饱和**（4 个 Node worker 各扛 500 连接）：
   c=2000 的吞吐下滑（2.6k→1.3k）主要是客户端侧，此时网关 CPU 反而降到 72%。
   要压更高的并发需要更多 worker 进程。
3. **上游桩万级 rps 下偶发拒绝**：关账本的 c≥1000 档出现的少量 502 来自
   Node 桩（错误体 `error sending request for url`）；直连桩压测同样能看到
   个位数 ETIMEDOUT。真实厂商端点不会有这个台架噪声。
4. **macOS 临时端口**：16k 个临时端口 + 30s TIME_WAIT。连接 churn 大时
   客户端会报 `EADDRNOTAVAIL`——这正是 `benchmark.mjs` 默认多进程 +
   keep-alive 的原因；单进程打大并发会把端口烧穿。
5. **回环无 TLS**：所有数字不含 TLS 握手/加解密。公网部署在网关前面加
   反代（见 USAGE.md §8），TLS 代价算在反代上。

---

## 8. 历次记录

| 日期 | 版本 | 变化 |
|---|---|---|
| 2026-11 | 0.1.1 | 首次基线：§3–§5 全部数字；台架升级为多进程客户端 + keep-alive 桩 |
