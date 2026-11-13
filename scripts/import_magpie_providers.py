#!/usr/bin/env python3
"""把 magpie 的 providers.json 转成 local-router 的 config.json。

两者都是「providers 数组」，但字段语义并不一样，直接把文件拷过去不会被
加载成可用配置：

  magpie                         local-router
  ---------------------------    ----------------------------------------
  key (字符串) / keys[]          keys[]: [{id,key,enabled,label}]
  chat / anthropic / responses   protocol + base_url + *_base_url
  baseAPI: chat|responses        protocol: openai_chat|openai_responses
  off: true                      enabled: false
  groups[].members               groups[].models
  routing (账号级 rotate/…)      routing (组成员级)；多 key 是顺序 failover
  insecureTLS / decide /        无对应（见报告里的 warnings）
  accountModels / keyProtocol

还有两件事无法从文件里得到：

* **密钥**：magpie 写在磁盘上的是不可逆的 HMAC 占位符（形如 {{...}}），
  真实值只在运行中的 magpie 进程里。本脚本默认跳过占位符（宁可留空，
  也不要写进一个看起来能用、实际必然 401 的 key），带 --keys-from-web
  时会去问 magpie 的本机网页要真实密钥（需先在 magpie 里
  Settings → 网页 →「此电脑」打开网页，和 import_magpie_keys.py 一样）。
* **登录态 provider**：workbuddy / qwenwork / trae-cn / copilot / codex 等
  在 magpie 里既没有 URL 也没有 key，靠 magpie 插件的登录会话。它们在
  local-router 里是专用通道，要用各自的 session 文件，不能从这份 JSON
  生成。这些会被跳过并在报告里列出来。

用法（默认只打印报告，不动任何文件）::

    python3 scripts/import_magpie_providers.py
    python3 scripts/import_magpie_providers.py --out /tmp/lr-config.json
    python3 scripts/import_magpie_providers.py --merge        # 追加到现有 config
    python3 scripts/import_magpie_providers.py --keys-from-web --merge
    python3 scripts/import_magpie_providers.py --include-off  # 连禁用的也导
    python3 scripts/import_magpie_providers.py --only sub2api,nvidia,longcat

``--out`` 写出完整 config（settings/caller_keys 从现有 config 继承），
``--merge`` 只追加现有 config 里没有的 provider（默认不改动已有条目，
``--update`` 才覆盖结构字段，但永远保留已有密钥）。两种写法都会先备份。
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import datetime
import re
import shutil
import sys
import tempfile
from urllib.parse import urlsplit, urlunsplit

HOME = pathlib.Path.home()
MAGPIE_FILE = pathlib.Path(
    os.environ.get("MAGPIE_HOME", str(HOME / ".config" / "magpie"))
) / "providers.json"
LR_DIR = pathlib.Path(
    os.environ.get("LOCAL_ROUTER_HOME", str(HOME / ".config" / "local-router"))
)
LR_FILE = LR_DIR / "config.json"

PLACEHOLDER = re.compile(r"^\{\{[A-Z_]+_[a-z0-9]+\}\}$")

# magpie 的 baseAPI → local-router 的 protocol，以及它该读哪个 URL 字段。
APIS = {
    "chat": ("openai_chat", "chat", "base_url"),
    "responses": ("openai_responses", "responses", "base_url"),
    "anthropic": ("anthropic", "anthropic", "base_url"),
    "gemini": ("gemini", "gemini", "base_url"),
}
URL_FIELDS = {
    "chat": "base_url",
    "anthropic": "anthropic_base_url",
    "responses": "responses_base_url",
    "gemini": "gemini_base_url",
}
# local-router 的 client::url_for 在 base 不以 /v1 结尾时会补一层 /v1。
DEFAULT_PATH = {
    "chat": "/chat/completions",
    "responses": "/responses",
    "anthropic": "/v1/messages",
}
ROUTINGS = {"order": "order", "rotate": "rotate", "usage": "usage", "weight": "weight"}
SESSION_IDS = {
    "workbuddy", "qwenwork", "trae-cn", "copilot", "codex", "zcode",
    "kiro", "qoder", "atomcode", "scnet-token-plan", "joycode",
}


def is_placeholder(value: str) -> bool:
    return bool(PLACEHOLDER.match(value.strip()))


def split_endpoint(url: str, api: str):
    """把 magpie 的 URL 拆成 (base_url, chat_path)。

    local-router 的 base_url 是「基地址」，会自己拼 /chat/completions 等；
    magpie 的 URL 有时已经是完整 endpoint，有时带自定义路径（例如
    /api/coding/paas/v4），这两种都不能让 url_for 再补一层 /v1。
    """
    parts = urlsplit(url.strip().rstrip("/"))
    path = parts.path.rstrip("/")
    base = urlunsplit((parts.scheme, parts.netloc, path, "", ""))

    for suffix in ("/chat/completions", "/responses", "/v1/messages", "/messages"):
        if path.endswith(suffix):
            return base[: -len(suffix)].rstrip("/"), suffix

    if not path or path.endswith("/v1") or path.endswith("/v1beta"):
        return base, None
    # 自定义前缀（如 /api/coding/paas/v4、/zen/go/v1 的变体）：钉死路径，
    # 否则 url_for 会插进来一个多余的 /v1。
    if api == "gemini":
        # gemini 的路径里带模型名，chat_path 覆盖会把它弄丢。
        return base, None
    return base, DEFAULT_PATH[api]


def convert_provider(raw: dict, real_keys: dict[str, str], warnings: list[str],
                     keep_placeholders: bool = False):
    """返回 local-router 的 Provider dict，或 None（无法转换）。"""
    pid = raw.get("id", "").strip()
    if not pid:
        return None

    urls = {api: raw[api].strip() for api in ("chat", "anthropic", "responses", "gemini")
            if isinstance(raw.get(api), str) and raw[api].strip()}
    if not urls:
        return None  # 会话型 / 插件型 provider，报告里单独列

    api = raw.get("baseAPI") or "chat"
    if api not in APIS:
        warnings.append(f"{pid}: 未知的 baseAPI={api!r}，按 chat 处理")
        api = "chat"
    if api not in urls:
        # baseAPI 指向的 URL 不存在（例如只填了 anthropic 却说 baseAPI=chat）
        api = next(iter(urls))
        warnings.append(f"{pid}: baseAPI 指向的 URL 缺失，改用 {api}")
    protocol, _, _ = APIS[api]

    # 主 URL 决定 protocol 与 chat_path；其余 URL 作为切协议时的备用字段。
    # local-router 的出站协议恒等于 provider.protocol（route.rs::targets_for），
    # 且 provider test 直接读 base_url，所以 base_url 必须是主协议的地址。
    base, chat_path = split_endpoint(urls[api], api)
    provider = {
        "id": pid,
        "name": (raw.get("name") or "").strip() or pid,
        "kind": raw.get("preset") or "custom",
        "enabled": not raw.get("off", False),
        "protocol": protocol,
        "base_url": base,
        "keys": [],
        "models": list(raw.get("models") or []),
    }
    for name, field in URL_FIELDS.items():
        if name not in urls or name == api:
            continue
        if field == "base_url":
            warnings.append(
                f"{pid}: 另有 chat URL {urls[name]!r} 未导入——主协议是 {api}，"
                "local-router 的 base_url 只能放主协议地址（要用的话请手动改 protocol）"
            )
            continue
        value, _ = split_endpoint(urls[name], name)
        provider[field] = value
    # chat_path 是 provider 级覆盖，会作用于所有协议；只按主协议计算。
    if chat_path:
        provider["chat_path"] = chat_path
        if len(urls) > 1:
            warnings.append(
                f"{pid}: base_url 是自定义路径，已设 chat_path={chat_path!r}；"
                "这个覆盖对其它协议也生效，日后切换 protocol 时请复核"
            )

    if raw.get("catalog"):
        provider["catalog"] = str(raw["catalog"]).split(",")[0].strip()

    # 密钥：主 key + keys[]。占位符默认丢弃，避免写入必然 401 的值。
    entries = []
    if isinstance(raw.get("key"), str) and raw["key"].strip():
        entries.append(("magpie-" + pid, raw["key"].strip(), "主密钥", True))
    for i, extra in enumerate(raw.get("keys") or []):
        if not isinstance(extra, dict) or not isinstance(extra.get("key"), str):
            continue
        entries.append((
            f"magpie-{pid}-{i + 1}",
            extra["key"].strip(),
            extra.get("name") or f"账号{i + 1}",
            not extra.get("off", False),
        ))
    missing = 0
    for key_id, value, label, enabled in entries:
        if is_placeholder(value) and not keep_placeholders:
            value = real_keys.get(pid) if label == "主密钥" else None
            if not value or is_placeholder(value):
                missing += 1
                continue
        provider["keys"].append({
            "id": key_id, "key": value, "enabled": enabled, "label": label,
        })
    if missing:
        warnings.append(
            f"{pid}: {missing} 个密钥是 magpie 的占位符，已留空——"
            "用 --keys-from-web 取真实密钥，或跑 scripts/import_magpie_keys.py"
        )

    if raw.get("routing") and len(entries) > 1:
        warnings.append(
            f"{pid}: magpie 的账号级 routing={raw['routing']!r} 无法映射；"
            "local-router 把同一 provider 的多个 key 当成顺序 failover"
        )
    if raw.get("insecureTLS"):
        warnings.append(
            f"{pid}: magpie 关闭了 TLS 校验，local-router 没有这个开关；"
            "自签名证书的上游会连不上"
        )
    if raw.get("decide"):
        warnings.append(f"{pid}: magpie 的 decide URL 已忽略（网关不做 decision API）")
    if raw.get("keyProtocol"):
        warnings.append(
            f"{pid}: keyProtocol={raw['keyProtocol']!r} 已忽略；"
            "local-router 按 protocol 决定认证头"
        )
    if raw.get("accountModels"):
        warnings.append(f"{pid}: 账号级模型白名单 accountModels 无法映射，已忽略")
    concurrency = raw.get("accountConcurrency")
    if isinstance(concurrency, dict) and concurrency:
        limits = {v for v in concurrency.values() if isinstance(v, int)}
        if len(limits) == 1:
            provider["max_concurrency"] = limits.pop()
        else:
            warnings.append(f"{pid}: 账号级并发数不一致，已忽略（可用 provider limit 设）")
    return provider


def convert_groups(raw_groups, known_ids: set[str], warnings: list[str]):
    groups = []
    for raw in raw_groups or []:
        gid = raw.get("id", "").strip()
        if not gid:
            continue
        if raw.get("disabled"):
            warnings.append(f"group/{gid}: magpie 里是禁用的，未导入")
            continue
        off = set(raw.get("off") or [])
        models = []
        dropped = []
        for member in raw.get("members") or []:
            head = member.split("/", 1)[0]
            if member in off or head not in known_ids:
                dropped.append(member)
                continue
            models.append(member)
        if dropped:
            warnings.append(
                f"group/{gid}: {len(dropped)} 个成员被剔除"
                "（在 magpie 里关掉，或其 provider 没能导入）"
            )
        if not models:
            warnings.append(f"group/{gid}: 没有可用成员，未导入")
            continue
        groups.append({
            "id": gid,
            "name": raw.get("name") or gid,
            "models": models,
            "routing": ROUTINGS.get(raw.get("routing"), "smart"),
            "listed": True,
        })
    return groups


def bare_conflicts(existing: list[dict], incoming: list[dict]):
    """导入后会让「原本唯一的裸模型名」变成多方提供的那些名字。

    route.rs::resolve_bare_model 只在恰好一个 enabled provider 提供某裸名时
    直接路由；Claude Code 这类客户端发的就是裸名。返回 [(名字, 原有 id, 新 id)]。
    """
    def index(providers):
        out = {}
        for p in providers:
            if not p.get("enabled", True):
                continue
            for model in p.get("models") or []:
                out.setdefault(model, []).append(p["id"])
        return out

    before, after = index(existing), index(existing + incoming)
    return [
        (model, before[model][0], [i for i in after[model] if i != before[model][0]])
        for model in after
        if len(before.get(model, ())) == 1 and len(after[model]) > 1
    ]


def fetch_real_keys(ids: list[str], warnings: list[str]) -> dict[str, str]:
    """问 magpie 的本机网页要真实密钥（复用 import_magpie_keys.py）。"""
    try:
        import import_magpie_keys as imp  # 同目录
    except ImportError as err:
        warnings.append(f"--keys-from-web: 找不到 import_magpie_keys.py（{err}）")
        return {}
    try:
        host, port, key = imp.page_address()
    except SystemExit:
        warnings.append(
            "--keys-from-web: magpie 的本机网页没开；请在 magpie 里"
            " Settings → 网页 →「此电脑」打开后重跑"
        )
        return {}
    base = f"http://{host}:{port}"
    found = {}
    for pid in ids:
        try:
            value = imp.magpie_key(base, pid, key)
        except Exception as err:  # noqa: BLE001 - 单个 provider 失败不阻断其余
            warnings.append(f"--keys-from-web: {pid} 取密钥失败（{err}）")
            continue
        if value and not is_placeholder(value):
            found[pid] = value
        else:
            warnings.append(f"--keys-from-web: {pid} 拿到的仍是占位符")
    return found


def atomic_write(path: pathlib.Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix="config.json.", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            json.dump(data, out, ensure_ascii=False, indent=2)
            out.write("\n")
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def backup(path: pathlib.Path) -> str:
    """备份到带时间戳的新文件。

    用固定名的话，第二次运行会拿「已被第一次改过的」配置覆盖掉原始备份，
    于是最想要的那份反而丢了。
    """
    if not path.exists():
        return ""
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    dst = path.parent / f"{path.name}.before-magpie-import.{stamp}"
    shutil.copy2(path, dst)
    return str(dst)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=pathlib.Path, help="写出完整 config 到指定文件")
    ap.add_argument("--merge", action="store_true", help="合并进现有的 config.json")
    ap.add_argument("--update", action="store_true", help="--merge 时也覆盖已存在的 provider 结构字段（保留其密钥）")
    ap.add_argument("--keys-from-web", action="store_true", help="向 magpie 本机网页要真实密钥")
    ap.add_argument("--keep-placeholders", action="store_true", help="把 magpie 的占位符原样写进 keys（会 401，仅供离线检查）")
    ap.add_argument("--include-off", action="store_true", help="连 magpie 里 off 的 provider 也导（enabled=false）")
    ap.add_argument("--include-picked", action="store_true", help="把 pickedFrom 候选模型也并进 models")
    ap.add_argument("--no-groups", action="store_true", help="不导入路由组")
    ap.add_argument("--drop-bare-conflicts", action="store_true",
                    help="从新 provider 的 models 里删掉重名项以恢复裸名直连；"
                         "代价：这些模型在新上游上彻底不可访问（models 也是白名单）")
    ap.add_argument("--enable-all", action="store_true",
                    help="导入后把 enabled 全部置 true（连 magpie 里 off 的也直接启用）")
    ap.add_argument("--only", help="只导这些 provider id，逗号分隔")
    ap.add_argument("--magpie-file", type=pathlib.Path, default=MAGPIE_FILE)
    args = ap.parse_args()

    if args.out and args.merge:
        ap.error("--out 和 --merge 二选一")

    if not args.magpie_file.exists():
        sys.exit(f"× 找不到 {args.magpie_file}")
    magpie = json.loads(args.magpie_file.read_text(encoding="utf-8"))
    existing = json.loads(LR_FILE.read_text(encoding="utf-8")) if LR_FILE.exists() else {}

    only = {x.strip() for x in args.only.split(",")} if args.only else None
    warnings: list[str] = []
    candidates, skipped_session, skipped_off = [], [], []
    for raw in magpie.get("providers") or []:
        pid = raw.get("id", "")
        if only and pid not in only:
            continue
        if raw.get("off") and not args.include_off:
            skipped_off.append(pid)
            continue
        if not any(isinstance(raw.get(k), str) and raw[k].strip()
                   for k in ("chat", "anthropic", "responses", "gemini")):
            skipped_session.append(pid + ("（登录态）" if pid in SESSION_IDS else ""))
            continue
        candidates.append(raw)

    real_keys = {}
    if args.keys_from_web:
        real_keys = fetch_real_keys([r["id"] for r in candidates], warnings)

    converted = []
    for raw in candidates:
        item = convert_provider(raw, real_keys, warnings, args.keep_placeholders)
        if not item:
            continue
        if args.include_picked:
            seen = set(item["models"])
            for model in raw.get("pickedFrom") or []:
                if isinstance(model, str) and model not in seen:
                    seen.add(model)
                    item["models"].append(model)
        converted.append(item)

    existing_providers = list(existing.get("providers") or [])
    existing_ids = {p.get("id") for p in existing_providers}
    fresh = [p for p in converted if p["id"] not in existing_ids]
    conflicts = bare_conflicts(existing_providers, fresh) if existing_providers else []
    if conflicts and args.drop_bare_conflicts:
        drop = {name for name, _, _ in conflicts}
        for p in fresh:
            p["models"] = [m for m in p["models"] if m not in drop]
        warnings.append(
            f"--drop-bare-conflicts: 从新 provider 的 models 里删了 {len(drop)} 个重名项。"
            "注意：models 同时是白名单（route.rs 会校验），删了就等于这些模型在该上游"
            "彻底不可访问；要「全部能接」就不要用这个开关"
        )
        conflicts = []

    known = {p["id"] for p in converted}
    if args.merge:
        known |= existing_ids
    groups = [] if args.no_groups else convert_groups(magpie.get("groups"), known, warnings)

    default_settings = {
        "gateway_port": 3435, "web_port": 0, "console": True, "lan": False,
        "gateway_addr": "127.0.0.1", "web_addr": "127.0.0.1",
        "default_caller_key": "local-router", "send_agent_user_agent": False,
        "masking": False, "record_usage": True, "trace_metadata": False,
        "locale": "zh-CN", "cors_origins": [], "workbuddy_auto_checkin": True,
    }
    settings = dict(default_settings)
    settings.update(existing.get("settings") or {})
    config = {
        "providers": converted,
        "groups": groups,
        "settings": settings,
        "caller_keys": existing.get("caller_keys") or [],
        "revision": existing.get("revision", 0),
        "schema_version": 1,
    }

    # --- 报告 ---
    def say(line=""):
        print(line, file=sys.stderr)

    say(f"· 源文件 {args.magpie_file}：{len(magpie.get('providers') or [])} 个 provider，"
        f"{len(magpie.get('groups') or [])} 个组")
    say(f"· 可转换 {len(converted)} 个：" + (", ".join(p["id"] for p in converted) or "（无）"))
    if args.merge:
        say(f"· --merge 实际会新增 {len(fresh)} 个（已存在的 {len(converted) - len(fresh)} 个默认跳过）："
            + (", ".join(p["id"] for p in fresh) or "（无）"))
    say(f"· 跳过（无 URL / 登录态，需 session 文件）{len(skipped_session)} 个："
        + (", ".join(skipped_session) or "（无）"))
    say(f"· 跳过（magpie 里 off）{len(skipped_off)} 个：" + (", ".join(skipped_off) or "（无）"))
    say(f"· 路由组 {len(groups)} 个：" + (", ".join(g["id"] for g in groups) or "（无）"))
    no_key = [p["id"] for p in converted if not p["keys"]]
    say(f"· 没有密钥的 {len(no_key)} 个：" + (", ".join(no_key) or "（无）"))
    if conflicts:
        say(f"× 裸模型名冲突 {len(conflicts)} 个——这些名字原本能直接路由，"
            "导入后会报 'is served by both'（Claude Code 等发裸名的客户端会受影响）：")
        for name, old, added in sorted(conflicts)[:12]:
            say(f"    - {name}: 原 {old} → 新增 {', '.join(added)}")
        if len(conflicts) > 12:
            say(f"    - …另有 {len(conflicts) - 12} 个")
        say("  取舍：带前缀的 <新 provider>/<名字> 仍可用，只失去这些名字的裸名直连；"
            "保留全部模型（默认）>> 删模型换裸名。真要删才加 --drop-bare-conflicts")
    if warnings:
        say("· 提醒：")
        for w in warnings:
            say(f"    - {w}")
    say()

    if args.merge:
        providers = list(existing.get("providers") or [])
        by_id = {p.get("id"): p for p in providers}
        added = updated = 0
        for item in converted:
            old = by_id.get(item["id"])
            if not old:
                providers.append(item)
                by_id[item["id"]] = item
                added += 1
            elif args.update:
                saved_keys = old.get("keys") or []
                # models 取并集：本地可能已经 expose 了 magpie 里没有的模型。
                merged = list(old.get("models") or [])
                for model in item["models"]:
                    if model not in merged:
                        merged.append(model)
                item["models"] = merged
                old.update(item)
                if saved_keys:
                    old["keys"] = saved_keys  # 永不覆盖已经导入的真实密钥
                updated += 1
        if args.enable_all:
            for provider in config["providers"]:
                provider["enabled"] = True
        config["providers"] = providers
        if not args.no_groups:
            have = {g.get("id") for g in existing.get("groups") or []}
            config["groups"] = list(existing.get("groups") or []) + [
                g for g in groups if g["id"] not in have
            ]
        note = backup(LR_FILE)
        atomic_write(LR_FILE, config)
        say(f"✓ 已合并进 {LR_FILE}（新增 {added}，更新 {updated}）")
        if note:
            say(f"  备份：{note}")
    elif args.out:
        note = backup(args.out)
        atomic_write(args.out, config)
        say(f"✓ 已写出 {args.out}（权限 0600）")
        if note:
            say(f"  备份：{note}")
    else:
        print(json.dumps(config, ensure_ascii=False, indent=2))
        say("· 以上是预览；加 --out <文件> 或 --merge 才会写入。")


if __name__ == "__main__":
    main()
