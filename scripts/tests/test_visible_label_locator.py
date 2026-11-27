"""OCR token joining for connected Iced GUI label targeting.

Ubuntu tesseract 5.3.4 with PSM 11, the engine used by the Linux connected
acceptance job, split 000_GUI_DOWNLOAD_FIXTURE.bin on run 38049884103 and
scored both fragments at 24.79. A cropped text row can also drop the
underscores. The click must still come from those visible word boxes, not
from a guessed coordinate.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path
import shutil
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class VisibleLabelLocatorTest(unittest.TestCase):
    def test_joins_split_low_confidence_filename_fragments(self):
        rows = [
            ("999_GUI_UPLOAD_FIXTURE.bin", 331, 493, 213, 14, 71.8),
            ("000_GUI_DOWNLOAD", 802, 485, 139, 28, 24.79),
            ("FIXTURE.bin", 954, 485, 88, 28, 24.79),
            ("2.0", 1124, 494, 21, 10, 95.4),
            ("KiB", 1151, 494, 21, 10, 90.8),
        ]
        bounds = module.match_visible_label(rows, "000_GUI_DOWNLOAD_FIXTURE.bin")
        self.assertIsNotNone(bounds)
        self.assertLess(bounds[0], 820)
        self.assertGreater(bounds[2], 1020)
        self.assertLess(bounds[1], 500)
        self.assertGreater(bounds[3], 500)
        self.assertLess(bounds[2], 1100)

    def test_joins_underscore_dropped_listing_row_tokens(self):
        rows = [
            ("000", 802, 494, 25, 10, 95),
            ("GUI", 831, 494, 35, 13, 95),
            ("DOWNLOAD", 868, 492, 82, 15, 92),
            ("FIXTURE.bin", 951, 492, 90, 15, 89),
            ("rte", 1124, 494, 21, 10, 22),
        ]
        bounds = module.match_visible_label(rows, "000_GUI_DOWNLOAD_FIXTURE.bin")
        self.assertIsNotNone(bounds)
        self.assertLess(bounds[0], 820)
        self.assertGreater(bounds[2], 1020)
        self.assertLess(bounds[2], 1100)

    def test_lone_filename_prefix_is_not_a_click_target(self):
        rows = [("000_GUI_DOWNLOAD", 802, 485, 139, 28, 24.79)]
        self.assertIsNone(
            module.match_visible_label(rows, "000_GUI_DOWNLOAD_FIXTURE.bin")
        )

    def test_joins_transfer_queue_without_the_next_line(self):
        rows = [
            ("TRANSFER", 298, 609, 57, 8, 69.6),
            ("QUEUE", 360, 609, 37, 9, 92.9),
            ("No", 299, 629, 13, 8, 78.6),
            ("transfers", 317, 628, 48, 9, 96.6),
        ]
        bounds = module.match_visible_label(rows, "TRANSFER QUEUE")
        self.assertIsNotNone(bounds)
        self.assertLess(bounds[3], 625)
        self.assertGreater(bounds[0], 280)
        self.assertLess(bounds[2], 420)

    def test_joins_dock_right_on_its_own_line(self):
        rows = [
            ("Dock", 1177, 402, 33, 11, 96.6),
            ("right", 1216, 402, 32, 14, 96.9),
            ("LOCAL", 301, 452, 35, 8, 55.8),
        ]
        bounds = module.match_visible_label(rows, "Dock right")
        self.assertIsNotNone(bounds)
        self.assertGreater(bounds[0], 1100)
        self.assertLess(bounds[3], 430)

    def test_arrow_merged_upload_token_still_targets_the_control(self):
        rows = [("Upload->", 303, 548, 68, 14, 63.7)]
        bounds = module.match_visible_label(rows, "Upload")
        self.assertIsNotNone(bounds)
        self.assertGreater(bounds[0], 290)
        self.assertLess(bounds[2], 390)

    def test_parser_keeps_fragments_below_the_old_confidence_cutoff(self):
        tsv = "\n".join([
            "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext",
            "Estimating resolution as 130",
            "5\t1\t54\t1\t1\t1\t802\t485\t139\t28\t24.790092\t000_GUI_DOWNLOAD",
            "5\t1\t54\t1\t1\t2\t954\t485\t88\t28\t24.790092\tFIXTURE.bin",
            "5\t1\t1\t1\t1\t1\t17\t82\t58\t9\t0.000000\tnoise",
        ])
        rows = module.parse_visible_words(tsv)
        self.assertEqual(
            [row[0] for row in rows],
            ["000_GUI_DOWNLOAD", "FIXTURE.bin"],
        )



CI_TERMINAL_ROWS = (
    Path(__file__).resolve().parent / "fixtures"
    / "connected-tunnels-editor-pre-alpha-terminal-rows-ci.png"
)


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(CI_TERMINAL_ROWS.is_file(), "CI terminal-rows fixture missing")
class TunnelListRegionCiFrameTest(unittest.TestCase):
    """CI frame: NATIVE_ECHO:* terminal lines left of the dialog share the
    Alpha row; region (0, 42, 640, 800) missed "Alpha forward" there."""

    def test_list_rows_found_from_the_dialog_edge(self):
        region = module.tunnel_list_region((245, 147, 372, 158), 1280, 800)
        alpha = module.locate_visible_label(CI_TERMINAL_ROWS, "Alpha forward", region=region)
        self.assertIsNotNone(alpha)
        self.assertGreaterEqual(alpha[0], 280)
        self.assertLessEqual(alpha[2], 400)
        self.assertGreaterEqual(alpha[1], 185)
        self.assertLessEqual(alpha[3], 205)
        bravo = module.locate_visible_label(CI_TERMINAL_ROWS, "Bravo forward", region=region)
        self.assertIsNotNone(bravo)
        self.assertGreaterEqual(bravo[0], 280)
        self.assertLessEqual(bravo[2], 400)
        self.assertGreater(bravo[1], alpha[3])


CI_SELECTED_ALPHA = (
    Path(__file__).resolve().parent / "fixtures"
    / "ci-tunnels-delete-alpha-selected-pre.png"
)
# tunnel_list_region((249, ...), 1280, 800) == (237, 42, 640, 800), the CI
# region. Alpha's selected fill spans y 223..264 on x 253..536 (luma 58 on
# the luma 25 dialog); Bravo's unselected row is below it (text y 274..283).
SELECTED_HEADING = (249, 147, 372, 158)
ALPHA_FILL_Y = (223, 264)
ALPHA_FILL_X = (253, 536)


class SelectedRowFillTest(unittest.TestCase):
    """Pure pixel geometry of the selected-row stage (no OCR)."""

    def frame(self, fill_rows=()):
        width, height = 300, 100
        rows = [bytearray(bytes((25, 25, 25)) * width) for _ in range(height)]
        for y in fill_rows:
            rows[y][30 * 3:270 * 3] = bytes((58, 58, 58)) * 240
        return width, height, rows

    def test_selected_row_label_no_fill_means_no_rect(self):
        width, height, rows = self.frame()
        self.assertEqual(module.selected_fill_rects(rows, 3, (0, 0, width, height), width, height), [])

    def test_selected_row_label_fill_rect_is_the_flat_row(self):
        width, height, rows = self.frame(range(20, 61))
        self.assertEqual(
            module.selected_fill_rects(rows, 3, (0, 0, width, height), width, height),
            [(30, 20, 269, 60, 58)],
        )

    def test_selected_row_label_thin_rule_is_not_a_row(self):
        width, height, rows = self.frame(range(20, 22))
        self.assertEqual(module.selected_fill_rects(rows, 3, (0, 0, width, height), width, height), [])

    @unittest.skipUnless(CI_SELECTED_ALPHA.is_file(), "CI selected-alpha fixture missing")
    def test_selected_row_label_ci_fill_is_alpha_row_only(self):
        width, height, channels, rows = module._read_png(CI_SELECTED_ALPHA)
        region = module.tunnel_list_region(SELECTED_HEADING, width, height)
        self.assertEqual(region, (237, 42, 640, 800))
        rects = module.selected_fill_rects(rows, channels, region, width, height)
        alpha = [r for r in rects if r[1] <= 237 <= r[3] and r[0] <= 337 <= r[2]]
        self.assertEqual(len(alpha), 1, rects)
        x1, y1, x2, y2, _luma = alpha[0]
        self.assertEqual((y1, y2), ALPHA_FILL_Y)
        self.assertEqual((x1, x2), ALPHA_FILL_X)
        self.assertFalse([r for r in rects if r[1] <= 279 <= r[3] and r[0] <= 337 <= r[2]])
        lines = module._fill_text_lines(rows, channels, alpha[0])
        # Title then subtitle, each its own line (not one 26 px band).
        self.assertEqual(len(lines), 2, lines)
        self.assertLess(lines[0][3], lines[1][1])


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(CI_SELECTED_ALPHA.is_file(), "CI selected-alpha fixture missing")
class SelectedRowLabelCiFrameTest(unittest.TestCase):
    """CI run: select_row clicked "Alpha forward" while Alpha was already
    selected (grey fill). The region passes read Bravo but not Alpha; only
    the late whole-window/inverted stages could, and the 12 s budget plus
    click_label's 15 s window cut them off ("OCR miss ... after 14.0s")."""

    region = (237, 42, 640, 800)

    def assert_alpha_row(self, box):
        cx, cy = module.center(box)
        self.assertTrue(ALPHA_FILL_Y[0] < cy < ALPHA_FILL_Y[1], box)
        self.assertTrue(ALPHA_FILL_X[0] < cx < ALPHA_FILL_X[1], box)
        self.assertLessEqual(box[3] - box[1], 14, box)  # the title line only

    def test_selected_row_label_found_without_slow_stages(self):
        # The stages the CI budget cut short must not be needed at all.
        def slow(*_args, **_kwargs):
            raise AssertionError("slow fallback stage reached for a selected row label")
        saved = {name: getattr(module, name) for name in (
            "_region_band_label", "visible_pixel_rows", "visible_segment_rows",
            "_inverted_segment_search")}
        try:
            for name in saved:
                setattr(module, name, slow)
            box = module.locate_visible_label(CI_SELECTED_ALPHA, "Alpha forward", region=self.region)
        finally:
            for name, value in saved.items():
                setattr(module, name, value)
        self.assert_alpha_row(box)

    def test_selected_row_label_found_with_click_label_budget(self):
        box = module.locate_visible_label(
            CI_SELECTED_ALPHA, "Alpha forward", region=self.region,
            budget=module.LOCATE_BUDGET_S,
        )
        self.assert_alpha_row(box)

    def test_selected_row_label_stage_ignores_unselected_rows(self):
        # Bravo is not on a fill: the new stage never answers for it, and
        # the locator keeps the first region pass's box.
        tesseract = shutil.which("tesseract")
        self.assertIsNone(module._selected_row_label(
            CI_SELECTED_ALPHA, tesseract, "Bravo forward", self.region))
        words = module.region_upscaled_rows(CI_SELECTED_ALPHA, tesseract, self.region, psm="6")
        first_pass = module._bounds_inside(module.match_visible_label(words, "Bravo forward"), self.region)
        self.assertIsNotNone(first_pass)
        self.assertEqual(
            module.locate_visible_label(CI_SELECTED_ALPHA, "Bravo forward", region=self.region),
            first_pass,
        )
        self.assertGreater(module.center(first_pass)[1], ALPHA_FILL_Y[1])


if __name__ == "__main__":
    unittest.main()
