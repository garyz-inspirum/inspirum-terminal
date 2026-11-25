"""Pure helpers behind the opt-in Filer "Open in External Editor" GUI step."""
from __future__ import annotations

import ast
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_extedit", SOURCE)
gui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gui)


class FakeEditorTest(unittest.TestCase):
    def test_script_logs_argv_and_appends_marker_to_last_argument(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            editor = root / "fake editor.py"
            log = root / "argv.log"
            editor.write_text(
                gui.fake_editor_script(sys.executable, log, gui.EXTEDIT_MARKER),
                encoding="utf-8",
            )
            editor.chmod(0o755)
            first = root / "decoy.bin"
            target = root / "444_GUI_DOWNLOAD_FIXTURE.bin"
            first.write_bytes(b"decoy")
            target.write_bytes(b"\x00\xffbody")
            subprocess.run([str(editor), str(first), str(target)], check=True, timeout=20)
            self.assertEqual(target.read_bytes(), b"\x00\xffbody" + gui.EXTEDIT_MARKER)
            self.assertEqual(first.read_bytes(), b"decoy")
            argv = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(len(argv), 1)
            self.assertEqual(Path(argv[0][-1]).name, target.name)

    def test_script_starts_with_shebang(self):
        text = gui.fake_editor_script("/usr/bin/python3", Path("/tmp/x.log"), b"M\n")
        self.assertTrue(text.startswith("#!/usr/bin/python3\n"))
        ast.parse(text)


class UiStatePayloadTest(unittest.TestCase):
    def test_partial_payload_only_sets_quoted_editor(self):
        payload = gui.external_editor_ui_state(Path("/tmp/a b/fake-editor.py"))
        self.assertEqual(payload, {"external_editor": '"/tmp/a b/fake-editor.py"'})
        self.assertNotIn("{file}", payload["external_editor"])
        self.assertNotIn("theme", payload)


class VerdictAndRegionTest(unittest.TestCase):
    def test_bytes_verdict(self):
        m = gui.EXTEDIT_MARKER
        self.assertIsNone(gui.extedit_bytes_verdict(b"abc", b"abc" + m, m))
        self.assertIsNotNone(gui.extedit_bytes_verdict(b"abc", b"abc", m))
        self.assertIsNotNone(gui.extedit_bytes_verdict(b"abc", b"abX" + m, m))
        self.assertIsNotNone(gui.extedit_bytes_verdict(b"abc", b"ab" + m, m))

    def test_region_fits_seven_rows_either_side_and_clamps(self):
        x1, y1, x2, y2 = gui.extedit_menu_region(300, 700, 1280, 800)
        self.assertGreaterEqual(700 - y1, 7 * 24 + 2)
        self.assertEqual(y2, 800)
        self.assertGreaterEqual(x2 - 300, 400)
        x1, y1, _, y2 = gui.extedit_menu_region(5, 5, 1280, 800)
        self.assertEqual((x1, y1), (0, 0))
        self.assertGreaterEqual(y2 - 5, 7 * 24 + 2)


class AllowlistTest(unittest.TestCase):
    def test_new_tokens_are_logged(self):
        self.assertIn("echo:AFTER_EXTERNAL_EDIT", gui.LOGGED_ECHO_LINES)

    def test_switch_defaults_off(self):
        self.assertEqual(os.environ.get("INSPIRUM_GUI_EXTEDIT") == "1", gui.EXTEDIT)


if __name__ == "__main__":
    unittest.main()
