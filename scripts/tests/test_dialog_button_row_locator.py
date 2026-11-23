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


# connected-profile-spec-cancel.png: CI frame from run 45177442798 (4cba4b6),
# 1280x800 dark. The refused Local forward edit: one danger line above the
# same button row, outlines drawn brighter. CI (ubuntu-latest, tesseract
# 5.5.0) raised "visible label not found: Save only" here
# (scripts/test-iced-connected-gui.py:5908, cancel()): the whole band and its
# rule-free interior both OCR as nothing. tesseract 5.3.4 (the local GUI
# image) still reads the interior, so the locate tests below are red on
# 4cba4b6 only under tesseract 5.5.0.
SPEC_CANCEL = FIXTURES / "connected-profile-spec-cancel.png"
CANCEL_INTERIOR = (626, 689)
SAVE_ONLY_INTERIOR = (701, 787)


def erased_save_only_glyphs(source: Path) -> Path:
    """``source`` with the Save only label painted out, borders kept."""
    width, height, channels, rows = module._read_png(source)
    out = bytearray()
    x1, y1, x2, y2 = SAVE_ONLY_BUTTON
    for y in range(height):
        row = rows[y]
        for x in range(width):
            base = x * channels
            if x1 + 2 <= x < x2 - 2 and y1 + 2 <= y < y2 - 2:
                out += bytes(BACKGROUND)
            else:
                out += bytes(row[base:base + 3])
    fd, name = tempfile.mkstemp(suffix=".png")
    os.close(fd)
    module._write_png(Path(name), width, height, bytes(out))
    return Path(name)


class OutlineButtonInteriorsTest(unittest.TestCase):
    """Per-button crops of an outlined row; pure pixel geometry, no OCR."""

    def buttons(self, path, span=BUTTON_ROW_SPAN, band=(724, 746)):
        width, height, channels, rows = module._read_png(path)
        thresh = module._text_threshold(width, height, channels, rows)
        interior = module._outline_interior(rows, channels, thresh, *span, *band)
        self.assertIsNotNone(interior)
        return module._outline_button_interiors(rows, channels, thresh, *span, *interior)

    def test_spec_cancel_frame_splits_cancel_and_save_only(self):
        self.assertEqual(self.buttons(SPEC_CANCEL), [
            (CANCEL_INTERIOR[0], 726, CANCEL_INTERIOR[1], 746),
            (SAVE_ONLY_INTERIOR[0], 726, SAVE_ONLY_INTERIOR[1], 746),
        ])

    def test_rename_frames_split_the_same_buttons(self):
        self.assertEqual(self.buttons(CANCEL), [(626, 726, 689, 746), (701, 726, 787, 746)])
        self.assertEqual(self.buttons(BEFORE), [(626, 725, 689, 746), (701, 725, 787, 746)])

    def test_crops_stay_inside_the_button_borders(self):
        x1, y1, x2, y2 = SAVE_ONLY_BUTTON
        crop = self.buttons(SPEC_CANCEL)[1]
        self.assertTrue(x1 < crop[0] < crop[2] < x2 - 1 and y1 < crop[1] < crop[3] < y2)

    def test_filled_primary_button_yields_no_crop(self):
        # Save changes fill 797..910 alone: one wide lit run, no border pair.
        width, height, channels, rows = module._read_png(SPEC_CANCEL)
        thresh = module._text_threshold(width, height, channels, rows)
        self.assertEqual(
            module._outline_button_interiors(rows, channels, thresh, 792, 911, 725, 745), [],
        )
        # Cut before Cancel's right border (x 690): one border, no crop.
        self.assertEqual(
            module._outline_button_interiors(rows, channels, thresh, 624, 690, 725, 745), [],
        )
        # Save only's border pair, then the fill: only Save only is a crop.
        self.assertEqual(
            module._outline_button_interiors(rows, channels, thresh, 693, 911, 725, 745),
            [(SAVE_ONLY_INTERIOR[0], 726, SAVE_ONLY_INTERIOR[1], 746)],
        )

    def test_plain_text_band_yields_no_crop(self):
        width, height, channels, rows = module._read_png(SPEC_CANCEL)
        thresh = module._text_threshold(width, height, channels, rows)
        # Danger line "... changing it." (y 705..711) has no vertical rules.
        self.assertEqual(
            module._outline_button_interiors(rows, channels, thresh, 354, 771, 705, 711), [],
        )


@unittest.skipUnless(HAS_TESSERACT, "tesseract is not installed")
class SpecCancelSaveOnlyLocatorTest(unittest.TestCase):
    """Default budget and every stage, exactly as cancel() calls it."""

    def assertInsideButton(self, box):
        x1, y1, x2, y2 = SAVE_ONLY_BUTTON
        self.assertTrue(x1 <= box[0] < box[2] <= x2 and y1 <= box[1] < box[3] <= y2,
                        f"{box} not inside Save only button {SAVE_ONLY_BUTTON}")

    def test_after_refused_spec_edit_frame(self):
        self.assertInsideButton(
            module.locate_visible_label(SPEC_CANCEL, "Save only", region=DIALOG))

    def test_spec_frame_erased_button_is_a_miss(self):
        path = erased_save_only(SPEC_CANCEL)
        self.addCleanup(path.unlink)
        with self.assertRaises(RuntimeError):
            module.locate_visible_label(path, "Save only", region=DIALOG)

    def test_spec_frame_outline_without_label_is_a_miss(self):
        # Borders intact, label gone: the per-button crop must not invent it.
        path = erased_save_only_glyphs(SPEC_CANCEL)
        self.addCleanup(path.unlink)
        with self.assertRaises(RuntimeError):
            module.locate_visible_label(path, "Save only", region=DIALOG)

    def test_spec_frame_save_only_outside_region_is_a_miss(self):
        # Region ends above the button row: no box from the row may escape it.
        with self.assertRaises(RuntimeError):
            module.locate_visible_label(
                SPEC_CANCEL, "Save only", region=(DIALOG[0], DIALOG[1], DIALOG[2], 720))


if __name__ == "__main__":
    unittest.main()
