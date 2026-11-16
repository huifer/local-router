#!/usr/bin/env python3
"""批量探测每个上游能否真正对话：逐个 provider 发一个最小请求。

为什么要它：`local-router provider test <id>` 一次只测一家，接进来几十个
上游（尤其是从 Magpie 全量导入之后）时没法一眼看清哪些能用、哪些是凭证/
额度/网络问题。这个脚本读运行中网关的 `/api/config`（密钥已掩码），对每个
启用的 provider 用它的第一个模型发一次 `max_tokens=16` 的请求，并把结果
按「能对话 / 上游拒绝 / 连不上」分开。它**不改配置、不打印密钥**。

用法::

    LOCAL_ROUTER_KEY=… python3 scripts/probe_providers.py
    python3 scripts/probe_providers.py --base http://127.0.0.1:3435 \
        --key "$LOCAL_ROUTER_KEY" --timeout 60 --concurrency 4
    python3 scripts/probe_providers.py --provider zhipu --provider sub2api
    python3 scripts/probe_providers.py --cascade      # 再探 magpie/* 每个前缀
    python3 scripts/probe_providers.py --all          # 连 disabled 的也探

退出码：0 = 所有被探测的上游都返回 2xx；1 = 有失败（--all 时 disabled 的
`turned off` 也算失败）。超时的上游单独计数，因为慢模型常需要更多时间，
可以调大 `--timeout` 再跑一次。
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor


def get(url: str, key: str) -> dict:
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {key}"})
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.loads(r.read())


def chat(base: str, key: str, model: str, timeout: int, headers: dict[str, str]) -> tuple:
    body = json.dumps(
        {
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 16,
            "stream": False,
        }
    ).encode()
    req = urllib.request.Request(
        f"{base}/v1/chat/completions",
        data=body,
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {key}",
            **headers,
        },
    )
    started = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            payload = json.loads(r.read())
            text = (payload.get("choices") or [{}])[0].get("message", {}).get("content") or ""
            return r.status, time.time() - started, repr(text[:40])
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")[:200].replace("\n", " ")
        return e.code, time.time() - started, detail
    except Exception as e:  # noqa: BLE001 - 网络层错误都当成「连不上」
        return "ERR", time.time() - started, type(e).__name__


def targets_from_config(config: dict, only: list[str], include_disabled: bool) -> list[tuple]:
    out = []
    for p in sorted(config.get("providers", []), key=lambda x: x["id"]):
        if only and p["id"] not in only:
            continue
        if not p.get("enabled") and not include_disabled:
            continue
        models = p.get("models") or []
        if not models:
            out.append((p["id"], p["id"], "-"))
            continue
        out.append((p["id"], p["id"], models[0]))
    return out


def cascade_targets(base: str, key: str) -> list[tuple]:
    """magpie 这类级联：按上游前缀各取一个模型，避免把 300 个模型全打一遍。"""
    ids = [m["id"] for m in get(f"{base}/v1/models", key).get("data", [])]
    seen, out = set(), []
    for mid in ids:
        prefix = mid.split("/")[0]
        if prefix in seen or "/" not in mid:
            continue
        seen.add(prefix)
        out.append((prefix, "magpie", mid))
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description="逐个上游做一次最小对话，检查能否接入")
    ap.add_argument("--base", default="http://127.0.0.1:3435", help="网关地址")
    ap.add_argument("--key", default=os.environ.get("LOCAL_ROUTER_KEY", ""),
                    help="caller key（默认读 LOCAL_ROUTER_KEY）")
    ap.add_argument("--provider", action="append", default=[],
                    help="只探这几家（可重复）；默认探全部启用中的")
    ap.add_argument("--timeout", type=int, default=60, help="单次请求超时秒数")
    ap.add_argument("--concurrency", type=int, default=4, help="并发数")
    ap.add_argument("--cascade", action="store_true",
                    help="额外探测 magpie 级联里每个上游前缀（每个前缀取一个模型）")
    ap.add_argument("--all", action="store_true", help="连 disabled 的 provider 也探")
    ap.add_argument("--header", action="append", default=[], metavar="K=V",
                    help="附加请求头，如 x-opencode-session=abc（可重复）")
    args = ap.parse_args()

    if not args.key:
        print("缺少 caller key：用 --key 或环境变量 LOCAL_ROUTER_KEY", file=sys.stderr)
        return 2
    headers = dict(h.split("=", 1) for h in args.header if "=" in h)

    base = args.base.rstrip("/")
    try:
        config = get(f"{base}/api/config", args.key)
    except Exception as e:  # noqa: BLE001
        print(f"读不到 {base}/api/config：{e}", file=sys.stderr)
        return 2

    targets = targets_from_config(config, args.provider, args.all)
    if args.cascade:
        targets += cascade_targets(base, args.key)
    if not targets:
        print("没有要被探测的 provider（是不是都 disabled 了？加 --all）", file=sys.stderr)
        return 2

    def probe(t):
        name, provider, model = t
        if model == "-":
            return name, "-", "SKIP", 0.0, "no models"
        req_id = model if provider in ("magpie", "") else f"{provider}/{model}"
        status, seconds, detail = chat(base, args.key, req_id, args.timeout, headers)
        return name, req_id, status, seconds, detail

    rows = []
    with ThreadPoolExecutor(max_workers=max(1, args.concurrency)) as pool:
        for row in pool.map(probe, targets):
            rows.append(row)
            name, req_id, status, seconds, detail = row
            print(f"{status!s:>4} | {name:28} | {req_id:52} | {seconds:5.1f}s | {detail}")

    ok = sum(1 for r in rows if r[2] == 200)
    rejected = [r for r in rows if isinstance(r[2], int) and 400 <= r[2] < 500]
    # 5xx 与网络错误都算「没连上」：502/504 通常是网关连不上或上游挂断，
    # 与上游明确拒绝（4xx）要分开看，否则会把网络问题误判成凭证问题。
    unreachable = [r for r in rows if r[2] == "ERR" or (isinstance(r[2], int) and r[2] >= 500)]
    skipped = [r for r in rows if r[2] == "SKIP"]
    print(
        f"\n{ok} 可用 · {len(rejected)} 上游拒绝（4xx）· "
        f"{len(unreachable)} 连不上（5xx/超时）· {len(skipped)} 跳过"
    )
    if rejected:
        print("上游拒绝（看错误体：401/403 凭证、429 额度、404 模型下架）：")
        for name, _req, status, _s, _d in rejected:
            print(f"  {status} {name}")
    if unreachable:
        print("连不上（超时可调大 --timeout；被墙的上游给 provider 配 proxy）：")
        for name, _req, status, _s, _d in unreachable:
            print(f"  {status} {name}")
    return 0 if not rejected and not unreachable else 1


if __name__ == "__main__":
    sys.exit(main())
