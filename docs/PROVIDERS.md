# 登录态上游：WorkBuddy / QwenWork / Trae CN

不想买 API key 时，local-router 可以直接复用订阅账号的**登录态**，把账号
当成一个上游。登录、多账号、额度查看的日常操作见
[USAGE.md § 2.7](../USAGE.md#27-workbuddy登录与多账号)；本页讲机制与边界。

## 通用设计

- **密钥不进 `config.json`**：配置里只有 `"auth": "workbuddy"` /
  `"auth": "qwenwork"` 这样的标记；token 只在会话文件（0600）与进程内存里，
  日志与 `usage.jsonl` 也不出现 token。
- **多账号轮转**：同一 provider 登录多个账号后，网关让请求在账号之间轮流走，
  分摊额度；`workbuddy use <名字>` / `qwen use <名字>` 指定优先账号。
  每个账号的额度窗口独立可见（控制台 Quota 页与
  `GET /v1/local-router/quotas`）；控制台与额度响应里的账号名 / uid 一律打码。
- **自动续期**：token 快到期时用 refresh token 换新，无需人工干预。
- **容器注意**：容器里看不到宿主机的登录态，除非把会话文件挂进去
  （环境变量见文末）。

## WorkBuddy

```sh
local-router provider add workbuddy      # 不需要 key
local-router claude workbuddy/hy3        # 让 Claude Code 走 WorkBuddy
```

- **登录**：`local-router workbuddy login` 打印登录链接（并自动打开浏览器），
  链接五分钟内有效，也可以复制到别的机器的浏览器里打开。登录完成后 token
  存到 local-router 自己的会话文件，provider 自动加上——本机 WorkBuddy
  客户端与其登录文件完全不受影响。（桌面客户端新版把 token 加密存储，
  local-router 读不出来，所以一律走浏览器登录。）
- **自动签到**：网关自己盯签到活动，到点未签就代签（默认开，
  `settings.workbuddy_auto_checkin: false` 关掉），每日 +100 credits；
  手动领用 `local-router workbuddy checkin`。
- **续期**：access token 只在内存；快到期时用 refresh token 换新
  （refresh token 经上游同一接口换发）。
- **协议**：上游只接受流式请求（`POST /v2/chat/completions`）。调用方要
  整段回复时，网关收流后拼好再返回，所以四种入口协议都能用。
- **媒体**：图像 / 视频模型（如 `hunyuan-image-v3.0-art`、
  `kling-video-v3`）要用 `local-router provider media <id> <模型...>` 声明；
  WorkBuddy 把媒体放在 `/v2` 且视频一次生成完（无任务状态接口），
  provider 的 `media_paths` 里写明即可，网关自己兜住状态与下载。
- **验证**：`bash scripts/workbuddy-login-check.sh`——五步检查
  （客户端 / 登录态 / 服务端 / 接入 / 真实推理）。

## QwenWork（千问办公）

```sh
local-router qwen login                  # 设备流登录 → 自动配置 provider
local-router claude qwenwork/qwen3-coder-plus
```

- **登录**：与千问办公桌面客户端同款**设备流**——打印登录链接，浏览器登录
  一次即可，token 存 `qwen-session.json`（0600）。
- **协议专线**：每个请求都要过 COSY 信封（RSA 包 AES 密钥 + MD5 签名 +
  乱序 base64 体编码），通用 OpenAI 管道表达不了，网关里走专用模块；
  调用方看到的仍是标准四种入口协议。
- **续期**：token 到期前五分钟自动用 refresh token 换新；换发结果**不回写
  磁盘**（与客户端行为一致）。
- **范围**：只支持个人公有云（`gateway.qwenwork.cn`）；企业版 VPC / PAT
  有意不做。
- **额度**：钱包分日常 / 月度 / 长期三个池，`local-router qwen credits` 与
  控制台 Quota 页可见。

## Trae CN

Trae CN 是 Cloud-IDE 协议（`Cloud-IDE-JWT` 鉴权），不是普通 OpenAI 兼容
上游，网关为它走专用通道。当前**只支持导入的会话**（从 Magpie 迁移过来的
Cloud-IDE 登录态，存 `trae-session.json`），浏览器登录尚未接通——控制台
点登录会明确提示。导入后的账号同样可列出、切换、删除，按账号轮转分摊。

## 环境变量

| 变量 | 作用 |
| --- | --- |
| `LOCAL_ROUTER_WORKBUDDY_SESSION` | WorkBuddy 登录态文件的完整路径（容器挂载用；找不到时不再猜别的位置） |
| `LOCAL_ROUTER_WORKBUDDY_ENDPOINT` | 换用别的 WorkBuddy 端点（私有化 / 测试） |
| `WORKBUDDY_HOME` | 本机客户端登录态所在目录（默认自动查找） |
| `LOCAL_ROUTER_QWEN_SESSION` | QwenWork 会话文件路径 |
| `LOCAL_ROUTER_QWEN_ENDPOINT` | 换用别的 QwenWork 端点（测试） |

## 相关文档

- 登录 / 多账号 / 签到的日常操作：[USAGE.md § 2.7](../USAGE.md#27-workbuddy登录与多账号)
- 媒体端点与声明：[API.md](API.md) · 额度窗口与账单：[BILLING.md](BILLING.md)
