# local-router 使用说明

面向「第一次用」和「日常运维」两种场景：照着第 1–3 节走完，就能把本地
AI 网关跑起来并让编码 agent 用上；第 4 节之后是按需查阅的参考。

- 想了解项目是什么、为什么这么设计 → [README.md](README.md)
- 想贡献代码 → [CONTRIBUTING.md](CONTRIBUTING.md)
- 想报安全问题 → [SECURITY.md](SECURITY.md)

---

## 1. 五分钟跑起来

### 1.1 装

两种方式，任选其一。

**方式 A：从源码构建（推荐，能拿到最新功能）**

```sh
git clone https://github.com/huifer/local-router.git
cd local-router
npm install --prefix frontend      # 控制台依赖（Node 24+）
cargo build --release               # Rust 1.80+；会自动构建前端
./target/release/local-router --version
```

**方式 B：Docker**

```sh
git clone https://github.com/huifer/local-router.git
cd local-router
docker compose up -d                # 网关与控制台共用 3435
# 或自建镜像：
# docker build -t local-router . && docker run -p 3435:3435 -v lr-data:/data local-router
```

### 1.2 启动

```sh
local-router                # 前台：网关 + 控制台，同一个端口 :3435
```

看到这样就成了：

```
local-router 0.1.1
  gateway  http://127.0.0.1:3435
  console  http://127.0.0.1:3435
  press ctrl-c to stop
```

**网关和控制台是同一个服务、同一个端口**——不用记两个地址：

- 控制台（网页）：<http://127.0.0.1:3435>
- 网关（API）：<http://127.0.0.1:3435>

> 端口占用？3435 被别的 local-router 占着就换一个：
> `local-router serve --port 13435`。
> 只想要 API、不要控制台页面：`local-router serve --no-console`。

### 1.3 配一个上游

三种上游，按你的实际情况选：

```sh
# ① 有 API key 的厂商（最常见）
local-router provider add deepseek --key sk-你的key
local-router provider add openai --key sk-你的key
local-router provider add anthropic --key sk-你的key
local-router provider add gemini --key 你的key

# ② 本机装了 WorkBuddy：免 key，直接用它的登录态
local-router provider add workbuddy

# ③ 有 QwenWork（千问办公）个人账号：免 key，浏览器登录一次
local-router qwen login
```

厂商 id 记不住？`local-router provider list` 看已配的，
`local-router models` 看网关对外暴露的模型。

### 1.4 让 agent 用上

```sh
# 改写 agent 自己的配置文件（推荐）
local-router claude   deepseek/deepseek-chat   # 写 ~/.claude/settings.json
local-router codex    deepseek/deepseek-chat   # 写 ~/.codex/config.toml
local-router gemini   deepseek/deepseek-chat   # 写 ~/.gemini/.env
local-router opencode deepseek/deepseek-chat   # 写 opencode.json
```

或者不碰 agent 配置，直接给环境变量（任何吃 base URL 的工具都行）：

```sh
export OPENAI_BASE_URL=http://127.0.0.1:3435/v1
export OPENAI_API_KEY=local-router            # 默认调用 key
export ANTHROPIC_BASE_URL=http://127.0.0.1:3435
export ANTHROPIC_API_KEY=local-router
export GOOGLE_GEMINI_BASE_URL=http://127.0.0.1:3435
export GEMINI_API_KEY=local-router
```

**改完要重开 agent 会话**才生效。

### 1.5 验证

```sh
local-router doctor        # 一眼看出哪里没配好
curl -H "Authorization: Bearer local-router" http://127.0.0.1:3435/v1/models
```

---

## 2. 日常怎么用

### 2.1 控制台（网页）

控制台支持 **English / 简体中文** 两种语言：侧边栏底部的 `EN` / `中` 按钮切换，
选择会存进网关设置（`settings.locale`），换浏览器、换机器也保持一致；
首次访问按系统语言自动选择。供应商卡片**可点击**，右侧滑出抽屉查看该上游的
全部信息（端点、凭据来源、暴露模型、请求头规则、流式/额度开关等）。

<http://127.0.0.1:3435>，左边六个页签：

