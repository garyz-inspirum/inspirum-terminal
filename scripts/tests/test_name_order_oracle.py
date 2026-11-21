"""Name-order oracle must read Filer Name-column rows, never the status line.

Ubuntu CI does not install tesseract for scripts/tests (only the iced-design
job does), so every OCR assertion is skipped unless tesseract is on PATH.
"""
from __future__ import annotations

import importlib.util
import shutil
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock


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


def _wholly_inside(box, region) -> bool:
    left, top, right, bottom = box
    x1, y1, x2, y2 = region
    return left >= x1 and top >= y1 and right <= x2 and bottom <= y2


@unittest.skipUnless(HAS_TESSERACT, "tesseract is not installed")
class NameOrderOracleTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.row_111 = module.locate_visible_label(ASC, LABEL_111, region=BODY, budget=8)
        cls.rows_region = module.name_rows_region(cls.row_111, 3)

    def test_region_locate_never_returns_box_outside_region(self):
        box = module.locate_visible_label(
            ASC, LABEL_444, region=BODY, budget=20,
        )
        self.assertTrue(
            _wholly_inside(box, BODY),
            f"444 box {box} is not wholly inside {BODY}",
        )
        self.assertGreaterEqual(box[1], 620)
        self.assertLessEqual(box[3], 700)

    def test_ascending_frame_reads_ascending_from_real_rows(self):
        rows_region = self.rows_region
        tops = module.name_order_tops(ASC, LABEL_111, LABEL_444, rows_region)
        self.assertIsNotNone(tops, f"oracle missed ascending rows in {rows_region}")
        top_111, top_444 = tops
        self.assertGreaterEqual(top_111, 620)
        self.assertLessEqual(top_111, 648)
        self.assertGreaterEqual(top_444, 643)
        self.assertLessEqual(top_444, 671)
        self.assertLess(top_111, top_444)

    def test_descending_frame_fails_restore_check(self):
        rows_region = self.rows_region
        tops = module.name_order_tops(DESC, LABEL_111, LABEL_444, rows_region)
        self.assertIsNotNone(tops, f"oracle missed descending rows in {rows_region}")
        top_111, top_444 = tops
        self.assertGreater(
            top_111, top_444,
            f"descending frame reported ascending: y(111)={top_111} y(444)={top_444}",
        )
        restored = top_111 < top_444
        self.assertFalse(restored)

    def test_frozen_descending_frame_with_only_status_line_444_is_rejected(self):
        rows_region = self.rows_region
        located = module.locate_visible_label(DESC, LABEL_444, region=rows_region, budget=8)
        self.assertTrue(_wholly_inside(located, rows_region), located)
        width, height, channels, rows = module._read_png(DESC)
        # 23 px data row that contains the located glyphs (rows start at y=623).
        row_top = located[1] - ((located[1] - 623) % 23)
        background = bytes(rows[min(height - 1, row_top + 4)][channels:channels + 3])
        y0 = max(0, row_top)
        y1 = min(height, row_top + 23)
        for y in range(y0, y1):
            row = rows[y]
            for x in range(0, 290):
                base = x * channels
                row[base:base + 3] = background
        rgb = b"".join(bytes(row) for row in rows)
        saved_budget = module.LOCATE_BUDGET_S
        saved_inverted = module.INVERTED_MIN_S
        # A miss must not walk whole-window fallbacks. Region stages still run.
        setattr(module, "LOCATE_BUDGET_S", 0)
        setattr(module, "INVERTED_MIN_S", 0)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                frozen = Path(tmp) / "frozen-desc.png"
                module._write_png(frozen, width, height, rgb)
                tops = module.name_order_tops(frozen, LABEL_111, LABEL_444, rows_region)
        finally:
            setattr(module, "LOCATE_BUDGET_S", saved_budget)
            setattr(module, "INVERTED_MIN_S", saved_inverted)
        self.assertIsNone(
            tops,
            f"frozen descending frame with only the status-line 444 was accepted: {tops}",
        )

    def test_status_line_box_from_a_fallback_is_rejected(self):
        rows_region = self.rows_region
        row_111 = self.row_111
        status = (70, 784, 300, 794)
        self.assertFalse(_wholly_inside(status, rows_region))

        def fake(_path, label, *, region=None, budget=12):
            if label == LABEL_444:
                return status
            return row_111

        with mock.patch.object(module, "locate_visible_label", side_effect=fake):
            tops = module.name_order_tops(ASC, LABEL_111, LABEL_444, rows_region)
        self.assertIsNone(tops, f"status-line 444 box was accepted: {tops}")


if __name__ == "__main__":
    unittest.main()
