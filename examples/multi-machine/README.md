# 多机级联：一台主网关 + 若干上游节点

> **没有「节点机制」。** local-router 是有意做成单机单进程网关的（见
> [USAGE.md](../../USAGE.md) 的「有意不做：TLS / 反代 / 多用户」）。
> 没有节点注册、没有服务发现、没有集群。
>
> 但你要的拓扑**不需要**节点机制：**把另一台机器上的 local-router 当成
> 一个普通上游加进来**就行——级联。

```
各设备上的 agent
      │  ANTHROPIC_BASE_URL / OPENAI_BASE_URL
      ▼
┌─────────────────────┐
│ 香港：主网关         │   local-router（3435）
│  所有 agent 连这里   │   ← 统一路由 / 故障转移 / 冷却 / 用量 / 链路
└──────────┬──────────┘
           │  当成普通上游（provider: claude-us）
           ▼
┌─────────────────────┐
│ 美国：上游节点       │   local-router（3445）→ api.anthropic.com
│  从美国 IP 出网     │   ← 自己的 key、自己的冷却、自己的链路
└─────────────────────┘
```

好处：主网关统一记账（`by_provider_key` / `by_caller_key`）、统一故障转移、
统一冷却；trace id 逐层下传，**两边的链路是同一条**，从厂商账单也能反查回
会话。

---

## 0. 先在本机验证一遍（不需要真实 API key）

```sh
./examples/multi-machine/run-demo.sh
```

脚本会起三个进程，把正式配置里的地址改写成回路地址：

```
agent → 香港网关(:随机) → 美国节点(:随机) → mock 上游(:随机)
```

真实的部分全部保留：caller key 鉴权、协议互译、路由组、链路追踪。
输出会依次验证：

1. 不带 key → 香港网关 **401**
2. 带 key 请求 `claude-us/claude-sonnet-4-5` → 拿到回复
3. 走路由组 `group/claude` → 拿到回复
4. **两边的 trace id 是同一个**（`trc_…`），span 树分别长这样：

```
香港网关侧                        美国节点侧
└─ request group/claude  ping      └─ request claude-opus-4-5  ping
   └─ attempt 1  claude-us ·         └─ attempt 1  claude ·
      claude-opus-4-5  ok 200          claude-opus-4-5  ok 200
```

5. mock 收到的模型名是**剥掉前缀的裸名**（`claude-sonnet-4-5`），
   鉴权头是**美国节点自己的 key**——两跳的行为都对。

端口是动态挑的空闲端口，不会碰到你正在跑的 local-router。

---

## 1. 真实部署

### 1.1 美国那台（上游节点）

```sh
# 1) 指向 Anthropic（anthropic 是内置预设，协议本来就是 anthropic）
local-router provider add anthropic --key sk-ant-xxx
#    模型名会是 anthropic/<model>；想用别的名字就改这个 provider 的 id
#    （本目录的示例配置里它叫 claude）

# 2) 暴露给主网关：给主网关发一把 caller key
#    config.json 里加：
#    "caller_keys": [{ "id": "k1", "name": "hk", "key": "us-caller-key", "enabled": true }]
#    并且 settings.gateway_addr 改成 "0.0.0.0"

# 3) 对外要有 TLS —— 本项目不内置 TLS，两条路：
#    a) caddy/nginx 反代（最常见）：
#       us.example.com { reverse_proxy 127.0.0.1:3445 }
#    b) Cloudflare Tunnel / Tailscale / frp：不用开公网端口
```

> **caller key 是必须的**：一把 key 都没有时网关拒绝一切请求。`config.json`
> 权限保持 `0600`。

### 1.2 香港那台（主网关）

```sh
local-router provider add claude-us \
  --url https://us.example.com \
  --key us-caller-key \
  --protocol anthropic

# 暴露的模型名（和模型目录保持一致）
local-router provider expose claude-us claude-opus-4-5,claude-sonnet-4-5,claude-haiku-4-5

# 一个对外统一的名字
local-router group add claude \
  --models claude-us/claude-opus-4-5,claude-us/claude-sonnet-4-5,claude-us/claude-haiku-4-5 \
  --routing order
```

本目录里有两份可直接用的配置：[us-node/config.json](us-node/config.json)、
[hk-gateway/config.json](hk-gateway/config.json)。