| 页签 | 干什么 |
| --- | --- |
| **Overview** | 网关状态、快速入口、给 agent 用的环境变量 |
| **Providers** | 加/删上游、改 key、刷新模型列表、测连通性 |
| **Models** | 所有对外暴露的模型（`provider/model`）一览 |
| **Groups** | 路由组：成员、策略、粘性、列出与否 |
| **Usage** | 用量：Overview / Sessions / Projects / Context / Quota / Bills |
| **Settings** | 端口、绑定地址、调用 key、遮蔽开关、caller key |

**Usage 页的子页签**：

- **Sessions**：按会话出账（标题取首个用户消息第一行）
- **Projects**：按工作目录分组，可改名/配色/设预算/合并
- **Context**：上下文窗口拆解（系统提示 / 工具定义 / 附件 / 对话各占多少 token）
- **Quota**：WorkBuddy、QwenWork 的额度窗口与重置时间

### 2.2 交互模式（小白友好）

```sh
local-router console
```

进入后每行一个词，敲 `help` 看全部命令，`exit` 退出（Ctrl-D 也行）：

```
local-router> add 七牛云 --url https://api.qnaigc.com/v1 --key sk-xxx
local-router> list                    列出已配的上游
local-router> test 七牛云              测连通性
local-router> fetch 七牛云             拉厂商的模型列表
local-router> expose 七牛云 qwen-max,qwen-plus
local-router> use claude 七牛云/qwen-max    把 Claude Code 指过去
local-router> workbuddy login          浏览器登录 WorkBuddy
local-router> workbuddy accounts        列出（可登录多个）WorkBuddy 账号
local-router> workbuddy use personal     切换到另一个账号
local-router> clear                     清屏（clean / cls 同义）
```

面板里**可以搜索**厂商（按名称 / 协议 / 地址过滤）；带 **OAUTH** 徽标的厂商
（如 OpenRouter）不用贴 key——选它再点「Sign in with …」，浏览器里完成登录，
凭证自动存好。

加自定义厂商时可以指定协议：

```sh
add 我的中转 --url https://api.example.com/v1 --key sk-xxx --protocol anthropic
```

协议取值：`openai_chat`（默认，OpenAI 兼容）/ `anthropic` / `gemini` /
`openai_responses`。厂商名可以写中文。

### 2.3 命令行速查

```sh
# 看配置
local-router ls                      # 有哪些 provider / group / 模型
local-router models                  # 网关对外暴露的模型目录
local-router doctor                  # 自检
local-router prices                  # 当前价目表

# 上游
local-router provider add <preset> --key sk-xxx
local-router provider list
local-router provider expose deepseek deepseek-chat deepseek-reasoner   # 改暴露的模型
local-router provider models deepseek          # 拉厂商自己的模型列表
local-router provider test deepseek             # 发一个真实请求测连通性
local-router provider key deepseek sk-第二个key  # 加一把 key（多 key 轮试）
local-router provider rm deepseek

# 请求头重写（某些中转/上游挑剔头的时候用）
local-router provider headers deepseek                            # 查看全部规则
local-router provider set-headers deepseek "x-profile=gw-{version}"  # 静态头（{version}/{platform}/{arch} 会展开）
local-router provider forward-headers deepseek "x-opencode-session"  # 把 agent 发来的同名头原样转给上游
local-router provider remove-headers deepseek "anthropic-version"    # 删掉上游不认识的头
local-router provider header-order deepseek "user-agent,authorization"  # 发送顺序（多数厂商不在乎）
local-router provider user-agent deepseek "my-relay/{version} ({platform} {arch})"  # UA 模板

# 路由组
local-router group add backup --models deepseek/deepseek-chat,glm/glm-4.6 --routing order
local-router group list
local-router group rm backup

# 用量
local-router usage                  # 近 7 天表格
local-router usage --period today
local-router usage --csv            # 机器可读

# 登录态上游
local-router workbuddy status|credits|models|checkin
local-router qwen status|credits|models

# 进程
local-router daemon start|stop|status|logs
local-router dashboard               # 终端仪表盘（q 退出）
```

