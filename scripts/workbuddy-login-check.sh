#!/usr/bin/env bash
# 检查 WorkBuddy 登录状态，并验证 local-router 能不能用这个登录态。
# 用法：bash scripts/workbuddy-login-check.sh
set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; NC='\033[0m'
ok()   { printf "%b  %s\n" "${GREEN}✔${NC}" "$1"; }
warn() { printf "%b  %s\n" "${YELLOW}!${NC}" "$1"; }
fail() { printf "%b  %s\n" "${RED}✘${NC}" "$1"; }

cd "$(dirname "$0")/.."
BIN=./target/release/local-router
if [ ! -x "$BIN" ]; then
  echo "先构建：cargo build --release"
  exit 1
fi

echo "== 1. WorkBuddy 客户端 =="
if [ -d "/Applications/WorkBuddy.app" ]; then
  ok "已安装 /Applications/WorkBuddy.app"
else
  warn "未在 /Applications 找到 WorkBuddy.app（登录要在客户端里完成）"
fi
if pgrep -f "WorkBuddy.app" >/dev/null 2>&1; then
  ok "客户端正在运行"
else
  warn "客户端未运行 —— 让登录的人打开 WorkBuddy 并登录"
fi

echo
echo "== 2. 登录态文件 =="
if "$BIN" workbuddy status; then
  :
else
  fail "读取登录态失败"
fi

echo
echo "== 3. 登录态是否真的能用（走服务端） =="
if "$BIN" workbuddy credits >/tmp/wb-check-credits 2>&1; then
  ok "服务端接受了这个登录态（额度/签到可读）："
  sed 's/^/    /' /tmp/wb-check-credits | head -3
else
  fail "服务端拒绝了这个登录态："
  sed 's/^/    /' /tmp/wb-check-credits
fi
if "$BIN" workbuddy models >/tmp/wb-check-models 2>&1; then
  n=$(tail -n +2 /tmp/wb-check-models | wc -l | tr -d ' ')
  ok "可读模型目录：$n 个模型"
else
  fail "模型目录读取失败："
  sed 's/^/    /' /tmp/wb-check-models
fi

echo
echo "== 4. 接入 local-router（临时目录，不动你的配置） =="
TEST_HOME=$(mktemp -d /tmp/lr-login-check.XXXXXX)
LOCAL_ROUTER_HOME="$TEST_HOME" "$BIN" provider add workbuddy >/dev/null 2>&1
if grep -q '"auth": "workbuddy"' "$TEST_HOME/config.json" 2>/dev/null; then
  ok "provider 已可配置（无需 API key）"
else
  fail "provider 配置失败"
fi

echo
echo "== 5. 真实推理验证 =="
if LOCAL_ROUTER_HOME="$TEST_HOME" timeout 60 "$BIN" provider test workbuddy >/tmp/wb-check-test 2>&1; then
  ok "通过 local-router 发起了一次真实 WorkBuddy 请求："
  head -c 160 /tmp/wb-check-test | sed 's/^/    /'; echo
else
  warn "推理请求未成功（登录仍有效，只是额度/模型限制）："
  head -c 200 /tmp/wb-check-test | sed 's/^/    /'; echo
fi
rm -rf "$TEST_HOME"

echo
echo "== 提示：没有登录态？=="
echo "  不装客户端也可以登录：local-router workbuddy login"
echo "  （打印链接并打开浏览器，登录后自动接入）"

echo
echo "== 结论 =="
if "$BIN" workbuddy status >/dev/null 2>&1 && "$BIN" workbuddy credits >/dev/null 2>&1; then
  echo "登录有效。可以正常使用：local-router provider add workbuddy"
  echo "额度与签到看：local-router workbuddy credits"
else
  echo "登录无效或未完成。请让对方："
  echo "  1. 打开 WorkBuddy.app"
  echo "  2. 右上角登录（浏览器 OAuth 回调，用手机号/微信均可）"
  echo "  3. 登录完成后重新运行本脚本"
fi