### 1.3 各设备上的 agent

```sh
export ANTHROPIC_BASE_URL=http://hk-server:3435
export ANTHROPIC_API_KEY=<香港主网关的 caller key>
```

模型名写 `group/claude`（走路由组）或 `claude-us/claude-sonnet-4-5`（点名某条路）。

---

## 2. 模型名怎么对得上

主网关把请求转给上游节点时，**模型名会去掉主网关自己的前缀**（`claude-us/`），
发出去的是 `claude-sonnet-4-5`。上游节点收到的正是一个**裸模型名**。

网关认三种写法：

| 写法 | 例子 | 说明 |
| --- | --- | --- |
| `provider/model` | `claude-us/claude-sonnet-4-5` | 点名某家的某个模型 |
| `group/id` | `group/claude` | 走路由组 |
| **裸模型名** | `claude-sonnet-4-5` | **恰好一个**上游提供它时直接用它 |

所以裸名不会报错——这是级联能直接跑起来的关键（也顺带让 Claude Code 这类
发裸模型名的客户端不用改配置）。两台机器的模型名保持一致即可。

> 上游节点若有两个上游都提供同一个裸名，网关会拒绝并把两家都列出来，
> 这时该用 `provider/model` 写清楚。

---

## 3. 另一种更简单的形态：美国那台不跑 local-router

如果美国那台**只需要一个美国出口 IP**（并没有自己的路由组、冷却、用量需求），
那它跑一个裸的反代就够了：

```nginx
# 美国机器：nginx
location / {
    proxy_pass https://api.anthropic.com;
    proxy_ssl_server_name on;
    proxy_set_header Host api.anthropic.com;
}
```

```sh
# 香港主网关：把它当一个普通上游
local-router provider add claude \
  --url https://us.example.com \
  --key sk-ant-xxx \
  --protocol anthropic
```

这样**模型名不用绕前缀**（主网关发 `claude-sonnet-4-5`，反代原样转给厂商），
链路追踪也只剩一跳。什么时候值得多跑一个 local-router：

| | 只跑反代 | 跑 local-router |
| --- | --- | --- |
| 配置量 | 最小 | 两份配置 + TLS |
| 模型名 | 裸名直通 | 裸名（网关能解析） |
| 故障转移 | 没有 | 主网关统一做 |
| 429 冷却 | 没有 | 两台各自冷却 |
| 用量统计 | 只在主网关 | 主网关能看到经过的量，节点自己也能 |
| 链路追踪 | 一跳 | 两跳，trace id 串起来 |

**建议**：先把「只跑反代」跑通（分钟级），等真的需要多上游/故障转移时，
再把美国那台升级成 local-router——主网关那侧的配置几乎不用动。

---

## 4. 加更多节点

每加一台，在主网关上多一条 provider：

```sh
local-router provider add claude-jp --url https://jp.example.com --key <jp 的 caller key> --protocol anthropic
local-router provider expose claude-jp claude-sonnet-4-5
```

然后把它加进同一个路由组：

```sh
local-router group add claude \
  --models claude-us/claude-sonnet-4-5,claude-jp/claude-sonnet-4-5 \
  --routing order
```

没有自动发现——**每加一台都要手动 `provider add` 一次**。

路由组怎么选是另一回事：`order` 固定顺序、`rotate` 轮流、`usage`/`smart`
挑最闲的、`weight` 按比例。

---

## 5. 排障

| 现象 | 原因 / 做法 |
| --- | --- |
| `401` / `that key is not accepted` | 那一跳没有 caller key，或两边 key 不一致 |
| `is not a model id; use provider/model or group/id` | 这一跳的上游节点不认裸名——检查上游节点有没有这个模型；或在上游节点侧改成 `provider/model` |
| `is served by both a and b` | 两个上游提供同一个裸名，写清楚用 `provider/model` |
| `there is no provider called X` | provider id 写错，或它被 `group_only` 挡着 |
| 连不上上游节点 | TLS / 防火墙 / `gateway_addr` 不是 `0.0.0.0` |
| 上游 429 | 冷却是**每台实例独立**的：上游节点冷却它自己，主网关冷却「上游节点」这个 provider |

链路对不上时，先看两边的 trace id 是否相同：

```sh
local-router traces            # 主网关侧
local-router trace trc_xxx     # 一棵树
```
