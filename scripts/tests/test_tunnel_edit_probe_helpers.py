"""Geometry/fill helpers behind the running-forward edit/delete GUI probe.

The probe clicks Save by its primary fill and the list's remove button by
its outlined box (pixels under the list), then requires the footer notice
and the persisted profile file. These tests pin each helper on the real populated 1280x800 frame and
show it finds nothing on a frame without the Tunnel Manager.
"""
from __future__ import annotations

import importlib.util
import json
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_edit", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
EDITOR = FIXTURES / "connected-tunnels-editor-selected-alpha.png"
CLOSED = FIXTURES / "connected-tunnels-closed.png"
HEADING = (249, 151, 367, 159)
# Same step after the Session / profile row became a combo box (rows below
# it 44 px lower, no "Add to session" list under +/-). OCR boxes from the
# GUI image; button and Save boxes measured with _read_png.
NEW_EDITOR = FIXTURES / "connected-tunnels-editor-selected-alpha-52241-host.png"
NEW_HEADING = (249, 188, 367, 196)
OLD_LISTEN_LABEL = (574, 292, 644, 296)
NEW_LISTEN_LABEL = (574, 336, 644, 340)
# Session-dropdown step, nothing selected: "−" (and Save) disabled, each
# 4 px taller, the centred dialog 8 px taller (top 124 vs 128).
DROPDOWN_PLUS = FIXTURES / "connected-tunnels-dropdown-plus.png"
DROPDOWN_HEADING = (249, 184, 367, 192)


