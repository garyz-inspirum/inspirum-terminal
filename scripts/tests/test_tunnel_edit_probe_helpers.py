"""Geometry/fill helpers behind the running-forward edit/delete GUI probe.

The probe clicks Save by its primary fill and the list's remove button by
its outlined box (pixels under the list), then requires the footer notice
and the persisted profile file. These tests pin each helper on the real populated 1280x800 frame and
show it finds nothing on a frame without the Tunnel Manager.
"""
from __future__ import annotations

import ast
import importlib.util
import json
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
# GitHub Ubuntu runner frames (run for head 4704a39): same layout, every
# outline anti-aliased over 2 px. CI_REMOVE is the frame press_remove read
# before failing with "fill luma -6"; GSBDEV_REMOVE the same step on the
# build host (crisp 1 px borders); CI_PRE_ALPHA a runner frame with nothing
# selected (disabled "−", which the old card sample read as -9).
CI_REMOVE = FIXTURES / "ci-tunnels-delete-alpha-remove.png"
GSBDEV_REMOVE = FIXTURES / "gsbdev-tunnels-delete-alpha-remove.png"
CI_PRE_ALPHA = FIXTURES / "ci-tunnels-editor-pre-alpha.png"


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

    @unittest.skipUnless(CI_REMOVE.is_file(), "CI remove fixture missing")
    def test_aa_border_ci_frame_reads_enabled_minus(self):
        # The runner's 2 px left border of "−" is x 284,285 (36,61) and the
        # detected box starts at 286; the old card sample x 283,284 read
        # 25,36 (median 36): 30 - 36 = -6. The card itself is 281..283 = 25.
        _w, _h, channels, rows = image = module._read_png(CI_REMOVE)
        self.assertEqual([module._luma_at(rows, channels, x, 592) for x in range(279, 287)],
                         [38, 60, 25, 25, 25, 36, 61, 30])
        (plus, plus_on), (minus, minus_on) = module.tunnel_list_button_states(
            CI_REMOVE, NEW_HEADING, 1280, image=image)
        self.assertEqual((plus, minus), ((254, 581, 281, 604), (286, 581, 312, 604)))
        self.assertTrue(plus_on and minus_on)
        x, y = module.tunnel_remove_centre(CI_REMOVE, NEW_HEADING, 1280)
        print(f"aa_border CI remove centre ({x},{y})", flush=True)
        self.assertTrue(286 <= x <= 310 and 583 <= y <= 601, (x, y))
        self.assertEqual((x, y), (299, 592))
        self.assertGreater(rows[y][x * channels], 100, "remove target is not on the − glyph")
        self.assertEqual(module.tunnel_add_centre(CI_REMOVE, NEW_HEADING, 1280), (267, 592))

    @unittest.skipUnless(GSBDEV_REMOVE.is_file(), "gsb-dev remove fixture missing")
    def test_aa_border_gsbdev_frame_unchanged(self):
        # Recorded with the old card sample before the change.
        (plus, plus_on), (minus, minus_on) = module.tunnel_list_button_states(
            GSBDEV_REMOVE, NEW_HEADING, 1280)
        self.assertEqual((plus, minus), ((254, 581, 281, 604), (286, 581, 313, 604)))
        self.assertTrue(plus_on and minus_on)
        self.assertEqual(module.tunnel_remove_centre(GSBDEV_REMOVE, NEW_HEADING, 1280), (299, 592))
        self.assertEqual(module.tunnel_add_centre(GSBDEV_REMOVE, NEW_HEADING, 1280), (267, 592))

    @unittest.skipUnless(DROPDOWN_PLUS.is_file() and CI_PRE_ALPHA.is_file(), "fixtures missing")
    def test_aa_border_disabled_minus_still_refused(self):
        # Host and runner renderings of a disabled "−" (fill = card 25).
        for frame, minus_box in ((DROPDOWN_PLUS, (286, 577, 313, 604)),
                                 (CI_PRE_ALPHA, (286, 577, 312, 604))):
            with self.subTest(frame=frame.name):
                (plus, plus_on), (minus, minus_on) = module.tunnel_list_button_states(
                    frame, DROPDOWN_HEADING, 1280)
                self.assertEqual(plus, (254, 577, 281, 600))
                self.assertEqual(minus, minus_box)
                self.assertTrue(plus_on)
                self.assertFalse(minus_on)
                with self.assertRaisesRegex(RuntimeError, "remove .* is disabled"):
                    module.tunnel_remove_centre(frame, DROPDOWN_HEADING, 1280)
                self.assertEqual(module.tunnel_add_centre(frame, DROPDOWN_HEADING, 1280), (267, 588))

    def test_aa_border_ambiguous_fill_and_reference_rejected(self):
        # 40x30 card (25) with a 10x12 box at (16, 8); the card reference
        # strip is x 11..13. Optional overrides paint single columns.
        def image(fill, columns=None, interior=None):
            rows = []
            for y in range(30):
                row = bytearray()
                for x in range(40):
                    v = 25
                    if 16 <= x < 26 and 8 <= y < 20:
                        v = fill
                        if interior is not None and interior(x, y):
                            v = 180
                    if columns and x in columns and 8 <= y < 20:
                        v = columns[x]
                    row += bytes((v, v, v))
                rows.append(row)
            return 40, 30, 3, rows
        box = (16, 8, 26, 20)
        self.assertTrue(module.tunnel_list_button_enabled(image(30), box))
        self.assertFalse(module.tunnel_list_button_enabled(image(25), box))
        for fill in (27, 19):  # +2 and -6 against a genuinely flat card
            with self.subTest(fill=fill):
                with self.assertRaisesRegex(RuntimeError, "neither enabled nor disabled"):
                    module.tunnel_list_button_enabled(image(fill), box)
        # An anti-aliased border pixel inside the reference strip: not card.
        with self.assertRaisesRegex(RuntimeError, r"card reference x 11\.\.13 spread 25\.\.36 is not flat"):
            module.tunnel_list_button_enabled(image(30, columns={13: 36}), box)
        # The old sample (x1-3..x1-2) would have read this border; the new
        # one does not, so the same enabled box still reads enabled.
        self.assertTrue(module.tunnel_list_button_enabled(image(30, columns={14: 36, 15: 61}), box))
        # Glyph strokes over most of the interior cannot pose as the fill.
        with self.assertRaisesRegex(RuntimeError, "covers only"):
            module.tunnel_list_button_enabled(image(30, interior=lambda x, y: (x + y) % 2 == 0), box)

    @unittest.skipUnless(CI_REMOVE.is_file(), "CI remove fixture missing")
    def test_aa_border_ci_frame_minus_repainted_to_card_is_disabled(self):
        # The runner frame with the "−" fill and glyph (inside its 2 px
        # borders, x 286..310 x 582..602) repainted to the card luma 25.
        width, height, channels, rows = module._read_png(CI_REMOVE)
        rows = [bytearray(r) for r in rows]
        for y in range(582, 603):
            for x in range(286, 311):
                rows[y][x * channels:x * channels + 3] = bytes((25, 25, 25))
        mutated = (width, height, channels, rows)
        (plus, plus_on), (minus, minus_on) = module.tunnel_list_button_states(
            CI_REMOVE, NEW_HEADING, 1280, image=mutated)
        self.assertEqual(minus, (286, 581, 312, 604))
        self.assertTrue(plus_on)
        self.assertFalse(minus_on)
        real = module._read_png
        with mock.patch.object(module, "_read_png",
                               lambda path: mutated if path == CI_REMOVE else real(path)):
            with self.assertRaisesRegex(RuntimeError, "remove .* is disabled"):
                module.tunnel_remove_centre(CI_REMOVE, NEW_HEADING, 1280)

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


