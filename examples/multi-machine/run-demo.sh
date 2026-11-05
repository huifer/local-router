#!/usr/bin/env bash
# 在本机用回路地址验证 local-router 的跨实例级联：
#
#   agent ──► 香港网关(:HK) ──► 美国节点(:US) ──► 上游(mock:MOCK)
#
# 用的是仓库里那两份正式配置（us-node / hk-gateway），只在临时目录里把
# 地址和端口改写成回路地址、走上游 mock，其余（caller key 鉴权、协议互译、
# 路由组、链路追踪）全是真实链路。
#
# 端口是动态挑的空闲端口，不会碰你正在跑的 local-router（比如 3435）。
#
# 用法： examples/multi-machine/run-demo.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BIN="$ROOT/target/release/local-router"
HK_KEY="hk-caller-key"

WORK="$(mktemp -d)"
PIDS=()

cleanup() {
  for pid in ${PIDS+"${PIDS[@]}"}; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -rf "$WORK"
}
trap cleanup EXIT

if [ ! -x "$BIN" ]; then
  echo "找不到 $BIN —— 先构建： cargo build --release" >&2
  exit 1
fi

# ------------------------------------------------------------ 空闲端口 --

pick_port() {
  node -e 'const s=require("net").createServer();
    s.listen(0,"127.0.0.1",()=>{const p=s.address().port;s.close(()=>console.log(p));});'
}

MOCK_PORT="$(pick_port)"
US_PORT="$(pick_port)"
while [ "$US_PORT" = "$MOCK_PORT" ]; do US_PORT="$(pick_port)"; done
HK_PORT="$(pick_port)"
while [ "$HK_PORT" = "$MOCK_PORT" ] || [ "$HK_PORT" = "$US_PORT" ]; do
  HK_PORT="$(pick_port)"
done

# -------------------------------------------- 1. 准备配置（改写为回路地址）

mkdir -p "$WORK/us-node" "$WORK/hk-gateway"
cp "$ROOT/examples/multi-machine/us-node/config.json" "$WORK/us-node/config.json"
cp "$ROOT/examples/multi-machine/hk-gateway/config.json" "$WORK/hk-gateway/config.json"

node -e '
const fs = require("fs");
const [usPath, hkPath, mockBase, usBase, mockPort, usPort, hkPort] = process.argv.slice(1);

const us = JSON.parse(fs.readFileSync(usPath, "utf8"));
us.providers[0].base_url = mockBase;      // 美国节点的上游 → mock
us.settings.gateway_addr = "127.0.0.1";
us.settings.gateway_port = Number(usPort);
us.settings.console = false;
fs.writeFileSync(usPath, JSON.stringify(us, null, 2));

const hk = JSON.parse(fs.readFileSync(hkPath, "utf8"));
hk.providers[0].base_url = usBase;        // 香港网关的上游 → 美国节点
hk.settings.gateway_addr = "127.0.0.1";
hk.settings.gateway_port = Number(hkPort);
hk.settings.console = false;
fs.writeFileSync(hkPath, JSON.stringify(hk, null, 2));
' \
  "$WORK/us-node/config.json" \
  "$WORK/hk-gateway/config.json" \
  "http://127.0.0.1:${MOCK_PORT}" \
  "http://127.0.0.1:${US_PORT}" \
  "$MOCK_PORT" "$US_PORT" "$HK_PORT"

# ---------------------------------------------------- 2. 起三个进程

PORT=$MOCK_PORT node "$ROOT/examples/multi-machine/mock-upstream/server.js" \
  >"$WORK/mock.log" 2>&1 &
PIDS+=($!)

LOCAL_ROUTER_HOME="$WORK/us-node" "$BIN" serve --no-console \
  >"$WORK/us.log" 2>&1 &
PIDS+=($!)

LOCAL_ROUTER_HOME="$WORK/hk-gateway" "$BIN" serve --no-console \
  >"$WORK/hk.log" 2>&1 &
PIDS+=($!)

