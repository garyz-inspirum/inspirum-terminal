"""Name-column OCR band must stop at the Filer table's Name divider.

The Filer lays each half out from its own width (src/iced_app.rs
``file_col_name``), so at the 960 px minimum a fixed ``+ 270`` band reaches
into Size. ``filer_name_column_width`` mirrors the Rust clamp and
``name_rows_region(..., half_left=, half_width=)`` clamps the band to it.

The fixture-oracle test needs tesseract and is skipped without it, like
test_name_order_oracle.py.
"""
from __future__ import annotations

import importlib.util
import shutil
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

FIXTURES = Path(__file__).resolve().parent / "fixtures"
ASC = FIXTURES / "connected-files-sorted-asc.png"
DESC = FIXTURES / "connected-files-sorted-desc.png"
BODY = (0, 528, 1280, 778)
LABEL_111 = "111_GUI_THIRD_FIXTURE.bin"
LABEL_444 = "444_GUI_DOWNLOAD_FIXTURE.bin"
HAS_TESSERACT = shutil.which("tesseract") is not None


class FilerNameColumnWidthTest(unittest.TestCase):
    def test_matches_rust_file_col_name(self):
        # Same pairs as Rust file_name_column_adapts_to_half_width, plus 2000.
        for half, expected in [
            (420, 258), (479, 258), (600, 338), (639, 377), (760, 420), (2000, 420),
        ]:
            with self.subTest(half=half):
                self.assertEqual(module.filer_name_column_width(half), expected)


class NameRowsRegionClampTest(unittest.TestCase):
    # 960x640 minimum window: filer_halves puts the divider at x=480, so the
    # remote half is x=481..960 (479 px). Row glyphs start 6 + 14 + 6 px in.
    HALF_LEFT = 481
    HALF_WIDTH = 960 - 481
    ROW = (HALF_LEFT + 26, 630, HALF_LEFT + 26 + 184, 637)

    def test_minimum_half_band_stops_before_name_divider(self):
        self.assertEqual(self.HALF_WIDTH, 479)
        divider = self.HALF_LEFT + 258
        unclamped = module.name_rows_region(self.ROW, 3, name_right=960)
        # The old band (first_left + 270 = 777) does reach into Size here.
        self.assertGreater(unclamped[2], divider + 10)
        region = module.name_rows_region(
            self.ROW, 3, name_right=960,
            half_left=self.HALF_LEFT, half_width=self.HALF_WIDTH,
        )
        self.assertLess(region[2], divider)
        inside_size = divider + 10
        # Crop right edge is exclusive: x is in the band only if x < right.
        self.assertFalse(region[0] <= inside_size < region[2], region)
        # A 28-character name (the minimum the column fits) still fits.
        longest_glyph_right = self.HALF_LEFT + 26 + 226
        self.assertLessEqual(longest_glyph_right, region[2])

    def test_without_half_params_is_unchanged(self):
        row = (31, 630, 215, 637)
        self.assertEqual(module.name_rows_region(row, 3), (0, 624, 301, 701))
        self.assertEqual(
            module.name_rows_region(row, 3, name_right=1280), (0, 624, 1280, 701),
        )
        self.assertEqual(
            module.name_rows_region(row, 3, row_height=20), (0, 624, 301, 692),
        )
        # Only one of the pair given: no clamp.
        self.assertEqual(module.name_rows_region(row, 3, half_left=0), (0, 624, 301, 701))
        self.assertEqual(module.name_rows_region(row, 3, half_width=479), (0, 624, 301, 701))


@unittest.skipUnless(HAS_TESSERACT, "tesseract is not installed")
class ClampedBandFixtureOracleTest(unittest.TestCase):
    # The fixtures are 1280x800 single-pane Filer frames (pre-split, old
    # column order, Name 0..290): table left edge x=0. A 639 px half there
    # gives a Name divider at 377, so the clamp leaves the band at 301 and
    # the existing oracle expectations must still hold.
    @classmethod
    def setUpClass(cls):
        cls.row_111 = module.locate_visible_label(ASC, LABEL_111, region=BODY, budget=8)
        cls.rows_region = module.name_rows_region(
            cls.row_111, 3, half_left=0, half_width=639,
        )

    def test_clamp_keeps_band_inside_name_divider(self):
        self.assertLess(self.rows_region[2], 0 + 377)

    def test_ascending_frame_reads_ascending(self):
        tops = module.name_order_tops(ASC, LABEL_111, LABEL_444, self.rows_region)
        self.assertIsNotNone(tops, f"oracle missed ascending rows in {self.rows_region}")
        self.assertLess(tops[0], tops[1])

    def test_descending_frame_reads_descending(self):
        tops = module.name_order_tops(DESC, LABEL_111, LABEL_444, self.rows_region)
        self.assertIsNotNone(tops, f"oracle missed descending rows in {self.rows_region}")
        self.assertGreater(tops[0], tops[1])


if __name__ == "__main__":
    unittest.main()
