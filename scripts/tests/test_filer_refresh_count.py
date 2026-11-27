"""Counting rule for the Filer refresh probe (``require_refresh_listing_count``).

``click_filer_refresh`` returns how many clicks it issued; each click can
start at most one remote listing (one ``SFTP_LIST_READY`` per scandir). One
click must start exactly one listing; retries allow 1..clicks.
"""
from __future__ import annotations

import ast
import importlib.util
import sys
import types
import unittest
from pathlib import Path

try:
    import asyncssh  # noqa: F401
except ImportError:
    _stub = types.ModuleType("asyncssh")
    for _name in ("SSHServerSession", "SSHServer", "SFTPServer",
                  "SSHServerChannel", "SSHServerConnection", "SSHKey"):
        setattr(_stub, _name, type(_name, (), {}))
    sys.modules["asyncssh"] = _stub

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui_refresh", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

check = module.require_refresh_listing_count


class RefreshListingCountTest(unittest.TestCase):
    def test_single_click_single_listing_passes(self):
        check(4, 5, 1)

    def test_single_click_double_listing_raises(self):
        # The old predicate (count > before) accepted this.
        self.assertTrue(6 > 4)
        with self.assertRaisesRegex(RuntimeError, r"started 2 .* 1 click.*4 -> 6.*exactly 5"):
            check(4, 6, 1)

    def test_single_click_no_listing_raises(self):
        with self.assertRaisesRegex(RuntimeError, "started 0"):
            check(4, 4, 1)

    def test_retry_allows_one_listing_per_click(self):
        for clicks in (2, 3):
            for after in range(5, 5 + clicks):
                with self.subTest(clicks=clicks, after=after):
                    check(4, after, clicks)

    def test_retry_more_listings_than_clicks_raises(self):
        with self.assertRaisesRegex(RuntimeError, r"started 3 .* 2 click.*expected 5\.\.6"):
            check(4, 7, 2)
        with self.assertRaisesRegex(RuntimeError, "started 4"):
            check(4, 8, 3)

    def test_retry_no_listing_raises(self):
        with self.assertRaisesRegex(RuntimeError, "started 0"):
            check(4, 4, 3)

    def test_no_click_raises(self):
        with self.assertRaisesRegex(RuntimeError, "no click"):
            check(4, 5, 0)


class RefreshCallSiteTest(unittest.TestCase):
    def test_click_filer_refresh_returns_click_count(self):
        tree = ast.parse(SOURCE.read_text(encoding="utf-8"))
        fn = next(node for node in ast.walk(tree)
                  if isinstance(node, ast.AsyncFunctionDef) and node.name == "click_filer_refresh")
        returns = [node for node in ast.walk(fn) if isinstance(node, ast.Return)]
        self.assertEqual(len(returns), 1)
        self.assertEqual(ast.unparse(returns[0].value), "index + 1")

    def test_caller_checks_count_after_third_file_and_before_header_baseline(self):
        source = SOURCE.read_text(encoding="utf-8")
        clicked = source.index("refresh_clicks = await click_filer_refresh(")
        visible = source.index('"connected-files-third-visible.png"', clicked)
        checked = source.index(
            "require_refresh_listing_count(listings_before, listings_after_refresh, refresh_clicks)",
            visible,
        )
        header_baseline = source.index("listings_at_sort = events.path", checked)
        self.assertLess(clicked, visible)
        self.assertLess(visible, checked)
        self.assertLess(checked, header_baseline)

    def test_local_half_probe_absent_after_tab_and_visible_after_refresh(self):
        source = SOURCE.read_text(encoding="utf-8")
        written = source.index("(local_root / local_probe_name).write_bytes(")
        tab = source.index('"connected-files-refresh-back.png"', written)
        absent = source.index(
            "require_label_absent, tab_local_shot, local_probe_name, region=local_body,", tab)
        clicked = source.index("refresh_clicks = await click_filer_refresh(", absent)
        visible = source.index('"connected-files-refresh-local-visible.png"', clicked)
        self.assertLess(written, tab)
        self.assertLess(visible, source.index("listings_at_sort = events.path", visible))
        # One absence proof only (each costs 1-3 min of untimed OCR).
        step = source[written:visible]
        self.assertEqual(step.count("require_label_absent"), 1)


if __name__ == "__main__":
    unittest.main()
