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


if __name__ == "__main__":
    unittest.main()
