"""Geometry/fill helpers behind the running-forward edit/delete GUI probe.

The probe clicks Save by its primary fill and the list's remove button by
dialog geometry, then requires the footer notice and the persisted profile
file. These tests pin each helper on the real populated 1280x800 frame and
show it finds nothing on a frame without the Tunnel Manager.
"""
from __future__ import annotations

import importlib.util
import json
import shutil
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_edit", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
EDITOR = FIXTURES / "connected-tunnels-editor-selected-alpha.png"
CLOSED = FIXTURES / "connected-tunnels-closed.png"
HEADING = (249, 151, 367, 159)


@unittest.skipUnless(EDITOR.is_file() and CLOSED.is_file(), "tunnel fixtures missing")
class TunnelFrameHelpersTest(unittest.TestCase):
    def test_save_fill_found_on_manager_frame(self):
        (x, y), box = module.tunnel_save_button(EDITOR, HEADING, 1280)
        # Measured Save button 984..1035 x 661..684.
        self.assertEqual(box, (984, 661, 1035, 684))
        self.assertEqual((x, y), (1009, 672))

    def test_save_fill_absent_without_manager(self):
        self.assertIsNone(module.tunnel_save_button(CLOSED, HEADING, 1280))

    def test_local_kind_toggle_is_not_save(self):
        # "Local" is also primary-filled; the scan band starts below it.
        (_xy, box) = module.tunnel_save_button(EDITOR, HEADING, 1280)
        self.assertGreater(box[1], HEADING[1] + 97)

    def test_remove_centre_hits_the_minus_glyph(self):
        x, y = module.tunnel_remove_centre(HEADING, 1280)
        self.assertEqual((x, y), (299, 556))
        width, _h, channels, rows = module._read_png(EDITOR)
        glyph = [
            (gx, gy) for gy in range(y - 4, y + 5) for gx in range(x - 6, x + 7)
            if rows[gy][gx * channels] > 100
        ]
        self.assertTrue(glyph, "no light minus glyph near the remove target")
        self.assertLess(x, 545, "remove target must be inside the list card")

    def test_listen_input_centre_inside_input(self):
        x, y = module.listen_input_centre(HEADING, 1280)
        self.assertTrue(719 <= x <= 1023 and 287 <= y <= 310, (x, y))

    def test_notice_region_left_of_cancel(self):
        _xy, box = module.tunnel_save_button(EDITOR, HEADING, 1280)
        region = module.tunnel_notice_region(box, 1280)
        self.assertLess(region[2], 909, "notice region must not reach Cancel")
        self.assertLessEqual(region[1], 661)
        self.assertGreaterEqual(region[3], 684)


LIGHT_EMPTY = FIXTURES / "tour-tunnel-manager-light-empty.png"


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(LIGHT_EMPTY.is_file(), "light tour fixture missing")
class LightLocateTest(unittest.TestCase):
    """Real LIGHT 1280x800 Tunnel Manager (unpopulated tour capture); the
    dark pipeline missed its heading in that run (OCR miss after 22 s)."""

    def test_heading_found_inside_region(self):
        region = (0, 42, 1280, 400)
        box = module.light_locate(LIGHT_EMPTY, "Tunnel Manager", region=region)
        self.assertIsNotNone(module._bounds_inside(box, region))
        left, dialog = module.tunnel_dialog_left(1280)
        self.assertTrue(left <= box[0] < left + 60, box)

    def test_absent_forward_label_fails(self):
        # Negative control: the unpopulated manager has no saved forward.
        for label in ("Alpha forward", "Bravo forward"):
            with self.assertRaises(RuntimeError):
                module.light_locate(LIGHT_EMPTY, label, region=(0, 42, 1280, 800))

    def test_region_is_a_hard_box(self):
        with self.assertRaises(RuntimeError):
            module.light_locate(LIGHT_EMPTY, "Tunnel Manager", region=(640, 42, 1280, 800))

    def test_minimum_window_geometry(self):
        left, dialog = module.tunnel_dialog_left(960)
        self.assertEqual((left, dialog), (60, 840))
        row = module.listen_port_row_region((89, 40, 207, 48), 960)
        self.assertTrue(row[0] > left + 325 and row[2] <= 960, row)


class SavedForwardsTest(unittest.TestCase):
    def write(self, payload) -> Path:
        path = Path(tempfile.mkstemp(suffix=".json")[1])
        self.addCleanup(path.unlink, missing_ok=True)
        path.write_text(json.dumps(payload), encoding="utf-8")
        return path

    def test_reads_named_profile(self):
        path = self.write([
            {"name": "CI connected fixture"},
            {"name": "CI tunnel fixture", "ssh": {"local_forwards": ["a", "b"]}},
        ])
        self.assertEqual(module.saved_local_forwards(path, "CI tunnel fixture"), ["a", "b"])
        self.assertEqual(module.saved_local_forwards(path, "CI connected fixture"), [])

    def test_wrapped_sessions_and_missing_profile(self):
        path = self.write({"sessions": [{"name": "x", "ssh": {"local_forwards": ["c"]}}]})
        self.assertEqual(module.saved_local_forwards(path, "x"), ["c"])
        with self.assertRaises(RuntimeError):
            module.saved_local_forwards(path, "CI tunnel fixture")


if __name__ == "__main__":
    unittest.main()
