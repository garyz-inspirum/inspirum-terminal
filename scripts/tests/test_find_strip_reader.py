"""Find-strip reader: exact Prev/Next inside the band, slash counter unchanged.

Ubuntu CI does not install tesseract for scripts/tests (only the iced-design
job does), so screenshot assertions are skipped unless tesseract is on PATH.
Counter parsing and the band-membership check do not need it.
"""
from __future__ import annotations

import importlib.util
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_find", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
OPEN_BAND = FIXTURES / "find-strip-open-band.png"
CLOSED_BAND = FIXTURES / "find-strip-closed-band.png"
TOUR_BAND = FIXTURES / "find-strip-tour-band.png"
HAS_TESSERACT = shutil.which("tesseract") is not None
BAND = (0, 0, 1280, 44)


def _row(text, bounds, confidence=90):
    left, top, right, bottom = bounds
    return (text, left, top, right - left, bottom - top, confidence)


class FindStripCounterTest(unittest.TestCase):
    def test_slash_pair_parses_with_the_existing_glyph_table(self):
        parsed = module.find_hit_counter([_row("0/0", (1128, 518, 1151, 530))])
        self.assertEqual(parsed[:2], (0, 0))
        # Light-theme 13px monospace reads the same pair as l/l. l→1 is the
        # existing counter table, not a label confusion.
        parsed = module.find_hit_counter([_row("l/l", (1128, 518, 1151, 530))])
        self.assertEqual(parsed[:2], (1, 1))
        parsed = module.find_hit_counter([_row("O/o", (1128, 518, 1151, 530))])
        self.assertEqual(parsed[:2], (0, 0))

    def test_a_real_nine_is_not_rewritten_to_zero(self):
        # 9/0 was the bad full-band read of an empty counter. Preprocessing
        # must produce 0/0; the parser must not paper over a 9.
        parsed = module.find_hit_counter([_row("9/0", (1128, 518, 1151, 530))])
        self.assertEqual(parsed[:2], (9, 0))
        self.assertIsNone(module.find_hit_counter([_row("Prev", (1163, 518, 1194, 530))]))
        self.assertIsNone(module.find_hit_counter([_row("0 0", (1128, 518, 1151, 530))]))

    def test_split_counter_tokens_on_one_line_still_parse(self):
        words = [
            _row("1", (1128, 518, 1134, 530)),
            _row("/", (1136, 518, 1140, 530)),
            _row("1", (1142, 518, 1148, 530)),
            _row("Next", (1212, 518, 1243, 530)),
        ]
        parsed = module.find_hit_counter(words)
        self.assertEqual(parsed[:2], (1, 1))


class FindStripLabelTest(unittest.TestCase):
    def test_prev_and_next_are_exact_and_must_sit_inside_the_band(self):
        band = (0, 499, 1280, 543)
        words = [
            ("Prey", (1163, 518, 1194, 530)),
            ("Hest", (1212, 518, 1243, 530)),
            ("Prev", (1163, 560, 1194, 572)),
            ("Next", (1212, 518, 1243, 530)),
        ]
        # Prey/Hest are the misses that failed the first run. They are not
        # accepted: the reader has to produce the real labels.
        self.assertIsNone(module.match_strip_label(words, "Prev", band))
        self.assertEqual(
            module.match_strip_label(words, "Next", band),
            (1212, 518, 1243, 530),
        )
        self.assertIsNone(module.match_strip_label(words, "prev", band))
        outside = [("Prev", (1163, 560, 1194, 572))]
        self.assertIsNone(module.match_strip_label(outside, "Prev", band))


@unittest.skipUnless(HAS_TESSERACT, "tesseract is not installed")
@unittest.skipUnless(OPEN_BAND.is_file(), "find-strip band fixtures are not present")
class FindStripOcrTest(unittest.TestCase):
    def test_open_band_reads_prev_next_and_zero_counter(self):
        words = module.read_strip_words(OPEN_BAND, BAND)
        texts = [text for text, _bounds in words]
        self.assertIn("Prev", texts, texts)
        self.assertIn("Next", texts, texts)
        self.assertIsNotNone(module.match_strip_label(words, "Prev", BAND))
        self.assertIsNotNone(module.match_strip_label(words, "Next", BAND))
        parsed = module.find_hit_counter(module.strip_words_as_rows(words))
        self.assertIsNotNone(parsed, texts)
        self.assertEqual(parsed[:2], (0, 0), texts)

    def test_closed_band_has_neither_control(self):
        words = module.read_strip_words(CLOSED_BAND, BAND)
        texts = [text for text, _bounds in words]
        self.assertIsNone(module.match_strip_label(words, "Prev", BAND), texts)
        self.assertIsNone(module.match_strip_label(words, "Next", BAND), texts)

    def test_tour_band_reads_prev_next_and_one_counter(self):
        words = module.read_strip_words(TOUR_BAND, BAND)
        texts = [text for text, _bounds in words]
        self.assertIsNotNone(module.match_strip_label(words, "Prev", BAND), texts)
        self.assertIsNotNone(module.match_strip_label(words, "Next", BAND), texts)
        parsed = module.find_hit_counter(module.strip_words_as_rows(words))
        self.assertIsNotNone(parsed, texts)
        self.assertEqual(parsed[:2], (1, 1), texts)

    def test_one_pixel_button_borders_do_not_hide_the_labels(self):
        from PIL import Image, ImageDraw

        image = Image.open(OPEN_BAND).convert("RGB")
        draw = ImageDraw.Draw(image)
        # 8px padding, 1px border, the restyle the strip is moving to.
        for box in ((1163, 17, 1194, 31), (1212, 17, 1243, 31)):
            left, top, right, bottom = box
            draw.rectangle(
                (left - 8, top - 8, right + 8, bottom + 8),
                outline=(188, 188, 188),
                width=1,
            )
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "bordered.png"
            image.save(path)
            words = module.read_strip_words(path, BAND)
        texts = [text for text, _bounds in words]
        self.assertIsNotNone(module.match_strip_label(words, "Prev", BAND), texts)
        self.assertIsNotNone(module.match_strip_label(words, "Next", BAND), texts)
        parsed = module.find_hit_counter(module.strip_words_as_rows(words))
        self.assertEqual(parsed[:2], (0, 0), texts)


if __name__ == "__main__":
    unittest.main()