# Runner frame of head 937f4fc: the Tunnel Settings dialog sits half a
# pixel lower (heading OCR y 143..152 vs 142..150 on the build host), so
# besides the 2 px side borders every top/bottom border is anti-aliased
# over TWO rows: top y 534/535 = luma 48/49, bottom y 556/557 = 53/44, and
# no single top row reaches the crisp-border floor of 50. HALF_PIXEL_LOCAL
# is the same step from the local (build-host container) run of that head.
HALF_PIXEL_CI = FIXTURES / "ci2-tunnels-delete-alpha-remove.png"
HALF_PIXEL_CI_HEADING = (250, 143, 367, 152)
HALF_PIXEL_LOCAL = FIXTURES / "local2-tunnels-delete-alpha-remove.png"
HALF_PIXEL_LOCAL_HEADING = (249, 142, 367, 150)


@unittest.skipUnless(HALF_PIXEL_CI.is_file() and HALF_PIXEL_LOCAL.is_file(),
                     "half-pixel fixtures missing")
class HalfPixelListButtonsTest(unittest.TestCase):
    def _states(self, image=None, frame=HALF_PIXEL_CI, heading=HALF_PIXEL_CI_HEADING):
        return module.tunnel_list_button_states(frame, heading, 1280, image=image)

    def test_border_rows_are_split(self):
        _w, _h, channels, rows = module._read_png(HALF_PIXEL_CI)
        luma = lambda x, y: module._luma_at(rows, channels, x, y)  # noqa: E731
        self.assertEqual([luma(270, y) for y in range(533, 538)], [25, 48, 49, 30, 30])
        self.assertEqual([luma(270, y) for y in range(555, 559)], [30, 53, 44, 25])
        self.assertTrue(all(luma(x, 535) < 50 for x in range(255, 278)))

    def test_ci_frame_finds_one_enabled_pair(self):
        found = self._states()
        self.assertIsNotNone(found, "no +/- pair on the half-pixel runner frame")
        (plus, plus_on), (minus, minus_on) = found
        print(f"half-pixel CI pair: + {plus} - {minus}", flush=True)
        self.assertTrue(plus_on and minus_on)
        for box, (lo, hi) in ((plus, (253, 281)), (minus, (284, 313))):
            self.assertTrue(lo <= box[0] and box[2] <= hi + 1, box)
            self.assertTrue(533 <= box[1] <= 535 and 557 <= box[3] <= 559, box)
        x, y = module.tunnel_remove_centre(HALF_PIXEL_CI, HALF_PIXEL_CI_HEADING, 1280)
        self.assertTrue(286 <= x <= 313 and 534 <= y <= 558, (x, y))

    def test_ci_click_points_inside_button_interiors(self):
        # Reviewer-measured interiors: "+" x 255..280, "−" x 287..312,
        # both y 536..555 (fill 30, card 25).
        _w, _h, channels, rows = module._read_png(HALF_PIXEL_CI)
        ax, ay = module.tunnel_add_centre(HALF_PIXEL_CI, HALF_PIXEL_CI_HEADING, 1280)
        rx, ry = module.tunnel_remove_centre(HALF_PIXEL_CI, HALF_PIXEL_CI_HEADING, 1280)
        print(f"half-pixel CI add ({ax},{ay}) remove ({rx},{ry})", flush=True)
        self.assertTrue(255 < ax < 280 and 536 < ay < 555, (ax, ay))
        self.assertTrue(287 < rx < 312 and 536 < ry < 555, (rx, ry))
        for x, y in ((ax, ay), (rx, ry)):
            self.assertGreaterEqual(module._luma_at(rows, channels, x, y), 28,
                                    f"({x},{y}) is card background, not the button")

    def test_local_frame_boxes_unchanged(self):
        # Recorded on 937f4fc's detector before the half-pixel change.
        (plus, plus_on), (minus, minus_on) = self._states(
            frame=HALF_PIXEL_LOCAL, heading=HALF_PIXEL_LOCAL_HEADING)
        self.assertEqual((plus, minus), ((254, 535, 281, 557), (286, 535, 313, 557)))
        self.assertTrue(plus_on and minus_on)
        self.assertEqual(module.tunnel_remove_centre(
            HALF_PIXEL_LOCAL, HALF_PIXEL_LOCAL_HEADING, 1280), (299, 546))
        self.assertEqual(module.tunnel_add_centre(
            HALF_PIXEL_LOCAL, HALF_PIXEL_LOCAL_HEADING, 1280), (267, 546))

    def test_one_button_painted_out_is_no_pair(self):
        # Negative control: either button (borders included, 1 px margin)
        # repainted to the card leaves a lone outline; it must not pair
        # with list-row outlines, the list card or the editor card.
        width, height, channels, real_rows = module._read_png(HALF_PIXEL_CI)
        real = module._read_png
        for name, (bx1, bx2) in (("plus", (252, 281)), ("minus", (283, 313))):
            with self.subTest(removed=name):
                rows = [bytearray(r) for r in real_rows]
                for y in range(533, 559):
                    for x in range(bx1, bx2 + 1):
                        rows[y][x * channels:x * channels + 3] = bytes((25, 25, 25))
                mutated = (width, height, channels, rows)
                self.assertIsNone(self._states(image=mutated))
                with mock.patch.object(
                        module, "_read_png",
                        lambda path, m=mutated: m if path == HALF_PIXEL_CI else real(path)):
                    with self.assertRaisesRegex(RuntimeError, r"no \+/- buttons"):
                        module.tunnel_remove_centre(HALF_PIXEL_CI, HALF_PIXEL_CI_HEADING, 1280)
                    with self.assertRaisesRegex(RuntimeError, r"no \+/- buttons"):
                        module.tunnel_add_centre(HALF_PIXEL_CI, HALF_PIXEL_CI_HEADING, 1280)


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
        # Exactly s05_tunnels_min's own source (via ast), not whatever text
        # follows it up to the next marker.
        fns = [node for node in ast.walk(ast.parse(src))
               if isinstance(node, ast.AsyncFunctionDef) and node.name == "s05_tunnels_min"]
        self.assertEqual(len(fns), 1)
        body = ast.get_source_segment(src, fns[0])
        self.assertTrue(body.startswith("async def s05_tunnels_min("))
        self.assertNotRegex(body, r"resize_tour\(\s*640\s*,\s*480\s*\)")
        self.assertIn("resize_tour(*MIN_WINDOW)", body)
        self.assertIn("require_min_window((min_w, min_h))", body)
        # Gate sits inside try so a wrong size reaches FAILED and restores.
        self.assertLess(body.index("try:"), body.index("require_min_window"))
        self.assertIn("light_locate", body)
        self.assertIn("require_editor_selection", body)