wait_ready() { # url, 期望的 http code（any = 只要有响应）
  local url="$1" want="$2" i code
  for i in $(seq 1 80); do
    code="$(curl -s -o /dev/null -w '%{http_code}' "$url" 2>/dev/null || echo 000)"
    if [ "$code" != "000" ] && { [ "$want" = "any" ] || [ "$code" = "$want" ]; }; then
      return 0
    fi
    sleep 0.25
  done
  echo "服务没起来：$url" >&2
  tail -20 "$WORK"/*.log >&2 || true
  return 1
}

wait_ready "http://127.0.0.1:${MOCK_PORT}/v1/models" any
wait_ready "http://127.0.0.1:${US_PORT}/v1/models" any
wait_ready "http://127.0.0.1:${HK_PORT}/v1/models" any

echo "三个进程就绪： mock:${MOCK_PORT}  us-node:${US_PORT}  hk-gateway:${HK_PORT}"
echo

# ---------------------------------------------------- 3. 验证

echo "【1】不带 key —— 香港网关应当拒绝"
code="$(curl -s -o /dev/null -w '%{http_code}' \
  "http://127.0.0.1:${HK_PORT}/v1/messages" \
  -H 'anthropic-version: 2023-06-01' -H 'content-type: application/json' \
  -d '{"model":"claude-us/claude-sonnet-4-5","max_tokens":64,"messages":[{"role":"user","content":"ping"}]}' \
  2>/dev/null || echo 000)"
echo "    HTTP ${code}（期望 401/403）"
echo

echo "【2】带 key，请求走 claude-us/ 这个 provider（级联到美国节点）"
curl -s "http://127.0.0.1:${HK_PORT}/v1/messages" \
  -H "x-api-key: ${HK_KEY}" \
  -H 'anthropic-version: 2023-06-01' \
  -H 'content-type: application/json' \
  -d '{"model":"claude-us/claude-sonnet-4-5","max_tokens":64,"messages":[{"role":"user","content":"ping"}]}' \
  | head -c 700 || true
echo; echo

echo "【3】走路由组 group/claude（对外统一的名字）"
curl -s "http://127.0.0.1:${HK_PORT}/v1/messages" \
  -H "x-api-key: ${HK_KEY}" \
  -H 'anthropic-version: 2023-06-01' \
  -H 'content-type: application/json' \
  -d '{"model":"group/claude","max_tokens":64,"messages":[{"role":"user","content":"ping"}]}' \
  | head -c 700 || true
echo; echo

echo "【4】香港网关看到的链路（trace 树）"
LOCAL_ROUTER_HOME="$WORK/hk-gateway" "$BIN" traces 2>&1 | sed 's/^/    /' | head -25
echo

echo "【4b】美国节点看到的链路（同一请求在下游的那一半）"
LOCAL_ROUTER_HOME="$WORK/us-node" "$BIN" traces 2>&1 | sed 's/^/    /' | head -25
echo

TRACE_ID="$(LOCAL_ROUTER_HOME="$WORK/hk-gateway" "$BIN" traces 2>/dev/null | grep -oE 'trc_[0-9a-f]+' | head -1 || true)"
if [ -n "$TRACE_ID" ]; then
  echo "【4c】同一个 trace id 在两边对得上（${TRACE_ID}）"
  echo "    香港网关侧:"
  LOCAL_ROUTER_HOME="$WORK/hk-gateway" "$BIN" trace "$TRACE_ID" 2>&1 | sed 's/^/      /' | head -12
  echo "    美国节点侧:"
  LOCAL_ROUTER_HOME="$WORK/us-node" "$BIN" trace "$TRACE_ID" 2>&1 | sed 's/^/      /' | head -12
  echo
fi

echo "【5】上游 mock 收到的请求（证明确实穿过了美国节点）"
grep -E "^\[mock\]" "$WORK/mock.log" | sed 's/^/    /' || echo "    (mock 没收到请求)"
echo

echo "美国节点日志（最近 5 行）:"
tail -5 "$WORK/us.log" | sed 's/^/    /' || true
echo
echo "香港网关日志（最近 5 行）:"
tail -5 "$WORK/hk.log" | sed 's/^/    /' || true