### 2.4 路由组：从已有模型里选

控制台 **Groups → Add routing group** 的成员不再手敲——面板直接列出
**你所有供应商已暴露的模型**（按供应商分组、可搜索、点选即加入），所选的
**顺序就是 failover 顺序**。命令行同样可以：

```sh
local-router group add backup --models 七牛云/qwen3-max,deepseek/deepseek-chat --routing order
local-router group weights balanced "a/m1=3,b/m2=1"   # weight 模式的权重
```

### 2.5 调用 API

四种入口协议任意选，与上游协议自由组合（流式 / 非流式都支持）。

```sh
KEY=local-router
BASE=http://127.0.0.1:3435

# OpenAI Chat
curl $BASE/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H "content-type: application/json" \
  -d '{"model":"deepseek/deepseek-chat","messages":[{"role":"user","content":"你好"}]}'

# 流式（加 "stream": true，响应是 SSE）
curl -N $BASE/v1/chat/completions -H "Authorization: Bearer $KEY" \
  -H "content-type: application/json" \
  -d '{"model":"deepseek/deepseek-chat","messages":[{"role":"user","content":"写首诗"}],"stream":true}'

# OpenAI Responses
curl $BASE/v1/responses -H "Authorization: Bearer $KEY" \
  -H "content-type: application/json" \
  -d '{"model":"deepseek/deepseek-chat","input":"你好"}'

# Anthropic Messages
curl $BASE/v1/messages -H "x-api-key: $KEY" \
  -H "anthropic-version: 2023-06-01" -H "content-type: application/json" \
  -d '{"model":"deepseek/deepseek-chat","max_tokens":1024,"messages":[{"role":"user","content":"你好"}]}'

# Gemini
curl "$BASE/v1beta/models/deepseek/deepseek-chat:generateContent" \
  -H "x-goog-api-key: $KEY" -H "content-type: application/json" \
  -d '{"contents":[{"role":"user","parts":[{"text":"你好"}]}]}'

# token 估算
curl $BASE/v1/messages/count_tokens -H "x-api-key: $KEY" \
  -H "content-type: application/json" \
  -d '{"model":"deepseek/deepseek-chat","messages":[{"role":"user","content":"数几个 token"}]}'
```

鉴权头四种都收：`Authorization: Bearer <key>`、`x-api-key`、`x-goog-api-key`、
`api-key`。key 取 `LOCAL_ROUTER_DEFAULT_KEY` → 设置里的默认 key → 任一
caller key；一把都没有时**拒绝一切请求**。

### 2.6 开机自启（以后不用再手动启动）

```sh
local-router daemon install     # 装上开机自启（macOS 用 launchd，Linux 用 systemd）
local-router daemon uninstall   # 卸掉
```

装上之后：登录系统网关自动起来；进程崩了也会被自动拉起（实测杀掉进程 8 秒内
自动恢复，页面照常可用）。想看它跑在哪：`local-router daemon status`。

### 2.7 WorkBuddy：登录与多账号

```sh
local-router workbuddy login                # 浏览器登录（不给名字就取第一个空闲账号槽）
local-router workbuddy login --account work # 登录到指定账号槽
local-router workbuddy login --no-open      # 只打印链接，不自动开浏览器
local-router workbuddy accounts              # 列出所有账号与当前使用的
local-router workbuddy use work             # 切换到某个账号
local-router workbuddy status|credits|models|checkin
```

**登录链接总是打在终端里**——复制到任何浏览器打开即可（远程 shell、
无桌面环境、SSH 登录都适用）；`--no-open` 表示不自动唤起本机浏览器。
千问办公同理：

```sh
local-router qwen login                  # 浏览器登录（不给名字就取第一个空闲账号槽）
local-router qwen login --account work   # 登录到指定账号槽
local-router qwen login --no-open        # 只打印链接，不自动开浏览器
local-router qwen accounts               # 列出所有千问办公账号与当前使用的
local-router qwen use <名字>              # 切换到某个账号
```

