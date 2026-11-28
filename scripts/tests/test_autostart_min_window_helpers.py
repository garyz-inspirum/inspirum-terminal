"""Pure helpers of the INSPIRUM_GUI_AUTOSTART=1 960x640 Tunnel Settings probe.

Real 1280x800 frames come from scripts/tests/fixtures; there is no saved
960x640 dialog frame yet, so the minimum-size cases use synthetic PNGs built
from the 1280 geometry shifted to the 960 dialog (left 60, width 840).
"""
from __future__ import annotations

import ast
import importlib.util
from pathlib import Path
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui_min", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
FIXTURES = Path(__file__).resolve().parent / "fixtures"

DARK = (30, 30, 30)
PRIMARY = (33, 150, 243)


def synthetic(path: Path, width: int, height: int, boxes) -> Path:
    """Dark frame with filled ``(box, rgb)`` rectangles."""
    pixels = bytearray(bytes(DARK) * (width * height))
    for (x0, y0, x1, y1), rgb in boxes:
        for y in range(max(0, y0), min(height, y1)):
            for x in range(max(0, x0), min(width, x1)):
                pixels[(y * width + x) * 3:(y * width + x) * 3 + 3] = bytes(rgb)
    module._write_png(path, width, height, bytes(pixels))
    return path


class DialogGeometry(unittest.TestCase):
    def test_dialog_region_at_min_and_regular(self):
        self.assertEqual(module.tunnel_dialog_region(960, 640), (60, 0, 900, 640))
        self.assertEqual(module.tunnel_dialog_region(1280, 800), (220, 0, 1060, 800))

    def test_cancel_regions_from_measured_1280_footer(self):
        save = (984, 671, 1035, 693)  # connected-autostart-editor-alpha.png
        region = module.footer_cancel_region(save, 1280)
        self.assertEqual(region, (640, 659, 984, 705))
        cancel_text = (923, 678, 961, 683)  # locate_visible_label on that row
        self.assertTrue(module._bounds_inside(cancel_text, region))
        # Measured button 909..976 x 671..693.
        self.assertEqual(module.footer_cancel_button(cancel_text, save), (909, 671, 975, 693))

    def test_notice_region_left_of_cancel_and_clipped(self):
        save = (744, 610, 795, 633)
        self.assertEqual(module.footer_notice_region(save, 960, 640), (66, 574, 664, 640))
        # "changing" at (430,679,..) in the 1280 refusal frame lies inside.
        region = module.footer_notice_region((984, 671, 1035, 693), 1280, 800)
        self.assertTrue(module._bounds_inside((430, 679, 479, 686), region))
        self.assertTrue(module._bounds_inside((250, 680, 269, 686), region))


