"""Selected-Filer tour oracle (tour-08 vs tour-08b) on real Light 1280x800 frames.

Fixtures are the retained tour frames: tour-08-filer.png (no selection,
Download/Rename/Edit/Delete disabled) and tour-08b-filer-selected.png
(444_GUI_DOWNLOAD_FIXTURE.bin selected, outline + enabled actions). The
negative controls edit those real frames in memory and go through the same
selected_filer_verdict the tour calls, with its default thresholds.
"""
from __future__ import annotations

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
spec = importlib.util.spec_from_file_location("iced_connected_gui_filer", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
UNSELECTED = module._read_png(FIXTURES / "tour-filer-light-1280x800-unselected.png")
SELECTED = module._read_png(FIXTURES / "tour-filer-light-1280x800-selected.png")
WIDTH, HEIGHT = 1280, 800
LABEL = (31, 628, 239, 639)  # tesseract box of 444_GUI_DOWNLOAD_FIXTURE.bin
# Same band the tour derives: (0, height - 56, width, height - 22).
ACTION_BAND = (0, HEIGHT - 56, WIDTH, HEIGHT - 22)
ROW_Y = (623, 645)  # outline rules on the selected frame
HOVER_FILL = (0xE4, 0xE4, 0xE4)  # LIGHT.hover; file_row_style hover has no border


def clone(frame):
    width, height, channels, rows = frame
    return (width, height, channels, [bytearray(row) for row in rows])


def splice_rows(base, donor, top, bottom):
    """base with rows [top, bottom) taken from donor."""
    out = clone(base)
    for y in range(top, bottom):
        out[3][y] = bytearray(donor[3][y])
    return out


def splice_box(base, donor, box):
    out = clone(base)
    channels = base[2]
    left, top, right, bottom = box
    for y in range(top, bottom):
        out[3][y][left * channels:right * channels] = donor[3][y][left * channels:right * channels]
    return out


def verdict(unselected, selected):
    return module.selected_filer_verdict(unselected, selected, LABEL, ACTION_BAND)


class SelectedFilerOracleTest(unittest.TestCase):
    def test_real_selected_frame_passes(self):
        summary = verdict(UNSELECTED, SELECTED)
        self.assertIn("outline (0, 623, 1279, 645) fill (216, 216, 216)", summary)
        for name in ("Download", "Rename", "Edit", "Delete"):
            self.assertIn(f"{name} ", summary)
        self.assertEqual(summary.count("border (191, 191, 191)->(118, 118, 118)"), 4)
        self.assertEqual(summary.count("face (240, 240, 240)->(255, 255, 255)"), 4)

    def test_paths_and_decoded_frames_agree(self):
        self.assertEqual(
            verdict(UNSELECTED, SELECTED),
            module.selected_filer_verdict(
                FIXTURES / "tour-filer-light-1280x800-unselected.png",
                FIXTURES / "tour-filer-light-1280x800-selected.png",
                LABEL, ACTION_BAND,
            ),
        )

    def test_buttons_found_in_both_states(self):
        before = module.filer_action_buttons(UNSELECTED, ACTION_BAND)
        after = module.filer_action_buttons(SELECTED, ACTION_BAND)
        boxes = [b["box"] for b in after]
        self.assertEqual(boxes, [b["box"] for b in before])
        self.assertEqual(boxes, [
            (8, 750, 109, 772), (120, 750, 268, 772), (279, 750, 396, 772),
            (407, 750, 508, 772), (519, 750, 636, 772),
        ])
        self.assertEqual(module.filer_action_enabled(before), [False, True, False, False, False])
        self.assertEqual(module.filer_action_enabled(after), [True] * 5)

    def test_unselected_action_strip_substitution_fails(self):
        # Quill's negative control: the selected frame with tour-08's strip.
        forged = splice_rows(SELECTED, UNSELECTED, ACTION_BAND[1], ACTION_BAND[3])
        # The row half alone is still a real selection...
        module.filer_row_outline(forged, LABEL)
        with self.assertRaisesRegex(
            RuntimeError, r"selected frame shows \['Download', 'Rename', 'Edit', 'Delete'\] disabled",
        ):
            verdict(UNSELECTED, forged)

    def test_each_button_is_checked_on_its_own(self):
        after = module.filer_action_buttons(SELECTED, ACTION_BAND)
        for index, name in module.FILER_SELECTION_ACTIONS.items():
            x0, top, x1, bottom = after[index]["box"]
            forged = splice_box(SELECTED, UNSELECTED, (x0 - 2, top - 1, x1 + 3, bottom + 2))
            with self.subTest(name=name):
                with self.assertRaisesRegex(
                    RuntimeError, rf"selected frame shows \['{name}'\] disabled",
                ):
                    verdict(UNSELECTED, forged)

    def test_hover_only_row_fails(self):
        # Hover = fill-only row; give it the selected action strip so only the
        # row proof can reject it.
        hovered = clone(UNSELECTED)
        channels = hovered[2]
        for y in range(ROW_Y[0], ROW_Y[1] + 1):
            row = hovered[3][y]
            for x in range(WIDTH):
                i = x * channels
                if tuple(row[i:i + 3]) == (255, 255, 255):
                    row[i:i + 3] = bytes(HOVER_FILL)
        hovered = splice_rows(hovered, SELECTED, ACTION_BAND[1], ACTION_BAND[3])
        with self.assertRaisesRegex(RuntimeError, "no selection outline rule"):
            verdict(UNSELECTED, hovered)

    def test_selected_row_without_outline_rule_fails(self):
        forged = splice_rows(SELECTED, UNSELECTED, ROW_Y[1], ROW_Y[1] + 1)
        with self.assertRaisesRegex(RuntimeError, "no selection outline rule"):
            verdict(UNSELECTED, forged)

    def test_unselected_strip_only_change_fails(self):
        # Enabled actions without a visible row outline are not a selection.
        forged = splice_rows(UNSELECTED, SELECTED, ACTION_BAND[1], ACTION_BAND[3])
        with self.assertRaisesRegex(RuntimeError, "no selection outline rule"):
            verdict(UNSELECTED, forged)

    def test_baseline_must_be_unselected(self):
        with self.assertRaisesRegex(RuntimeError, "already shows a row selection outline"):
            verdict(SELECTED, SELECTED)
        with self.assertRaisesRegex(RuntimeError, "no selection outline rule"):
            verdict(UNSELECTED, UNSELECTED)


if __name__ == "__main__":
    unittest.main()
