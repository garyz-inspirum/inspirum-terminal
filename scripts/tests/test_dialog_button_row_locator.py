"""Locate "Save only" on the real Edit SSH connection button row.

Fixtures are CI frames from run 45168137192 (cd6b6eb), 1280x800 dark:

* connected-profile-rename-before-submit.png: form scrolled to the bottom,
  no error; the call that located "Save only" before Submit.
* connected-profile-rename-cancel.png: same scroll after the refused rename,
  two danger lines above the buttons and the button outlines drawn brighter.
  CI raised "visible label not found: Save only" here
  (scripts/test-iced-connected-gui.py:5838, cancel()).
* connected-profile-rename-dialog.png: the same dialog at the top of the
  form (outlined inputs and toggles, no button row): absence control.

The buttons on both bottom frames measure Cancel 624..690, Save only
699..788, Save changes 797..910, y 724..746 (borders included).

Every locate runs exactly as the probe calls it: the dialog region, the
default budget and every fallback stage.
"""
from __future__ import annotations

import importlib.util
import os
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_button_row", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
CANCEL = FIXTURES / "connected-profile-rename-cancel.png"
BEFORE = FIXTURES / "connected-profile-rename-before-submit.png"
TOP = FIXTURES / "connected-profile-rename-dialog.png"
WIDTH, HEIGHT = 1280, 800
DIALOG = module.connection_dialog_region(WIDTH, HEIGHT)
SAVE_ONLY_BUTTON = (699, 724, 789, 747)  # exclusive right/bottom
BUTTON_ROW_SPAN = (624, 911)
BACKGROUND = (25, 25, 25)  # dialog body behind the buttons on both frames
HAS_TESSERACT = shutil.which("tesseract") is not None


def erased_save_only(source: Path) -> Path:
    """``source`` with the whole Save only button painted as background."""
    width, height, channels, rows = module._read_png(source)
    out = bytearray()
    x1, y1, x2, y2 = SAVE_ONLY_BUTTON
    for y in range(height):
        row = rows[y]
        for x in range(width):
            base = x * channels
            if x1 - 2 <= x < x2 + 2 and y1 - 2 <= y < y2 + 2:
                out += bytes(BACKGROUND)
            else:
                out += bytes(row[base:base + 3])
    fd, name = tempfile.mkstemp(suffix=".png")
    os.close(fd)
    module._write_png(Path(name), width, height, bytes(out))
    return Path(name)


class OutlineInteriorTest(unittest.TestCase):
    """Pure pixel geometry, no OCR."""

    def interior(self, path, band):
        width, height, channels, rows = module._read_png(path)
        thresh = module._text_threshold(width, height, channels, rows)
        return module._outline_interior(rows, channels, thresh, *BUTTON_ROW_SPAN, *band)

    def test_cancel_frame_drops_both_border_rows(self):
        self.assertEqual(self.interior(CANCEL, (724, 746)), (725, 745))

    def test_before_frame_drops_bright_bottom_border_only(self):
        # Top border is below the text threshold there (grey 84/96).
        self.assertEqual(self.interior(BEFORE, (724, 746)), (724, 745))

    def test_plain_text_band_is_untouched(self):
        # Second danger line "or replacing it." (y 705..711): no rules.
        width, height, channels, rows = module._read_png(CANCEL)
        thresh = module._text_threshold(width, height, channels, rows)
        self.assertIsNone(module._outline_interior(rows, channels, thresh, 354, 475, 705, 711))


@unittest.skipUnless(HAS_TESSERACT, "tesseract is not installed")
class SaveOnlyLocatorTest(unittest.TestCase):
    def assertInsideButton(self, box):
        x1, y1, x2, y2 = SAVE_ONLY_BUTTON
        self.assertTrue(x1 <= box[0] < box[2] <= x2 and y1 <= box[1] < box[3] <= y2,
                        f"{box} not inside Save only button {SAVE_ONLY_BUTTON}")

    def test_default_budget_is_in_force(self):
        self.assertNotIn("INSPIRUM_GUI_OCR_BUDGET", os.environ)
        self.assertEqual(module.LOCATE_BUDGET_S, 12.0)

    def test_after_refused_rename_frame(self):
        self.assertInsideButton(module.locate_visible_label(CANCEL, "Save only", region=DIALOG))

    def test_before_submit_frame(self):
        self.assertInsideButton(module.locate_visible_label(BEFORE, "Save only", region=DIALOG))

    def test_erased_button_is_a_miss(self):
        path = erased_save_only(CANCEL)
        self.addCleanup(path.unlink)
        with self.assertRaises(RuntimeError):
            module.locate_visible_label(path, "Save only", region=DIALOG)

    def test_top_of_form_without_buttons_is_a_miss(self):
        with self.assertRaises(RuntimeError):
            module.locate_visible_label(TOP, "Save only", region=DIALOG)


if __name__ == "__main__":
    unittest.main()