多个账号可以并存（WorkBuddy 存 `~/.config/local-router/workbuddy-session.json`，
千问办公存 `qwen-session.json`），网关让请求在它们之间**轮流走**，分摊额度；
`use` 指定优先用哪个。

> **账号槽**：不给账号名时，网关取第一个空闲槽（`default` → `workbuddy-2`
> → `workbuddy-3`），所以登录第二次不会覆盖第一个账号；删账号用控制台
> provider 抽屉里的 ✕，或 `workbuddy accounts` / `qwen accounts` 看清有哪些。
>
> **登录页上换人**：厂商的登录页（WorkBuddy / 千问办公）浏览器已经登录时会
> 直接用那个身份；要换成另一个人，在登录页点「切换账号」即可，**不用先
> 退出浏览器**。

> 本机装了 WorkBuddy 桌面版时，它自己的登录文件也能直接用——但新版本把
> token 加密存了，local-router 读不出来，所以请用上面的 `workbuddy login`
> （浏览器登录一次即可，之后自动续期）。

### 2.7.1 Trae CN

Trae CN 是 Cloud-IDE 协议（`Cloud-IDE-JWT` 鉴权），不是普通 OpenAI 兼容
上游，local-router 为它走专用通道。当前**只支持导入的会话**（从 Magpie
迁移过来的 Cloud-IDE 登录态，存 `trae-session.json`），浏览器登录尚未
接通——控制台点登录会明确告诉你这一点。导入后的账号同样可以列出、切换、
删除，网关按账号轮转分摊额度。

### 2.8 观察网关内部

```sh
KEY=local-router
BASE=http://127.0.0.1:3435

curl -H "Authorization: Bearer $KEY" $BASE/v1/local-router/status        # 运行状态
curl -H "Authorization: Bearer $KEY" $BASE/v1/local-router/quotas        # 各上游额度窗口
curl -H "Authorization: Bearer $KEY" "$BASE/v1/local-router/usage?period=7d"   # 用量汇总
curl -H "Authorization: Bearer $KEY" "$BASE/v1/local-router/usage/requests?limit=50"  # 逐请求明细
curl -H "Authorization: Bearer $KEY" -H "X-Session-ID: my-session" $BASE/v1/local-router/route  # 这次路由去了哪
```

### 2.9 图片与视频

```sh
# 图片生成
curl $BASE/v1/images/generations -H "Authorization: Bearer $KEY" \
  -H "content-type: application/json" \
  -d '{"model":"workbuddy/hunyuan-image-v3.0-art","prompt":"一只戴帽子的猫"}'

# 视频是异步任务：创建 → 查状态 → 取文件
curl $BASE/v1/videos -H "Authorization: Bearer $KEY" \
  -H "content-type: application/json" \
  -d '{"model":"workbuddy/kling-video-v3","prompt":"一只猫在跑"}'
```

能做图/视频的模型要先声明：

```sh
local-router provider media workbuddy hunyuan-image-v3.0-art kling-video-v3
```

---

## 3. 常用场景怎么做

### 场景 A：一个模型挂了自动换另一个

```sh
local-router group add backup \
  --models deepseek/deepseek-chat,glm/glm-4.6 \
  --routing order            # 主备：第一个不行就换第二个
```

然后让 agent 用 `group/backup` 这个模型名：

```sh
local-router claude group/backup
```

### 场景 B：多个厂商轮着用（分摊额度）

```sh
local-router group add mixed \
  --models deepseek/deepseek-chat,glm/glm-4.6,kimi/kimi-k2 \
  --routing rotate           # 每个请求换一个
```

### 场景 C：按用量挑最闲的

```sh
local-router group add balancer \
  --models deepseek/deepseek-chat,glm/glm-4.6 \
  --routing usage            # 最少用的先上
```

### 场景 D：让一段对话固定在同一个厂商（保住 prompt cache）

```sh
local-router group add sticky \
  --models deepseek/deepseek-chat,glm/glm-4.6 \
  --routing order --stays session
```

`stays` 五档：

