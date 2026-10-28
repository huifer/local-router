#!/usr/bin/env python3
"""把 magpie 里的 provider 真实密钥导进 local-router。

magpie 的磁盘配置（~/.config/magpie/providers.json）里，密钥是不可逆的
HMAC 占位符（`{{API_KEY_xxx}}`）——真实值只存在于运行中的 magpie 进程内存。
magpie 自己的 GUI 有个接口能交出真实密钥（编辑器的「显示」按钮用的就是它），
但那个接口只在它把网页开在本机时才可达：

    magpie → Settings → 网页 → 选「此电脑」

之后本脚本读 ~/.config/magpie/web-page.json 里的 key，向 magpie 的
`POST /api/provider/key` 逐个问真实密钥，写进 local-router 的
config.json 里同名 provider 的 keys。

用法：
    python3 scripts/import_magpie_keys.py                 # 导入已配好的四家
    python3 scripts/import_magpie_keys.py minimax-cn zhipu
    python3 scripts/import_magpie_keys.py --dry-run       # 只看，不写
"""

import json
import os
import pathlib
import sys
import urllib.error
import urllib.request

MAGPIE_DIR = pathlib.Path.home() / ".config" / "magpie"
WEB_PAGE = MAGPIE_DIR / "web-page.json"
LR_CONFIG = pathlib.Path.home() / ".config" / "local-router" / "config.json"

# local-router 的 provider id → magpie 的 provider id
PAIRS = {
    "minimax-cn": "minimax-cn",
    "zhipu": "zhipu",
    "qnaigc": "qnaigc",
    "codex-2": "codex-2",
}


def fail(message: str) -> None:
    print(f"× {message}", file=sys.stderr)
    raise SystemExit(1)


def page_address() -> tuple[str, int, str]:
    """magpie 的网页开在哪、key 是多少。"""
    if not WEB_PAGE.exists():
        fail(
            f"{WEB_PAGE} 不存在——magpie 的网页没开。\n"
            "  在 magpie 里：Settings → 网页 → 选「此电脑」，然后再跑一次。"
        )
    conf = json.loads(WEB_PAGE.read_text())
    if not conf.get("mode"):
        fail(
            "magpie 的网页是关的。在 magpie 里：Settings → 网页 → 选「此电脑」。"
        )
    port = int(conf.get("port") or 3430)
    host = "127.0.0.1" if conf["mode"] == "local" else "0.0.0.0"
    return host, port, conf.get("key", "")


def magpie_key(base: str, provider_id: str, key: str) -> str:
    """问 magpie 要某个 provider 的真实密钥。"""
    url = f"{base}/api/provider/key"
    body = json.dumps({"id": provider_id}).encode()
    request = urllib.request.Request(
        url,
        data=body,
        headers={
            "content-type": "application/json",
            "cookie": f"magpie_web_{port_of(base)}={key}",
        },
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.loads(response.read()).get("key", "")


def port_of(base: str) -> int:
    return int(base.rsplit(":", 1)[1])


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    dry = "--dry-run" in sys.argv
    hosts = args or list(PAIRS)

    host, port, key = page_address()
    base = f"http://{host}:{port}"
    print(f"· magpie 的网页在 {base}（key 已在 web-page.json 里）")

    found: dict[str, str] = {}
    for provider_id in hosts:
        magpie_id = PAIRS.get(provider_id, provider_id)
        try:
            value = magpie_key(base, magpie_id, key)
        except urllib.error.HTTPError as err:
            fail(f"{magpie_id}: magpie 拒绝了请求（HTTP {err.code}）")
        except urllib.error.URLError as err:
            fail(f"{magpie_id}: 连不上 magpie 的网页（{err.reason}）")
        if not value or value.startswith("{{"):
            print(f"  ! {magpie_id}: 拿到的还是占位符（{value}）——magpie 那边可能没保存真实 key")
            continue
        found[provider_id] = value
        print(f"  ✓ {magpie_id}: 拿到真实密钥（{len(value)} 字符）")

    if not found:
        fail("没有拿到任何真实密钥")

    if dry:
        print("· --dry-run：没有写入")
        return

    config = json.loads(LR_CONFIG.read_text())
    providers = config.get("providers", [])
    written = 0
    for provider in providers:
        value = found.get(provider.get("id", ""))
        if not value:
            continue
        keys = provider.setdefault("keys", [])
        for existing in keys:
            if existing.get("key") == value:
                break
        else:
            keys.append(
                {
                    "id": "magpie-" + provider["id"],
                    "key": value,
                    "enabled": True,
                    "label": "Magpie 导入",
                }
            )
            written += 1
    LR_CONFIG.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n")
    os.chmod(LR_CONFIG, 0o666)
    print(f"· 已写入 {LR_CONFIG}（新增 {written} 个密钥，{len(found)} 个来源）")
    print("· 改动会在 local-router 下一次刷新配置时生效。")


if __name__ == "__main__":
    main()
