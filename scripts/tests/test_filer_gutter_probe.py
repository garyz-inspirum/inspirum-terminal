"""Pure helpers of the opt-in Filer gutter probe (INSPIRUM_GUI_FILER_GUTTER_PROBE).

Synthetic PIL frames and pure verdict inputs only: no GUI, no tesseract.
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
    for _name in (
        "SSHServerSession",
        "SSHServer",
        "SFTPServer",
        "SSHServerChannel",
        "SSHServerConnection",
        "SSHKey",
    ):
        setattr(_stub, _name, type(_name, (), {}))
    sys.modules["asyncssh"] = _stub


SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

try:
    from PIL import Image, ImageDraw
except ImportError:  # pragma: no cover - CI installs Pillow for scripts/tests
    Image = None

LIST = module.DARK_LIST_RGB
RAIL = module.DARK_SCROLL_RAIL_RGB
THUMB = module.DARK_SCROLLER_RGB
TEXT = (0xBF, 0xBF, 0xBF)
HALF = (0, 0, 600, 200)  # a 600 px local half
THUMB_LEFT = 590  # 10 px thumb against the half's right edge


def frame(*, text_right=None, thumb=True, thumb_top=120, thumb_bottom=160, text_over_thumb=False):
    image = Image.new("RGB", (HALF[2], HALF[3]), LIST)
    draw = ImageDraw.Draw(image)
    if text_right is not None:
        # Glyph-like strokes in the row band y 100..112, ending at text_right.
        for x in range(440, text_right + 1, 3):
            draw.line([(x, 102), (x, 110)], fill=TEXT)
        draw.line([(text_right, 100), (text_right, 112)], fill=TEXT)
    if thumb:
        draw.rectangle([THUMB_LEFT, 0, HALF[2] - 1, HALF[3] - 1], fill=RAIL)
        if thumb_bottom > thumb_top:
            draw.rectangle([THUMB_LEFT, thumb_top, HALF[2] - 1, thumb_bottom - 1], fill=THUMB)
    if text_over_thumb and text_right is not None:
        draw.line([(text_right, 100), (text_right, 112)], fill=TEXT)
    return image


class WindowWidthForFilerHalf(unittest.TestCase):
    def test_1280_window_639_half_to_600(self):
        self.assertEqual(module.window_width_for_filer_half(1280, 639, 600), 1202)

    def test_identity(self):
        self.assertEqual(module.window_width_for_filer_half(1202, 600, 600), 1202)

    def test_grows(self):
        self.assertEqual(module.window_width_for_filer_half(1202, 600, 639), 1280)

    def test_name_width_at_600(self):
        self.assertEqual(module.filer_name_column_width(600), 338)
        self.assertEqual(
            338 + 1 + module.FILER_COL_SIZE + 1 + module.FILER_COL_TIME + 1, 600,
        )
        self.assertEqual(module.filer_name_column_width(400), module.FILER_COL_NAME_MIN)
        self.assertEqual(module.filer_name_column_width(900), module.FILER_COL_NAME_MAX)


@unittest.skipIf(Image is None, "Pillow not installed")
class TextRightEdge(unittest.TestCase):
    def test_text_block_end(self):
        image = frame(text_right=575, thumb=False)
        self.assertEqual(module.text_right_edge(image, (433, 98, 600, 115)), 575)

    def test_no_text(self):
        image = frame(text_right=None, thumb=False)
        self.assertIsNone(module.text_right_edge(image, (433, 98, 600, 115)))

    def test_scrollbar_is_not_text(self):
        image = frame(text_right=575, thumb=True, thumb_top=90, thumb_bottom=130)
        self.assertEqual(module.text_right_edge(image, (433, 98, 600, 115)), 575)

    def test_accepts_path(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "frame.png"
            frame(text_right=500, thumb=False).save(path)
            self.assertEqual(module.text_right_edge(path, (433, 98, 600, 115)), 500)


@unittest.skipIf(Image is None, "Pillow not installed")
class VerticalThumb(unittest.TestCase):
    def test_thumb_present(self):
        image = frame(text_right=575)
        self.assertEqual(module.vertical_thumb_left_edge(image, HALF), THUMB_LEFT)
        self.assertEqual(module.vertical_thumb_box(image, HALF), (THUMB_LEFT, 120, 600, 160))

    def test_no_thumb(self):
        self.assertIsNone(module.vertical_thumb_left_edge(frame(text_right=575, thumb=False), HALF))

    def test_rail_alone_is_no_thumb(self):
        image = frame(thumb=True, thumb_top=0, thumb_bottom=0)
        self.assertIsNone(module.vertical_thumb_left_edge(image, HALF))

    def test_short_thumb_needs_smaller_min_run(self):
        image = frame(thumb_top=180, thumb_bottom=193)
        self.assertIsNone(module.vertical_thumb_left_edge(image, HALF))
        self.assertEqual(module.vertical_thumb_left_edge(image, HALF, min_run=6), THUMB_LEFT)

    def test_text_clearing_thumb_has_positive_gap(self):
        image = frame(text_right=575, thumb_top=90, thumb_bottom=130)
        thumb_left = module.vertical_thumb_left_edge(image, HALF)
        right = module.text_right_edge(image, (433, 98, 600, 115))
        # Columns 576..589 are empty.
        self.assertEqual(module.gutter_clear_columns(right, thumb_left), 14)

    def test_text_overlapping_thumb_gives_non_positive_gap(self):
        for text_right in (THUMB_LEFT, THUMB_LEFT + 4):
            image = frame(text_right=text_right, thumb_top=60, thumb_bottom=160,
                          text_over_thumb=True)
            thumb_left = module.vertical_thumb_left_edge(image, HALF)
            right = module.text_right_edge(image, (433, 98, 600, 115))
            self.assertEqual(thumb_left, THUMB_LEFT)
            self.assertLess(module.gutter_clear_columns(right, thumb_left), 0)


class GutterClearColumns(unittest.TestCase):
    def test_counts_empty_columns_not_distance(self):
        # it13 run4 frame: ink ends at 586, thumb starts at 590 (587..589 clear).
        self.assertEqual(module.gutter_clear_columns(586, 590), 3)
        # Ink in the column immediately left of the thumb: nothing clear.
        self.assertEqual(module.gutter_clear_columns(589, 590), 0)
        # Ink in the thumb's first column.
        self.assertEqual(module.gutter_clear_columns(590, 590), -1)


@unittest.skipIf(Image is None, "Pillow not installed")
class GutterAdjacentInk(unittest.TestCase):
    """Measured end to end with the probe's own helpers (Quill, PR #154: a
    timestamp shifted until its ink touched the thumb still OCR'd exactly
    and passed when the gap was thumb_left - right_edge)."""

    EXPECTED = "2026/11/28 00:09:58"
    BAND = (433, 98, 600, 115)

    def measured_verdict(self, text_right):
        image = frame(text_right=text_right, thumb_top=90, thumb_bottom=130)
        right = module.text_right_edge(image, self.BAND)
        thumb_left = module.vertical_thumb_left_edge(image, HALF)
        self.assertEqual((right, thumb_left), (text_right, THUMB_LEFT))
        gap = module.gutter_clear_columns(right, thumb_left)
        return gap, module.gutter_probe_verdict([gap], self.EXPECTED, self.EXPECTED)

    def test_ink_adjacent_to_thumb_is_overlap_despite_exact_ocr(self):
        self.assertEqual(
            self.measured_verdict(THUMB_LEFT - 1), (0, ("OVERLAP", "pixel_gap_below_1")),
        )

    def test_one_empty_column_is_clear(self):
        self.assertEqual(self.measured_verdict(THUMB_LEFT - 2), (1, ("CLEAR", "ocr_exact")))


class GutterProbeVerdict(unittest.TestCase):
    EXPECTED = "2026/11/27 23:56:58"

    def verdict(self, ocr_text, gaps=(4,)):
        return module.gutter_probe_verdict(list(gaps), ocr_text, self.EXPECTED)

    def test_exact_match_is_clear(self):
        self.assertEqual(self.verdict(self.EXPECTED), ("CLEAR", "ocr_exact"))

    def test_real_mid_string_misread_is_clear_informational(self):
        # it13 run3: tesseract read the tens-of-seconds 5 as 3; gap 4 px.
        self.assertEqual(
            self.verdict("2026/11/27 23:56:38"),
            ("CLEAR", "ocr_differs_away_from_right_edge"),
        )

    def test_surrounding_whitespace_is_stripped(self):
        self.assertEqual(self.verdict(f"  {self.EXPECTED}\n"), ("CLEAR", "ocr_exact"))

    def test_one_character_short_is_overlap(self):
        self.assertEqual(self.verdict("2026/11/27 23:56:5"), ("OVERLAP", "ocr_not_full_timestamp"))

    def test_wrong_final_character_is_overlap(self):
        self.assertEqual(self.verdict("2026/11/27 23:56:53"), ("OVERLAP", "ocr_final_char_differs"))

    def test_trailing_junk_is_overlap(self):
        self.assertEqual(self.verdict("2026/11/27 23:56:5:"), ("OVERLAP", "ocr_not_full_timestamp"))

    def test_zero_gap_with_perfect_ocr_is_overlap(self):
        self.assertEqual(self.verdict(self.EXPECTED, gaps=(4, 0)), ("OVERLAP", "pixel_gap_below_1"))
        self.assertEqual(self.verdict(self.EXPECTED, gaps=(-3,)), ("OVERLAP", "pixel_gap_below_1"))

    def test_gap_of_one_is_enough(self):
        self.assertEqual(self.verdict(self.EXPECTED, gaps=(1,)), ("CLEAR", "ocr_exact"))

    def test_empty_ocr_is_overlap(self):
        self.assertEqual(self.verdict(""), ("OVERLAP", "ocr_empty"))
        self.assertEqual(self.verdict("   "), ("OVERLAP", "ocr_empty"))
        self.assertEqual(self.verdict(None), ("OVERLAP", "ocr_empty"))

    def test_no_row_beside_thumb_is_overlap(self):
        self.assertEqual(self.verdict(self.EXPECTED, gaps=()), ("OVERLAP", "no_row_beside_thumb"))

    def test_reasons_are_single_tokens(self):
        for text in (self.EXPECTED, "2026/11/27 23:56:38", "", "2026/11/27 23:56:5"):
            self.assertNotIn(" ", self.verdict(text)[1])


if __name__ == "__main__":
    unittest.main()