| 值 | 含义 |
| --- | --- |
| `auto` | 默认，尽量留在同一处（prompt cache 还值钱时） |
| `session` | 整个会话固定在一个目标 |
| `turn` | 同一轮对话内固定；新对话重新选 |
| `off` | 不粘，每次按策略选 |
| `strict` | 只认会话的目标；目标消失就报错，**不**回退到别的成员 |

### 场景 E：按用量加权分配

```sh
local-router group add weighted --models a/m1,b/m2 --routing weight
# 再在 config.json 里给该组配 weights（a=3 表示 a 承担三倍）
```

### 场景 F：多台机器共用一台网关

```sh
local-router serve --lan                       # 绑 0.0.0.0
# 另一台机器：
export OPENAI_BASE_URL=http://网关IP:3435/v1
export OPENAI_API_KEY=你的caller-key
```

局域网模式下控制台 `/api` 需要 key（回环才免鉴权）。

### 场景 G：开机自启（后台常驻）

```sh
local-router daemon start        # 后台跑（网关 + 控制台同一进程）
local-router daemon status       # 看状态与日志路径
local-router daemon logs         # tail 日志
local-router daemon stop
```

日志在 `~/.config/local-router/daemon/daemon.log`。

### 场景 H：给某个项目单独计费/限额

agent 启动时带上工作目录，网关就会自动建项目：

```sh
export X_PROJECT=/path/to/workdir     # 或 x-cwd / x-project-dir
```

然后在控制台 Projects 页改名、配色、设预算。

### 场景 I：不想让提示词里的密钥发出去

```sh
# config.json 的 settings 里：
"masking": true
```

开启后，提示词与工具参数里的 `sk-…`、`ghp_…`、长十六进制串、以及名字
含 key/token/secret/password 的参数会被替换成 `***MASKED***`。

### 场景 J：价格变了，不用等发版

```sh
cat > ~/.config/local-router/prices.json << 'EOF'
{"hy3": {"input": 0.5, "output": 2.0, "credits_per_million": 200.0}}
EOF
```

下一个请求就按新价算（价目是估算，`credits_per_million` 是订阅制厂商
的额度折算率）。

### 场景 K：新模型上线，不想改配置

给 provider 配 `auto_include`（glob，`*` 通配）：

```json
{"id": "deepseek", "auto_include": ["deepseek-*"]}
```

之后在控制台点「刷新模型」，新列出且匹配的模型会自动并入 provider 并
落盘。

---

## 4. 配置与文件

### 4.1 文件位置

`~/.config/local-router/`（`LOCAL_ROUTER_HOME` 可覆盖，Docker 里是 `/data`）：

| 文件 | 内容 |
| --- | --- |
| `config.json` | providers、groups、settings、caller-keys（0600） |
| `usage.jsonl` | 每请求一行：模型、provider、token、时延、状态、成本 |
| `usage.db` | 同一份记录的 SQLite 视图（`LOCAL_ROUTER_DISABLE_SQLITE=1` 可关） |
| `sessions.json` | 会话标题簿 |
| `projects.json` | 项目注册表（名称 / 颜色 / 预算） |
| `prices.json` | 可选：价目表覆盖 |
| `workbuddy-session.json` | WorkBuddy 浏览器登录的 token（0600） |
| `qwen-session.json` | QwenWork 设备流登录的 token（0600） |
| `daemon/daemon.log` | 守护进程日志 |

CLI 或控制台改了 `config.json`，网关在**下一个请求**自动重载（mtime 探测），
不用重启。

> 完整字段表与优先级见 [docs/CONFIG.md](docs/CONFIG.md)。

### 4.2 settings 字段

| 字段 | 作用 |
| --- | --- |
| `gateway_port` | 端口（网关与控制台共用） |
| `console` | 是否提供控制台页面（默认 true） |
| `gateway_addr` / `web_addr` | 绑定地址 |
| `lan` | 是否绑 0.0.0.0（局域网可用） |
| `default_caller_key` | 调用方默认用的 key |
| `record_usage` | 是否记录用量 |
| `masking` | 是否在发往上游前遮蔽提示词里的密钥 |
| `workbuddy_auto_checkin` | 是否自动领 WorkBuddy 每日签到（默认开） |
| `send_agent_user_agent` | 是否把 agent 的 User-Agent 转发给上游 |
| `cors_origins` | 允许哪些浏览器源跨域调用网关。默认空（不允许） |