# Located label boxes (locate_visible_label) on the real editor frames.
NEW_ORDER = {
    "Session / profile": (573, 301, 687, 308),
    "Listen Port": (574, 336, 644, 340),
    "Destination Host": (573, 371, 680, 375),
}
OLD_ORDER = {
    "Session / profile": (573, 264, 687, 271),
    "Listen Port": (574, 292, 644, 296),
    "Destination Host": (573, 327, 680, 331),
}


class EditorRowOrderTest(unittest.TestCase):
    """``require_editor_row_order``: Session / profile < Listen Port <
    Destination Host, with a minimum gap and the Listen Port digit band
    ending above the Destination Host input."""

    def test_measured_orders_pass(self):
        for boxes in (NEW_ORDER, OLD_ORDER):
            with self.subTest(listen=boxes["Listen Port"]):
                module.require_editor_row_order(dict(boxes))

    def test_listen_band_stays_above_destination_host_input(self):
        # Destination Host input measured 367..389 (new), 322..344 (old).
        for boxes, dest_input_top in ((NEW_ORDER, 367), (OLD_ORDER, 322)):
            band = module.listen_port_row_region(boxes["Listen Port"], 1280)
            self.assertLess(band[3], dest_input_top, band)

    def test_swapped_listen_and_destination_host_raise(self):
        boxes = dict(NEW_ORDER)
        boxes["Listen Port"], boxes["Destination Host"] = (
            (574, 371, 644, 375), (573, 336, 680, 340))
        # Before this check nothing else looked at the order: the band is
        # derived from wherever "Listen Port" was located, without error.
        module.listen_port_row_region(boxes["Listen Port"], 1280)
        with self.assertRaisesRegex(RuntimeError, r"'Listen Port' top y=371.*'Destination Host' top y=336"):
            module.require_editor_row_order(boxes)

    def test_session_below_listen_raises(self):
        boxes = dict(NEW_ORDER)
        boxes["Session / profile"] = (573, 350, 687, 357)
        with self.assertRaisesRegex(RuntimeError, "Session / profile"):
            module.require_editor_row_order(boxes)

    def test_equal_tops_raise(self):
        boxes = dict(NEW_ORDER)
        boxes["Destination Host"] = (573, 336, 680, 340)
        with self.assertRaisesRegex(RuntimeError, "Destination Host"):
            module.require_editor_row_order(boxes)

    def test_overlapping_boxes_raise(self):
        boxes = dict(NEW_ORDER)
        # Tall Listen box reaching into the next label (gap 22 >= 20).
        boxes["Listen Port"] = (574, 336, 644, 365)
        boxes["Destination Host"] = (573, 358, 680, 362)
        with self.assertRaisesRegex(RuntimeError, "not overlap"):
            module.require_editor_row_order(boxes)

    def test_small_gap_raises(self):
        boxes = dict(NEW_ORDER)
        boxes["Destination Host"] = (573, 350, 680, 354)
        with self.assertRaisesRegex(RuntimeError, "Destination Host"):
            module.require_editor_row_order(boxes)

    def test_band_reaching_destination_input_raises(self):
        # Ordered with the minimum gap, but rows only 25 px apart: the
        # Listen Port band (cy+23) would reach the Destination Host input.
        boxes = dict(NEW_ORDER)
        boxes["Destination Host"] = (573, 361, 680, 365)
        with self.assertRaisesRegex(RuntimeError, "digit band"):
            module.require_editor_row_order(boxes)

    def test_missing_label_raises(self):
        for name in module.TUNNEL_EDITOR_ROW_ORDER:
            with self.subTest(missing=name):
                boxes = dict(NEW_ORDER)
                del boxes[name]
                with self.assertRaisesRegex(RuntimeError, "not located"):
                    module.require_editor_row_order(boxes)
        boxes = dict(NEW_ORDER)
        boxes["Destination Host"] = None
        with self.assertRaisesRegex(RuntimeError, "not located"):
            module.require_editor_row_order(boxes)

    def test_every_listen_locate_for_a_required_check_gates_order(self):
        source = SOURCE.read_text(encoding="utf-8")
        # Two dark connected sites (editor selection loop, listen_label_for)
        # and the light tour.
        self.assertEqual(source.count("await require_located_editor_row_order("), 2)
        self.assertEqual(source.count("require_editor_row_order(order_boxes)"), 1)


