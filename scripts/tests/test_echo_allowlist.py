"""Every literal NATIVE_ECHO:<TOKEN> the GUI test waits for must be fixture-logged."""
from pathlib import Path
import re
import unittest
import importlib.util

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui_echo_allowlist", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class EchoAllowlistTest(unittest.TestCase):
    def test_waited_echo_tokens_are_logged_by_fixture(self):
        text = SOURCE.read_text(encoding="utf-8")
        waited = set(re.findall(r"NATIVE_ECHO:([A-Z][A-Z0-9_]*)", text))
        self.assertIn("AFTER_TUNNEL_TOGGLES", waited)
        missing = sorted(t for t in waited if f"echo:{t}" not in module.LOGGED_ECHO_LINES)
        self.assertEqual(missing, [], "NATIVE_ECHO tokens awaited but not in LOGGED_ECHO_LINES")

    def test_typed_after_probes_are_logged_by_fixture(self):
        text = SOURCE.read_text(encoding="utf-8")
        typed = set(re.findall(r'"echo:(AFTER_[A-Z0-9_]+)"', text))
        missing = sorted(t for t in typed if f"echo:{t}" not in module.LOGGED_ECHO_LINES)
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
