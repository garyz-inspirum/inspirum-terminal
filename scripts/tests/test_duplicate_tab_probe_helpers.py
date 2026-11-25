"""Pure helpers behind the opt-in duplicate-tab GUI step."""
from __future__ import annotations

import importlib.util
import os
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_duptab", SOURCE)
gui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gui)


class DuplicateTabHelpersTest(unittest.TestCase):
    def test_label(self):
        self.assertEqual(gui.duplicate_tab_label("CI connected fixture"), "CI connected fixture (2)")
        self.assertEqual(gui.duplicate_tab_label("a", 3), "a (3)")

    def test_band_is_full_width_around_label(self):
        self.assertEqual(gui.tab_strip_band((40, 36, 180, 50), 1280), (0, 28, 1280, 58))
        self.assertEqual(gui.tab_strip_band((40, 3, 180, 15), 1280)[1], 0)

    def test_tokens_logged_and_switch_default_off(self):
        for token in ("echo:IN_DUPLICATE_TAB", "echo:AFTER_DUPLICATE_TAB"):
            self.assertIn(token, gui.LOGGED_ECHO_LINES)
        self.assertEqual(os.environ.get("INSPIRUM_GUI_DUPTAB") == "1", gui.DUPTAB)


if __name__ == "__main__":
    unittest.main()
