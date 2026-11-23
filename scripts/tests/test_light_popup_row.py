"""Populated LIGHT Tunnels popup rows are proven by name word + own port.

Real 960x640 frame (tour-05d-tunnels-popup-960x640.png, populated tour):
rows "Alpha forward" / "CI tunnel fixture . port 41201" and "Bravo forward"
/ "... port 55057" above the "Tunnel Settings..." footer. The GUI container's
tesseract read Bravo's title as ['Bravo', 'Torward'] and the port as 55657 in
the whole-popup crop, so the full-label locator missed a visibly rendered row.
The row proof instead requires the exact first word AND the forward's own
listen port inside that row's band, above the footer.
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
    for _name in ("SSHServerSession", "SSHServer", "SFTPServer",
                  "SSHServerChannel", "SSHServerConnection", "SSHKey"):
        setattr(_stub, _name, type(_name, (), {}))
    sys.modules["asyncssh"] = _stub

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui_popup", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "tour-tunnels-popup-light-960x640.png"
POPUP = (960 - 440, 32, 960, 640)
FOOTER_TOP = 158  # "Tunnel Settings..." OCR top on the fixture
ALPHA, BRAVO = 41201, 55057

# Whole-popup psm 6 words the GUI container's tesseract returned for the
# Bravo row (tour.log), with boxes from the same frame.
CONTAINER_BRAVO_ROWS = [
    ("Bravo", 690, 121, 37, 9, 95.0), ("Torward", 737, 119, 52, 11, 40.0),
    ("CI", 690, 137, 11, 8, 92.0), ("tunnel", 710, 136, 37, 9, 95.0),
    ("fixture", 757, 136, 44, 9, 96.0), ("port", 823, 139, 23, 6, 96.0),
    ("55657", 856, 137, 30, 8, 79.0),
]


class ContainerMisreadTest(unittest.TestCase):
    def test_full_label_matcher_misses_container_words(self):
        # The original failure: a visible row, full label unmatched.
        self.assertIsNone(module.match_visible_label(CONTAINER_BRAVO_ROWS, "Bravo forward"))

    def test_row_proof_ignores_suffix_misread(self):
        def ocr(_path, region, *, psm, scale):
            if region == POPUP:
                return CONTAINER_BRAVO_ROWS
            # Row-band crop: port read from the band (as local OCR does).
            return [r if r[0] != "55657" else ("55057",) + r[1:]
                    for r in CONTAINER_BRAVO_ROWS
                    if module._word_inside(r, region)]
        box = module.light_popup_row(
            FIXTURE, "Bravo", BRAVO, ALPHA, region=POPUP, footer_top=FOOTER_TOP, ocr=ocr,
        )
        self.assertLessEqual(box[3], FOOTER_TOP)

    def test_name_without_own_port_in_row_fails(self):
        # "Bravo" visible but only another row carries the port: rejected.
        rows = [r for r in CONTAINER_BRAVO_ROWS if r[0] != "55657"]
        rows.append(("55057", 856, 400, 30, 8, 90.0))  # elsewhere in popup
        def ocr(_path, region, *, psm, scale):
            return [r for r in rows if module._word_inside(r, region)]
        with self.assertRaises(RuntimeError):
            module.light_popup_row(
                FIXTURE, "Bravo", BRAVO, ALPHA, region=POPUP, footer_top=FOOTER_TOP, ocr=ocr,
            )

    def test_row_below_footer_fails(self):
        rows = [(t, x, y + 300, w, h, c) for t, x, y, w, h, c in CONTAINER_BRAVO_ROWS]
        def ocr(_path, region, *, psm, scale):
            return [r for r in rows if module._word_inside(r, region)]
        with self.assertRaises(RuntimeError):
            module.light_popup_row(
                FIXTURE, "Bravo", BRAVO, ALPHA, region=POPUP, footer_top=FOOTER_TOP, ocr=ocr,
            )


@unittest.skipUnless(shutil.which("tesseract"), "tesseract unavailable")
class RealPopupFrameTest(unittest.TestCase):
    def test_both_rows_proven_on_real_frame(self):
        for name, port, sibling, (top, bottom) in (
            ("Alpha", ALPHA, BRAVO, (78, 117)), ("Bravo", BRAVO, ALPHA, (114, 153)),
        ):
            box = module.light_popup_row(
                FIXTURE, name, port, sibling, region=POPUP, footer_top=FOOTER_TOP,
            )
            self.assertTrue(top <= box[1] and box[3] <= bottom, (name, box))

    def test_port_of_other_row_rejected_on_real_frame(self):
        # 41201 is visible in the popup, but in Alpha's row, not Bravo's.
        with self.assertRaises(RuntimeError):
            module.light_popup_row(
                FIXTURE, "Bravo", ALPHA, BRAVO, region=POPUP, footer_top=FOOTER_TOP,
            )

    def test_absent_name_rejected_on_real_frame(self):
        with self.assertRaises(RuntimeError):
            module.light_popup_row(
                FIXTURE, "Charlie", BRAVO, ALPHA, region=POPUP, footer_top=FOOTER_TOP,
            )


if __name__ == "__main__":
    unittest.main()