class FooterSaveBox(unittest.TestCase):
    def test_real_frames(self):
        for name, want in (
            ("connected-tunnels-editor-selected-alpha.png", (984, 661, 1035, 684)),
            ("ci-tunnels-delete-alpha-selected-pre.png", (984, 624, 1035, 647)),
        ):
            with self.subTest(name=name):
                self.assertEqual(module.footer_save_box(FIXTURES / name, 1280, 800), want)

    def test_no_enabled_save_in_frames_without_selection(self):
        for name in ("ci-tunnels-editor-pre-alpha.png", "connected-tunnels-dropdown-plus.png",
                     "connected-tunnels-closed.png"):
            with self.subTest(name=name):
                self.assertIsNone(module.footer_save_box(FIXTURES / name, 1280, 800))

    def test_min_window_popup_frame_has_no_save(self):
        frame = FIXTURES / "tour-tunnels-popup-light-960x640.png"
        self.assertIsNone(module.footer_save_box(frame, 960, 640))

    def test_synthetic_960_save_found_and_local_toggle_ignored(self):
        # 1280 geometry shifted by -160: Save 824..875, "Local" toggle at
        # the editor's Type row (card 397 + 162) and the checked box.
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = synthetic(Path(tmp) / "min.png", 960, 640, [
                ((824, 600, 875, 623), PRIMARY),
                ((559, 120, 610, 143), PRIMARY),
                ((409, 420, 425, 436), PRIMARY),
            ])
            self.assertEqual(module.footer_save_box(path, 960, 640), (824, 600, 875, 623))

    def test_synthetic_clipped_save_is_short_and_unreachable(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = synthetic(Path(tmp) / "clip.png", 960, 640, [((824, 628, 875, 640), PRIMARY)])
            box = module.footer_save_box(path, 960, 640)
        self.assertEqual(box, (824, 628, 875, 640))
        self.assertFalse(module.box_reachable(box, (0, 0, 960, 640),
                                              min_h=module.TUNNEL_FOOTER_BUTTON_MIN_H))

    def test_lowest_group_wins(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = synthetic(Path(tmp) / "two.png", 960, 640, [
                ((824, 300, 875, 323), PRIMARY), ((824, 600, 875, 623), PRIMARY),
            ])
            self.assertEqual(module.footer_save_box(path, 960, 640)[1], 600)


class Reachability(unittest.TestCase):
    FRAME = (0, 0, 960, 640)

    def test_inside_and_tall_enough(self):
        self.assertTrue(module.box_reachable((824, 600, 875, 623), self.FRAME, min_h=18))

    def test_outside_or_short_or_missing(self):
        self.assertFalse(module.box_reachable((824, 630, 875, 653), self.FRAME))
        self.assertFalse(module.box_reachable((-1, 10, 20, 30), self.FRAME))
        self.assertFalse(module.box_reachable((824, 600, 875, 610), self.FRAME, min_h=18))
        self.assertFalse(module.box_reachable(None, self.FRAME))
        self.assertFalse(module.box_reachable((10, 10, 10, 30), self.FRAME))


class NoticeLines(unittest.TestCase):
    def test_one_line(self):
        self.assertEqual(module.distinct_text_lines([(70, 600, 110, 608), (300, 602, 340, 610)]), 1)

    def test_two_lines(self):
        boxes = [(70, 592, 110, 600), (400, 593, 430, 601), (90, 608, 150, 616)]
        self.assertEqual(module.distinct_text_lines(boxes), 2)

    def test_empty(self):
        self.assertEqual(module.distinct_text_lines([]), 0)

    def test_notice_words_match_product_notice_only(self):
        rust = (SOURCE.parents[1] / "src" / "iced_app.rs").read_text(encoding="utf-8")
        start = rust.index("const TUNNEL_AUTO_START_UNSAFE: &str = ")
        notice = rust[start:rust.index(";\n", start)].replace("\\\n", "")
        start = rust.index("const TUNNEL_AUTO_START_HELP: &str = ")
        help_text = rust[start:rust.index(";\n", start)].replace("\\\n", "")
        for word in module.AUTOSTART_MIN_NOTICE_WORDS:
            with self.subTest(word=word):
                self.assertIn(word, notice)
                self.assertNotIn(word, help_text)
                self.assertNotIn(word, module.AUTOSTART_CHECKBOX_LABEL)
        self.assertIn(f"Cannot auto-start this tunnel. Set the bind address", notice)
        self.assertNotIn(module.AUTOSTART_MIN_NOTICE_BIND, ("127.0.0.1", "[::1]"))


class FileHash(unittest.TestCase):
    def test_unchanged_and_changed(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = Path(tmp) / "profiles.json"
            path.write_text("[]", encoding="utf-8")
            before = module.file_sha256(path)
            module.require_file_unchanged(path, before, "ctx")
            path.write_text("[ ]", encoding="utf-8")
            with self.assertRaises(RuntimeError) as caught:
                module.require_file_unchanged(path, before, "ctx")
        self.assertIn("ctx", str(caught.exception))


class ProbePlacement(unittest.TestCase):
    def test_probe_is_module_level_after_precheck_and_called_after_it(self):
        tree = ast.parse(SOURCE.read_text(encoding="utf-8"))
        names = [n.name for n in tree.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]
        self.assertIn("autostart_min_window_probe", names)
        self.assertEqual(names.index("autostart_min_window_probe"),
                         names.index("autostart_precheck") + 1)
        text = SOURCE.read_text(encoding="utf-8")
        main = text[text.index("async def main()"):]
        pre = main.index("await autostart_precheck(")
        probe = main.index("await autostart_min_window_probe(")
        self.assertLess(pre, probe)
        # Nothing between the two calls but their arguments and a comment.
        between = main[pre:probe]
        self.assertNotIn("click_label", between)
        self.assertNotIn("xdotool", between)

    def test_probe_derives_regions_and_never_weakens(self):
        text = SOURCE.read_text(encoding="utf-8")
        start = text.index("async def autostart_min_window_probe(")
        body = text[start:text.index("\nasync def main()", start)]
        self.assertIn("require_min_window", body)
        self.assertIn("tunnel_list_region(heading", body)
        self.assertIn('"connected-autostart-min-notice.png"', body)
        self.assertIn('"connected-autostart-min-notice-scrolled.png"', body)
        self.assertIn("require_file_unchanged", body)
        self.assertNotIn("except Exception", body)
        self.assertNotIn("WARNING", body)


if __name__ == "__main__":
    unittest.main()
