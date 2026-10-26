<!-- 提交前确认（CI 会跑同一套检查）： -->
<!-- cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test -->

## 这个 PR 做了什么

<!-- 一两句话说清改动与动机；关联 Issue 用 "Closes #123" -->

## 怎么验证的

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-targets --all-features`
- [ ] 涉及协议 / 路由 / 网关 / 控制台守卫的改动补了对应测试
- [ ] `frontend/` 有改动时 `npm run build` 通过

## 自查

- [ ] 不在「范围与取舍」明确不做的方向里
- [ ] 没有引入新的供应商密钥打印 / 日志泄露
- [ ] README / CHANGELOG 需要的话已同步
