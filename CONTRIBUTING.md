# 贡献指南

谢谢愿意贡献！这份项目保持小而清晰：本地优先、单二进制、只做网页模式 +
CLI。提交改动前请先读 README 的「范围与取舍」，明确不做的方向就不必提了
（桌面客户端、插件生态、QwenWork 企业版等）。

## 开发环境

- Rust 1.80+（`rustup` 稳定通道即可）
- Node.js 24+（仅构建控制台用）

```sh
git clone <仓库地址> && cd local-router
npm ci --prefix frontend          # 控制台依赖（锁定文件已提交）
cargo test                        # 228 个测试（含 src/ 内的原生单元测试）
```

测试分两层：`src/` 内的 `#[cfg(test)]` 原生单元测试（纯逻辑，快）与
`tests/` 内的集成测试（真起 HTTP）。改哪层就在哪层补用例；控制台的
字段变更必须同步 `frontend/src/types.ts` 与 `tests/web.rs` 的 API 形状
用例，两边一起变。

`cargo build` 会通过 `build.rs` 自动调用 npm 构建控制台；没有 npm 的环境
可设 `LOCAL_ROUTER_SKIP_FRONTEND=1` 用桩页面代替。

## 提交前检查

四件事全过再提 PR，CI 会跑同一套：

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
npm run build --prefix frontend
```

## 怎么改

- **协议层**（`src/protocol/`）：改解析 / 格式化请同步补
  `tests/translate.rs` 的用例，四种协议的往返都要照顾到；纯函数（形状
  判定、名字映射）直接在同文件加 `#[cfg(test)]` 单元测试。
- **路由**（`src/gateway/route.rs`）：新策略加进 `Routing` 枚举并在
  `tests/routing.rs` 补确定性测试（建议用 order + 粘性成员来验证顺序）。
- **网关 / 控制台**（`src/gateway/` `src/web/`）：涉及安全的行为（鉴权、
  Origin 校验、密钥掩码）必须在 `tests/web.rs` 或 `tests/gateway.rs` 有
  对应用例；给控制台加新 API 字段时，同步补形状用例和前端类型。
- **控制台前端**（`frontend/`）：TypeScript 严格模式，别关 `tsc`。

## 提交信息

用祈使句、一行主题说清「做了什么」，正文补充「为什么」。参考
Conventional Commits 风格（`feat:` / `fix:` / `docs:` / `refactor:` /
`test:` / `chore:`），但不强制 scope。

## 报告问题

- 功能缺陷或崩溃：用 Issue 模板，附 `local-router doctor` 输出和最小
  复现（脱敏后）。
- 安全漏洞：**不要**开公开 Issue，走 [SECURITY.md](SECURITY.md) 的私下
  报告渠道。

## 行为准则

参与本项目即表示同意 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。

## 许可

提交的代码以 [MIT](LICENSE) 授权。