@unittest.skipUnless(EDITOR.is_file() and CLOSED.is_file(), "tunnel fixtures missing")
class TunnelFrameHelpersTest(unittest.TestCase):
    def test_save_fill_found_on_manager_frame(self):
        (x, y), box = module.tunnel_save_button(EDITOR, HEADING, 1280)
        # Measured Save button 984..1035 x 661..684.
        self.assertEqual(box, (984, 661, 1035, 684))
        self.assertEqual((x, y), (1009, 672))

    def test_save_fill_absent_without_manager(self):
        self.assertIsNone(module.tunnel_save_button(CLOSED, HEADING, 1280))

    def test_local_kind_toggle_is_not_save(self):
        # "Local" is also primary-filled; the scan band starts below it.
        (_xy, box) = module.tunnel_save_button(EDITOR, HEADING, 1280)
        self.assertGreater(box[1], HEADING[1] + 97)

    def test_remove_centre_hits_the_minus_glyph(self):
        # Outlined "+" x 253..280 and "−" x 285..312 (_read_png), y 544..566
        # on the old layout, 581..603 on the new one.
        for frame, heading, top in ((EDITOR, HEADING, 544), (NEW_EDITOR, NEW_HEADING, 581)):
            with self.subTest(frame=frame.name):
                plus, minus = module.tunnel_list_buttons(frame, heading, 1280)
                print(f"list buttons {frame.name}: + {plus} - {minus}", flush=True)
                self.assertEqual((plus[1], plus[3]), (top, top + 23))
                self.assertEqual(minus[1:], (top, 313, top + 23))
                x, y = module.tunnel_remove_centre(frame, heading, 1280)
                self.assertTrue(minus[0] < x < minus[2] and minus[1] < y < minus[3])
                width, _h, channels, rows = module._read_png(frame)
                glyph = [
                    (gx, gy) for gy in range(y - 4, y + 5) for gx in range(x - 6, x + 7)
                    if rows[gy][gx * channels] > 100
                ]
                self.assertTrue(glyph, "no light minus glyph near the remove target")
                self.assertLess(x, 545, "remove target must be inside the list card")
                ax, ay = module.tunnel_add_centre(frame, heading, 1280)
                self.assertEqual(ay, y)
                self.assertTrue(plus[0] < ax < plus[2] and ax < x - 20, (ax, x))

    def test_both_enabled_on_selected_frame(self):
        # A forward is selected: "+" and "−" both enabled 28x23, fill 30.
        (plus, plus_on), (minus, minus_on) = module.tunnel_list_button_states(
            NEW_EDITOR, NEW_HEADING, 1280)
        self.assertEqual((plus, minus), ((254, 581, 281, 604), (286, 581, 313, 604)))
        self.assertTrue(plus_on and minus_on)
        self.assertEqual(module.tunnel_add_centre(NEW_EDITOR, NEW_HEADING, 1280), (267, 592))
        self.assertEqual(module.tunnel_remove_centre(NEW_EDITOR, NEW_HEADING, 1280), (299, 592))

    @unittest.skipUnless(DROPDOWN_PLUS.is_file(), "dropdown fixture missing")
    def test_add_found_beside_disabled_remove(self):
        # Nothing selected: "+" enabled 28x23 (fill 30), "−" disabled 28x27
        # (same border luma 68, fill = card 25); bottoms differ by 4 px.
        (plus, plus_on), (minus, minus_on) = module.tunnel_list_button_states(
            DROPDOWN_PLUS, DROPDOWN_HEADING, 1280)
        self.assertEqual(plus, (254, 577, 281, 600))
        self.assertEqual(minus, (286, 577, 313, 604))
        self.assertTrue(plus_on)
        self.assertFalse(minus_on)
        x, y = module.tunnel_add_centre(DROPDOWN_PLUS, DROPDOWN_HEADING, 1280)
        self.assertEqual((x, y), (267, 588))
        self.assertTrue(plus[0] < x < plus[2] and plus[1] < y < plus[3])
        _w, _h, channels, rows = module._read_png(DROPDOWN_PLUS)
        self.assertGreater(rows[y][x * channels], 150, "add target is not on the + glyph")

    @unittest.skipUnless(DROPDOWN_PLUS.is_file(), "dropdown fixture missing")
    def test_remove_refuses_disabled_minus(self):
        # Found, but disabled: a click would be a silent no-op, so this is a
        # different error from "no buttons".
        with self.assertRaisesRegex(RuntimeError, "remove .* is disabled"):
            module.tunnel_remove_centre(DROPDOWN_PLUS, DROPDOWN_HEADING, 1280)

    def test_fill_rule_rejects_neither_rendering(self):
        # 6x6-px boxes on a synthetic card: interior +5 = enabled, +0 =
        # disabled, +2 = neither (raises rather than guessing).
        def image(fill):
            rows = []
            for y in range(20):
                row = bytearray()
                for x in range(20):
                    v = fill if 8 <= x < 18 and 4 <= y < 16 else 25
                    row += bytes((v, v, v))
                rows.append(row)
            return 20, 20, 3, rows
        box = (8, 4, 18, 16)
        self.assertTrue(module.tunnel_list_button_enabled(image(30), box))
        self.assertFalse(module.tunnel_list_button_enabled(image(25), box))
        with self.assertRaisesRegex(RuntimeError, "neither enabled nor disabled"):
            module.tunnel_list_button_enabled(image(27), box)

    def test_list_buttons_absent_without_manager(self):
        self.assertIsNone(module.tunnel_list_buttons(CLOSED, HEADING, 1280))
        with self.assertRaisesRegex(RuntimeError, "no \\+/- buttons"):
            module.tunnel_remove_centre(CLOSED, HEADING, 1280)

    def test_listen_input_centre_inside_input(self):
        for label, (y1, y2) in ((OLD_LISTEN_LABEL, (287, 310)), (NEW_LISTEN_LABEL, (332, 355))):
            x, y = module.listen_input_centre(label, 1280)
            self.assertTrue(719 <= x <= 1023 and y1 + 4 <= y <= y2 - 4, (x, y))

    def test_save_fill_found_on_new_layout_frame(self):
        (x, y), box = module.tunnel_save_button(NEW_EDITOR, NEW_HEADING, 1280)
        # Measured Save button 984..1035 x 624..647.
        self.assertEqual(box, (984, 624, 1035, 647))
        self.assertEqual((x, y), (1009, 635))

    def test_notice_region_left_of_cancel(self):
        _xy, box = module.tunnel_save_button(EDITOR, HEADING, 1280)
        region = module.tunnel_notice_region(box, 1280)
        self.assertLess(region[2], 909, "notice region must not reach Cancel")
        self.assertLessEqual(region[1], 661)
        self.assertGreaterEqual(region[3], 684)