### 4.3 环境变量

| 变量 | 作用 |
| --- | --- |
| `LOCAL_ROUTER_HOME` | 配置目录（Docker 里是 `/data`） |
| `LOCAL_ROUTER_GATEWAY_PORT` | 端口（CLI 参数可再覆盖） |
| `LOCAL_ROUTER_GATEWAY_ADDR` / `LOCAL_ROUTER_WEB_ADDR` | 绑定地址 |
| `LOCAL_ROUTER_DEFAULT_KEY` | 运行时调用密钥（**不落盘**，优先于文件里的 key） |
| `LOCAL_ROUTER_LAN` | `1` 等价于 `--lan` |
| `LOCAL_ROUTER_LOG` | 日志过滤（如 `debug`） |
| `LOCAL_ROUTER_USAGE_MAX_BYTES` | 用量日志翻页上限（默认 8 MB） |
| `LOCAL_ROUTER_DISABLE_SQLITE` | `1` 关掉 SQLite，只用 JSONL |
| `LOCAL_ROUTER_WORKBUDDY_SESSION` / `_ENDPOINT` / `WORKBUDDY_HOME` | WorkBuddy 登录态与端点 |
| `LOCAL_ROUTER_QWEN_SESSION` / `LOCAL_ROUTER_QWEN_ENDPOINT` | QwenWork 会话与端点 |

优先级：**CLI 参数 > 环境变量 > config.json > 默认值**。

---

## 5. 排障

### 常见问题

**Q: agent 说 401 / "this request has no key"**
A: 网关一把 key 都没有时会拒绝一切请求。设一个：
`export LOCAL_ROUTER_DEFAULT_KEY=local-router`，或控制台 Settings 里设
「Default API key」。

**Q: 控制台 401（但我就在本机）**
A: 网关绑到了非回环地址（`--lan`、容器里），此时 `/api` 需要 key。浏览器
会弹窗要 key，填网关那把即可。

**Q: agent 改了配置没生效**
A: 改完要**重开 agent 会话**；也确认 `local-router models` 里真的有这个
模型（provider 的 `models` 是白名单）。

**Q: 控制台看不到用量**
A: 检查 `settings.record_usage` 是否为 true；数据在 `usage.jsonl` /
`usage.db`，可 `local-router usage` 看。

**Q: 端口被占**
A: `local-router serve --port 13445`。注意 3435 常被另一个 local-router
占着。

**Q: 供应商报 401**
A: `local-router provider test <id>` 单测一把 key；再 `local-router doctor`
看整体。

**Q: 响应是流式但我想要整段**
A: 调用方不传 `stream` 即可；上游只支持流式时，网关会收流再拼成整段。

### 诊断命令

```sh
local-router doctor              # 自检：配置、登录态、连通性
local-router ls                  # 当前配置概览
local-router models              # 对外模型目录
local-router usage --period today
local-router daemon logs         # 守护进程日志
```

调试日志：`LOCAL_ROUTER_LOG=debug local-router serve`。

---

## 6. 安全须知

- 网关端点**永远**要求 key；没有 key 时拒绝一切请求。
- 控制台在**回环地址**免鉴权（本机信任模型）；绑到非回环地址时 `/api`
  需要同一把 key。
- 所有**写**请求校验 `Origin`，跨站写一律 403（防 CSRF）。
- `config.json` 以 0600 落盘；`LOCAL_ROUTER_DEFAULT_KEY` 只在进程内存。
- 控制台展示的供应商密钥永远是掩码。
- 有意不做：TLS / 反向代理、多用户体系、审计级日志。公网暴露请自备反代
  与鉴权网关。

更多见 [SECURITY.md](SECURITY.md)。

---

## 7. 卸载

```sh
local-router daemon stop                    # 先停守护进程
rm -rf ~/.config/local-router               # 删配置与用量
# agent 的配置是 CLI 改写的，需要自己改回去（claude/codex/gemini/opencode）
```
