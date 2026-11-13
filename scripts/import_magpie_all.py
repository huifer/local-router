#!/usr/bin/env python3
"""把所有 magpie 上游接进 local-router：直连导入 + magpie 级联兜底。

覆盖 providers.json 里全部 provider，分两层：

1. **直连**（有 URL 的）：复用 import_magpie_providers 的转换规则，
   一个 provider 一个上游，独立冷却 / 限额 / 计费。magpie 里 off 的也导入，
   但保持 enabled=false（`--enable-all` 才全部启用）。
2. **级联**（其余插件登录态上游，以及 magpie 侧未来的任何变化）：
   加一个 provider 指向 magpie 自己的本机网关，models 直接取 magpie
   `/v1/models` 当前返回的完整 id（形如 `llm-free/auto`）。
   local-router 转发时只剥掉自己这一层前缀，对面收到的正是它认识的 id；
   这些元素本身带斜杠，不参与裸名判定，所以**级联不引入裸名冲突**。
   group/* 会被跳过 —— 它会被 local-router 解析成**本地组**，不是上游模型。

直连与级联对同一批模型是冗余的两条路：直连少一跳、可单独限流；级联更全，
能覆盖 local-router 没有协议实现的插件登录态上游（kiro / qoder / copilot /
codex / joycode / scnet / atomcode / zcode 等）。

裸名保护（--drop-bare-conflicts）用**绝对规则**：一个裸名只要被任何非本次
导入的 enabled 上游提供，就不留在直连 provider 的 models 里。用「相对当前
配置的新增冲突」是不行的——models 合并取并集，重跑时上次删掉的名字会被并
回来，而基线已被自己的上一次输出污染，第二轮就删不干净（绝对规则与跑几次
无关，天然幂等）。

用法::

    python3 scripts/import_magpie_all.py                     # 报告 + 候选文件
    python3 scripts/import_magpie_all.py --drop-bare-conflicts --apply
    python3 scripts/import_magpie_all.py --cascade-id magpie --enable-all

候选文件默认在 /tmp/local-router.config.magpie-all.json，可先用隔离实例校验：
    mkdir -p /tmp/lr-check && cp <候选> /tmp/lr-check/config.json
    LOCAL_ROUTER_HOME=/tmp/lr-check local-router ls
--apply 前一定备份。daemon 靠 mtime 探测热重载，也可 `local-router daemon restart`。
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

from import_magpie_providers import (  # noqa: E402
    LR_FILE,
    MAGPIE_FILE,
    atomic_write,
    backup,
    convert_groups,
    convert_provider,
)

MAGPIE_DIR = pathlib.Path(
    os.environ.get("MAGPIE_HOME", str(pathlib.Path.home() / ".config" / "magpie"))
)
GATEWAY_FILE = MAGPIE_DIR / "gateway.json"


def cascade_base(port: int | None) -> str:
    """magpie 本机网关地址：优先读它自己写的 gateway.json（端口会变）。"""
    if port is not None:
        return f"http://127.0.0.1:{port}/v1"
    if GATEWAY_FILE.exists():
        try:
            addr = str(json.loads(GATEWAY_FILE.read_text())["addr"])
            host, _, number = addr.rpartition(":")
            if number.isdigit():
                return f"http://{host or '127.0.0.1'}:{number}/v1"
        except Exception:  # noqa: BLE001 - 读不到就退回默认端口
            pass
    return "http://127.0.0.1:3425/v1"


def cascade_key() -> str:
    """magpie 的 caller key。回环上它不一定拦，但别赌。"""
    path = MAGPIE_DIR / "caller-keys.json"
    if not path.exists():
        return ""
    try:
        entries = json.loads(path.read_text())
    except Exception:  # noqa: BLE001
        return ""
    if isinstance(entries, list):
        for entry in entries:
            if isinstance(entry, dict) and entry.get("secret"):
                return str(entry["secret"])
    return ""


def cascade_models(base: str, key: str, timeout: int = 15):
    """问 magpie 要它现在服务的全部模型 id。"""
    request = urllib.request.Request(
        base.rstrip("/") + "/models",
        headers={"authorization": f"Bearer {key}"} if key else {},
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            data = json.loads(response.read())
    except (urllib.error.URLError, OSError, ValueError) as err:
        return None, f"取 magpie /v1/models 失败：{err}"
    ids, groups = [], []
    for item in data.get("data") or []:
        model = str(item.get("id") or "")
        if not model:
            continue
        (groups if model.startswith("group/") else ids).append(model)
    note = f"magpie 现在服务 {len(ids)} 个模型 id" + (
        f"，跳过 {len(groups)} 个 group/*（{', '.join(groups)}）" if groups else ""
    )
    return ids, note


def index(providers):
    """裸模型名 → 提供它的 enabled provider id 列表。"""
    out = {}
    for p in providers:
        if not p.get("enabled", True):
            continue
        for model in p.get("models") or []:
            out.setdefault(model, []).append(p.get("id", ""))
    return out


def main() -> None:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--apply", action="store_true", help="写进真实 config.json（先备份）")
    ap.add_argument("--candidate", type=pathlib.Path,
                    default=pathlib.Path("/tmp/local-router.config.magpie-all.json"),
                    help="候选配置输出路径")
    ap.add_argument("--cascade-id", default="magpie", help="级联 provider 的 id")
    ap.add_argument("--cascade-port", type=int, help="magpie 网关端口（默认读 gateway.json）")
    ap.add_argument("--no-cascade", action="store_true", help="不做级联，只直连导入")
    ap.add_argument("--enable-all", action="store_true",
                    help="连 magpie 里 off 的 provider 也置 enabled=true")
    ap.add_argument("--drop-bare-conflicts", action="store_true",
                    help="从直连 provider 的 models 里删掉「别的上游也提供」的裸名，"
                         "保住裸名直连（这些模型仍经级联 provider 可达）")
    ap.add_argument("--magpie-file", type=pathlib.Path, default=MAGPIE_FILE)
    args = ap.parse_args()

    def say(line=""):
        print(line, file=sys.stderr)

    magpie = json.loads(args.magpie_file.read_text(encoding="utf-8"))
    existing = json.loads(LR_FILE.read_text(encoding="utf-8")) if LR_FILE.exists() else {}

    # ---- 1. 直连：有 URL 的全部导入（off 的留 enabled=false） ----
    warnings: list[str] = []
    direct, session_only, off_ids = [], [], []
    for raw in magpie.get("providers") or []:
        pid = str(raw.get("id") or "")
        if not pid:
            continue
        if not any(isinstance(raw.get(k), str) and raw[k].strip()
                   for k in ("chat", "anthropic", "responses", "gemini")):
            session_only.append(pid)
            continue
        if raw.get("off"):
            off_ids.append(pid)
        item = convert_provider(raw, {}, warnings, keep_placeholders=False)
        if item:
            direct.append(item)

    # ---- 2. 级联：magpie 本机网关 ----
    cascade = None
    cascade_note = ""
    if not args.no_cascade:
        base = cascade_base(args.cascade_port)
        key = cascade_key()
        ids, cascade_note = cascade_models(base, key)
        if ids is None:
            say(f"× {cascade_note}；本次退回「只直连」")
        else:
            cascade = {
                "id": args.cascade_id,
                "name": "Magpie（本机网关级联）",
                "kind": "custom",
                "enabled": True,
                "protocol": "openai_chat",
                "base_url": base,
                "keys": ([{"id": "magpie-caller", "key": key, "enabled": True,
                           "label": "magpie caller key"}] if key else []),
                "models": ids,
            }

    # ---- 3. 合并：已有 provider 保留（密钥永不覆盖），模型取并集 ----
    known = {p["id"] for p in direct} | ({cascade["id"]} if cascade else set())
    existing_providers = list(existing.get("providers") or [])
    groups = convert_groups(
        magpie.get("groups"),
        known | {str(p.get("id", "")) for p in existing_providers},
        warnings,
    )

    providers = existing_providers
    by_id = {str(p.get("id")): p for p in providers}
    added, updated = [], []
    for item in direct + ([cascade] if cascade else []):
        old = by_id.get(item["id"])
        if old is None:
            providers.append(item)
            by_id[item["id"]] = item
            added.append(item["id"])
            continue
        saved_keys = old.get("keys") or []
        item = dict(item)
        if not (cascade and item["id"] == args.cascade_id):
            # 级联的模型表以 magpie 现况为准（不累积陈旧项）；其余取并集。
            merged = list(old.get("models") or [])
            item["models"] = merged + [m for m in item["models"] if m not in merged]
        old.update(item)
        if saved_keys:
            old["keys"] = saved_keys
        updated.append(item["id"])

    if args.enable_all:
        for p in providers:
            p["enabled"] = True

    # ---- 4. 裸名保护：绝对规则，与跑几次无关（幂等） ----
    # 两个条件同时成立才删：
    #   a) 该裸名被「非本次直连导入」的 enabled 上游也提供（= 冲突的根源，
    #      这个集合不依赖上次删了什么，所以幂等）；
    #   b) 级联表里有 `<本 provider id>/<该名字>`，删了仍可达（不丢功能）。
    # 不满足 b) 就保留（宁可继续有歧义，也不把模型删到谁也访问不到）。
    direct_ids = {p["id"] for p in direct}
    cascade_set = set(cascade["models"]) if cascade else set()
    dropped_from: dict[str, int] = {}
    kept_ambiguously: list[str] = []
    if args.drop_bare_conflicts:
        others = index([p for p in providers if str(p.get("id")) not in direct_ids])
        for provider in providers:
            pid = str(provider.get("id"))
            if pid not in direct_ids:
                continue
            models = provider.get("models") or []
            keep, dropped = [], []
            for model in models:
                conflict = model in others
                recoverable = f"{pid}/{model}" in cascade_set
                if conflict and recoverable:
                    dropped.append(model)
                elif conflict:
                    kept_ambiguously.append(f"{pid}/{model}")
                else:
                    keep.append(model)
            if dropped:
                dropped_from[pid] = len(dropped)
                provider["models"] = keep

    after_map = index(providers)
    still_ambiguous = sorted(m for m, v in after_map.items() if len(v) > 1)

    have = {str(g.get("id")) for g in existing.get("groups") or []}
    config = {
        "providers": providers,
        "groups": list(existing.get("groups") or []) + [
            g for g in groups if g["id"] not in have
        ],
        "settings": existing.get("settings") or {},
        "caller_keys": existing.get("caller_keys") or [],
        "revision": existing.get("revision", 0),
        "schema_version": 1,
    }

    # ---- 报告 ----
    say("═" * 70)
    say(f"· 源：{args.magpie_file}（{len(magpie.get('providers') or [])} 个 provider）")
    say(f"· 直连导入 {len(direct)} 个（magpie 里 off 的 {len(off_ids)} 个：默认 enabled=false）")
    say(f"· 插件登录态/无 URL {len(session_only)} 个 → 只走级联："
        + (", ".join(session_only) or "（无）"))
    say(f"· 级联 provider："
        + ((f"「{args.cascade_id}」 {cascade_base(args.cascade_port)} — {cascade_note}")
           if cascade else f"未启用（{cascade_note or '--no-cascade'}）"))
    say(f"· 路由组 {len(groups)} 个：" + (", ".join(g["id"] for g in groups) or "（无）"))
    say(f"· 合并：新增 {len(added)}、更新 {len(updated)}、共 {len(providers)} 个 provider，"
        f"{sum(len(p.get('models') or []) for p in providers)} 个模型名")
    if args.drop_bare_conflicts:
        say(f"· 裸名保护："
            + ("、".join(f"{k} 删 {v}" for k, v in sorted(dropped_from.items()))
               if dropped_from else "无变化（已幂等）"))
        if kept_ambiguously:
            say(f"· 保留但仍有歧义（级联表里没有它，删了就谁也访问不到）"
                f" {len(kept_ambiguously)} 个："
                + ", ".join(kept_ambiguously[:8])
                + ("…" if len(kept_ambiguously) > 8 else ""))
    say(f"· 仍多方提供的裸名：{len(still_ambiguous)} 个"
        + ("（本地原有上游互相重叠，与本次导入无关，且不受白名单保护影响）"
           if still_ambiguous else ""))
    say("· 提醒：裸名一旦有两家提供，resolve_bare_model 直接报 400（不是随机选一家），")
    say("  发裸名的客户端（Claude Code 等）会失败 —— agent 里请写 <provider>/<模型> 或组名。")
    say("· 注意：--drop-bare-conflicts 只删「级联表里确实有 `<id>/<名字>`」的名字，")
    say("  被删的名字可经级联取回；但直写 <provider>/<名字> 会报 does not serve。")
    say("  路由组成员不受白名单限制（route.rs::plan_group 只查 provider.enabled）。")
    if warnings:
        say("· 转换提醒：")
        for w in warnings[:20]:
            say(f"    - {w}")
    say("═" * 70)

    atomic_write(args.candidate, config)
    say(f"✓ 候选写出 {args.candidate}")
    if args.apply:
        note = backup(LR_FILE)
        atomic_write(LR_FILE, config)
        say(f"✓ 已应用到 {LR_FILE}" + (f"（备份 {note}）" if note else ""))
        say("· daemon 下一个请求按 mtime 自动重载；立刻生效：local-router daemon restart")
    else:
        say("· 真实配置未改动。校验后再 --apply：")
        say(f"    mkdir -p /tmp/lr-check && cp {args.candidate} /tmp/lr-check/config.json")
        say("    LOCAL_ROUTER_HOME=/tmp/lr-check local-router ls")


if __name__ == "__main__":
    main()