LIGHT_EMPTY = FIXTURES / "tour-tunnel-manager-light-empty.png"


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(LIGHT_EMPTY.is_file(), "light tour fixture missing")
class LightLocateTest(unittest.TestCase):
    """Real LIGHT 1280x800 Tunnel Manager (unpopulated tour capture); the
    dark pipeline missed its heading in that run (OCR miss after 22 s)."""

    def test_heading_found_inside_region(self):
        region = (0, 42, 1280, 400)
        box = module.light_locate(LIGHT_EMPTY, "Tunnel Manager", region=region)
        self.assertIsNotNone(module._bounds_inside(box, region))
        left, dialog = module.tunnel_dialog_left(1280)
        self.assertTrue(left <= box[0] < left + 60, box)

    def test_absent_forward_label_fails(self):
        # Negative control: the unpopulated manager has no saved forward.
        for label in ("Alpha forward", "Bravo forward"):
            with self.assertRaises(RuntimeError):
                module.light_locate(LIGHT_EMPTY, label, region=(0, 42, 1280, 800))

    def test_region_is_a_hard_box(self):
        with self.assertRaises(RuntimeError):
            module.light_locate(LIGHT_EMPTY, "Tunnel Manager", region=(640, 42, 1280, 800))

    def test_minimum_window_geometry(self):
        left, dialog = module.tunnel_dialog_left(960)
        self.assertEqual((left, dialog), (60, 840))
        # Label column at the narrowest dialog: x 409..537, label OCR ~414.
        row = module.listen_port_row_region((414, 200, 484, 204), 960)
        self.assertTrue(left + 325 < row[0] < left + 337 + 12 + 128 and row[2] <= 960, row)
        region = module.tunnel_editor_label_region((89, 40, 207, 48), 960, 640)
        self.assertEqual(region, (391, 40, 557, 640))


class SavedForwardsTest(unittest.TestCase):
    def write(self, payload) -> Path:
        path = Path(tempfile.mkstemp(suffix=".json")[1])
        self.addCleanup(path.unlink, missing_ok=True)
        path.write_text(json.dumps(payload), encoding="utf-8")
        return path

    def test_reads_named_profile(self):
        path = self.write([
            {"name": "CI connected fixture"},
            {"name": "CI tunnel fixture", "ssh": {"local_forwards": ["a", "b"]}},
        ])
        self.assertEqual(module.saved_local_forwards(path, "CI tunnel fixture"), ["a", "b"])
        self.assertEqual(module.saved_local_forwards(path, "CI connected fixture"), [])

    def test_wrapped_sessions_and_missing_profile(self):
        path = self.write({"sessions": [{"name": "x", "ssh": {"local_forwards": ["c"]}}]})
        self.assertEqual(module.saved_local_forwards(path, "x"), ["c"])
        with self.assertRaises(RuntimeError):
            module.saved_local_forwards(path, "CI tunnel fixture")


if __name__ == "__main__":
    unittest.main()


class PopulatedMinimumWindowTest(unittest.TestCase):
    """s05c populated LIGHT Tunnel Manager must be captured at the product
    minimum 960x640 (src/iced_app.rs), never a WM-forced clipped 640x480."""

    def test_min_window_matches_product(self):
        product = (Path(__file__).resolve().parents[2] / "src" / "iced_app.rs").read_text()
        self.assertIn("min_size: Some(iced::Size::new(960.0, 640.0))", product)
        self.assertEqual(module.MIN_WINDOW, (960, 640))

    def test_rejects_clipped_and_wrong_sizes(self):
        for size in ((640, 480), (959, 640), (960, 639), (1280, 800)):
            with self.assertRaises(RuntimeError):
                module.require_min_window(size)
        self.assertEqual(module.require_min_window((960, 640)), "960x640")

    def test_tour_requests_and_gates_supported_minimum(self):
        src = SOURCE.read_text()
        start = src.index("async def s05_tunnels_min")
        body = src[start:src.index("if POPULATED:", start)]
        self.assertNotRegex(body, r"resize_tour\(\s*640\s*,\s*480\s*\)")
        self.assertIn("resize_tour(*MIN_WINDOW)", body)
        self.assertIn("require_min_window((min_w, min_h))", body)
        # Gate sits inside try so a wrong size reaches FAILED and restores.
        self.assertLess(body.index("try:"), body.index("require_min_window"))
        self.assertIn("light_locate", body)
        self.assertIn("require_editor_selection", body)
