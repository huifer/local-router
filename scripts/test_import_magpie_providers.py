#!/usr/bin/env python3
"""import_magpie_providers.py 的关键转换规则回归测试。

跑法：python3 -m unittest discover -s scripts -p 'test_*.py'
"""

import unittest

from import_magpie_providers import (
    bare_conflicts,
    convert_groups,
    convert_provider,
    split_endpoint,
)

# 拼出来而不是写字面量：这个仓库不该出现任何看起来像真实凭据的串。
# 用单字符拼出占位符：源码里不出现任何形如真实凭据的整串。
PLACEHOLDER = "".join(
    ["{", "{", "A", "P", "I", "_", "K", "E", "Y", "_",
     "t", "e", "s", "t", "1", "2", "3", "}", "}"]
)


def magpie_provider(**overrides):
    raw = {
        "id": "demo",
        "name": "Demo",
        "key": "sk-live",
        "chat": "https://demo.example/v1",
        "models": ["demo-model"],
    }
    raw.update(overrides)
    return raw


class SplitEndpointTest(unittest.TestCase):
    def test_standard_v1_base_needs_no_override(self):
        base, path = split_endpoint("https://host/v1", "chat")
        self.assertEqual(base, "https://host/v1")
        self.assertIsNone(path)

    def test_host_only_lets_url_for_add_v1(self):
        base, path = split_endpoint("https://free.example", "chat")
        self.assertEqual(base, "https://free.example")
        self.assertIsNone(path)

    def test_full_endpoint_is_split_into_base_and_chat_path(self):
        base, path = split_endpoint(
            "https://host/endpoints/v1/chat/completions", "chat"
        )
        self.assertEqual(base, "https://host/endpoints/v1")
        self.assertEqual(path, "/chat/completions")

    def test_custom_path_is_pinned_so_url_for_does_not_insert_v1(self):
        base, path = split_endpoint(
            "https://open.bigmodel.cn/api/coding/paas/v4", "chat"
        )
        self.assertEqual(base, "https://open.bigmodel.cn/api/coding/paas/v4")
        self.assertEqual(path, "/chat/completions")

    def test_gemini_never_gets_a_chat_path(self):
        _, path = split_endpoint("https://host/inference/go", "gemini")
        self.assertIsNone(path)


class ProviderTest(unittest.TestCase):
    def test_chat_is_default_protocol_and_key_becomes_a_key_entry(self):
        p = convert_provider(magpie_provider(), {}, [])
        self.assertEqual(p["protocol"], "openai_chat")
        self.assertEqual(p["base_url"], "https://demo.example/v1")
        self.assertEqual(len(p["keys"]), 1)
        self.assertEqual(p["keys"][0]["key"], "sk-live")
        self.assertTrue(p["keys"][0]["enabled"])
        self.assertNotIn("chat_path", p)

    def test_responses_base_api_becomes_openai_responses(self):
        p = convert_provider(
            magpie_provider(
                baseAPI="responses",
                chat=None,
                responses="https://codex.example/v1",
            ),
            {},
            [],
        )
        self.assertEqual(p["protocol"], "openai_responses")
        self.assertEqual(p["base_url"], "https://codex.example/v1")

    def test_other_protocol_urls_are_kept_as_fallbacks(self):
        p = convert_provider(
            magpie_provider(anthropic="https://demo.example/anthropic"), {}, []
        )
        self.assertEqual(p["anthropic_base_url"], "https://demo.example/anthropic")
        self.assertEqual(p["base_url"], "https://demo.example/v1")

    def test_off_maps_to_disabled(self):
        p = convert_provider(magpie_provider(off=True), {}, [])
        self.assertFalse(p["enabled"])

    def test_extra_accounts_keep_their_off_flag(self):
        p = convert_provider(
            magpie_provider(keys=[{"name": "二号", "key": "sk-2"},
                                  {"key": "sk-3", "off": True}]),
            {},
            [],
        )
        self.assertEqual([k["enabled"] for k in p["keys"]], [True, True, False])
        self.assertEqual(p["keys"][1]["label"], "二号")

    def test_placeholder_is_dropped_unless_a_real_key_is_supplied(self):
        warnings = []
        p = convert_provider(magpie_provider(key=PLACEHOLDER), {}, warnings)
        self.assertEqual(p["keys"], [])
        self.assertTrue(any("占位符" in w for w in warnings))

        p = convert_provider(
            magpie_provider(key=PLACEHOLDER), {"demo": "sk-real"}, []
        )
        self.assertEqual(p["keys"][0]["key"], "sk-real")

    def test_keep_placeholders_writes_them_verbatim(self):
        p = convert_provider(
            magpie_provider(key=PLACEHOLDER), {}, [], keep_placeholders=True
        )
        self.assertEqual(p["keys"][0]["key"], PLACEHOLDER)

    def test_session_provider_without_url_is_not_converted(self):
        self.assertIsNone(
            convert_provider({"id": "workbuddy", "models": ["hy4free"]}, {}, [])
        )


class GroupTest(unittest.TestCase):
    def test_members_of_missing_providers_are_dropped(self):
        warnings = []
        groups = convert_groups(
            [{
                "id": "haochi",
                "name": "自用",
                "routing": "rotate",
                "members": ["demo/model-a", "workbuddy/hy4free", "group/inner"],
                "off": ["demo/model-b"],
            }],
            {"demo", "group"},
            warnings,
        )
        self.assertEqual(groups[0]["models"], ["demo/model-a", "group/inner"])
        self.assertEqual(groups[0]["routing"], "rotate")

    def test_disabled_groups_are_skipped(self):
        self.assertEqual(
            convert_groups([{"id": "g", "disabled": True, "members": []}], set(), []),
            [],
        )

    def test_empty_group_is_skipped(self):
        self.assertEqual(
            convert_groups([{"id": "g", "members": ["ghost/x"]}], set(), []), []
        )


class BareConflictTest(unittest.TestCase):
    def test_only_names_that_were_unique_are_reported(self):
        existing = [{"id": "a", "enabled": True, "models": ["solo", "shared"]},
                    {"id": "b", "enabled": True, "models": ["shared"]}]
        incoming = [{"id": "c", "enabled": True, "models": ["solo", "shared", "new"]}]
        found = bare_conflicts(existing, incoming)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0][0], "solo")
        self.assertEqual(found[0][2], ["c"])

    def test_disabled_provider_does_not_count(self):
        existing = [{"id": "a", "enabled": False, "models": ["x"]}]
        incoming = [{"id": "b", "enabled": True, "models": ["x"]}]
        self.assertEqual(bare_conflicts(existing, incoming), [])


if __name__ == "__main__":
    unittest.main()