def _swap_rows(src: Path, dst: Path, top: int, pitch: int) -> None:
    """Write ``src`` with the pixel stripes [top, top+pitch) and
    [top+pitch, top+2*pitch) exchanged (labels and inputs move together)."""
    width, height, channels, rows = module._read_png(src)
    rgb = [bytes(b for i, b in enumerate(row) if channels == 3 or i % 4 != 3) for row in rows]
    a = rgb[top:top + pitch]
    b = rgb[top + pitch:top + 2 * pitch]
    rgb[top:top + 2 * pitch] = b + a
    module._write_png(dst, width, height, b"".join(rgb))


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(NEW_EDITOR.is_file(), "new-layout editor fixture missing")
class SwappedRowsFrameTest(unittest.TestCase):
    """A real frame with the Listen Port and Destination Host rows swapped:
    the exact digit read the probes already did still accepts it; the
    order gate rejects it."""

    def setUp(self):
        module._OCR_FAILURES.clear()
        module._LOCATE_CUT_SHORT.clear()
        module._PIXEL_CACHE.clear()
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def test_swapped_frame_passes_digit_read_but_fails_order(self):
        swapped = self.tmp / "editor-swapped-listen-desthost.png"
        # Listen Port row cy 338, Destination Host cy 373: 35 px stripes
        # from cy-17 (Session input ends at 319, Destination Port row
        # input starts at 402).
        _swap_rows(NEW_EDITOR, swapped, 321, 35)
        region = module.tunnel_editor_label_region(NEW_HEADING, 1280, 800)
        boxes = {name: module.locate_visible_label(swapped, name, region=region)
                 for name in module.TUNNEL_EDITOR_ROW_ORDER}
        print(f"swapped frame labels: {boxes}", flush=True)
        self.assertGreater(boxes["Listen Port"][1], boxes["Destination Host"][1])
        digits, _box = module.read_port_digits(
            swapped, module.listen_port_row_region(boxes["Listen Port"], 1280))
        # require_editor_selection's row check is digits == str(port).
        self.assertEqual(digits, "52241")
        with self.assertRaisesRegex(RuntimeError, "out of order"):
            module.require_editor_row_order(boxes)
