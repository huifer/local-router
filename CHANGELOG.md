# Changelog

本项目的所有显著变更都会记录在这个文件里。
格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.1.0] - 2026-10-26

首个公开发布版本。

### Added

- 本地 AI 网关（Rust 单二进制）：一个进程提供 OpenAI Chat / OpenAI
  Responses / Anthropic / Gemini 四种入口协议，任意「入口 × 上游」互译
  （流式、工具调用、思考块含 signature、stop 原因、token 用量双向翻译）。
- 路由组：`order` / `rotate` / `usage` / `smart` / `weight` 五种策略，
  故障转移、会话粘性（`stays`）、嵌套组（带环检测）、单 provider 多 key。
- React + TypeScript 控制台：服务商 / 模型 / 路由 / 用量 / 设置 / caller
  key 管理；浏览器密钥只做掩码展示。
- CLI：`provider` / `group` / `models` / `usage` / `doctor`，以及
  `claude` / `codex` / `gemini` / `opencode` 一键改写各 agent 自己的配置。
- 用量记录：每请求一行 `usage.jsonl`（模型、provider、key、token、时延、
  状态），`UsageGuard` 保证客户端提前断开也落盘。
- 配置热重载：CLI / 其他进程改了 `config.json`，网关在下一个请求自动
  感知（mtime 探测），不用重启。
- Docker：多阶段构建（node 构建控制台 → rust 构建二进制 → alpine 运行时），
  非 root 运行、`/data` 持久化、自带 healthcheck，compose 一键起。
- 安全：
  - 网关所有端点要求 `Authorization: Bearer <key>`（或 `x-api-key` /
    `x-goog-api-key` / `api-key`）。
  - 控制台在回环地址免鉴权；绑到非回环地址时 `/api` 需要同一套 key。
  - 所有写请求校验 Origin，拒绝跨站驱动控制台。
  - `config.json` 以 0600 落盘；`LOCAL_ROUTER_DEFAULT_KEY` 只在内存。

[Unreleased]: https://github.com/TODO/local-router/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TODO/local-router/releases/tag/v0.1.0
