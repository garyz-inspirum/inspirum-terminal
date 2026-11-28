"""Helpers behind the Sessions-pane Edit Profile GUI probe.

The probe right-clicks the populated tunnel profile, chooses Edit, and
clicks "Save changes" (primary fill right of "Save only") while Alpha
runs. A refused Submit must leave the profile file byte-identical, keep
the dialog open and add danger-red error text; an idle sibling edit must
persist. Each verdict/geometry helper is pinned here with negative
controls so a missed click or an accepted write cannot pass.
"""
from __future__ import annotations

import ast
import importlib.util
import json
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_profile", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
CLOSED = FIXTURES / "connected-tunnels-closed.png"
EDITOR = FIXTURES / "connected-tunnels-editor-selected-alpha.png"

BG = (30, 30, 30)
BLUE = (33, 150, 243)
RED = (255, 92, 92)
AMBER = (246, 184, 75)
DIMMED_RED = (166, 60, 60)
W, H = 1280, 800
# "Save only" OCR box on the synthetic button row.
SAVE_ONLY = (760, 700, 822, 712)


def frame(boxes) -> Path:
    """Synthetic 1280x800 RGB frame: BG plus filled ``(box, colour)``."""
    pixels = bytearray(bytes(BG) * (W * H))
    for (x1, y1, x2, y2), colour in boxes:
        for y in range(y1, y2):
            for x in range(x1, x2):
                at = (y * W + x) * 3
                pixels[at:at + 3] = bytes(colour)
    path = Path(tempfile.mkstemp(suffix=".png")[1])
    module._write_png(path, W, H, bytes(pixels))
    return path


class DialogGeometryTest(unittest.TestCase):
    def test_region_is_centred_600px_dialog(self):
        self.assertEqual(module.connection_dialog_region(1280, 800), (336, 32, 944, 778))
        x1, _y1, x2, y2 = module.connection_dialog_region(960, 640)
        self.assertEqual((x1, x2, y2), (176, 784, 618))

    def test_local_forward_click_inside_dialog_below_label(self):
        label = (360, 400, 470, 410)
        x, y = module.local_forward_input_centre(label, 1280)
        left, _t, right, _b = module.connection_dialog_region(1280, 800)
        self.assertTrue(left < x < right - 30, x)
        self.assertTrue(label[3] + 12 <= y <= label[3] + 36, y)


class DialogScrollTest(unittest.TestCase):
    def test_failing_ci_frame_drags_thumb_to_bottom_of_dialog(self):
        shot = Path("/mnt/data/atlas/it14-fe89d61-ci/connected/connected-profile-rename-before-submit.png")
        if not shot.is_file():
            self.skipTest("CI screenshot artifact unavailable")
        start, end = module.connection_scroll_drag(shot, module.connection_dialog_region(W, H))
        self.assertEqual(start, (922, 243))
        self.assertEqual(end[0], start[0])
        self.assertGreater(end[1], 700)
        self.assertLess(end[1], H - 22)

    def test_synthetic_thumb_drags_into_footer(self):
        shot = frame([((917, 80, 927, 746), (55, 55, 55)),
                      ((917, 123, 927, 363), (81, 81, 81))])
        self.addCleanup(shot.unlink)
        start, end = module.connection_scroll_drag(shot, module.connection_dialog_region(W, H))
        self.assertEqual(start, (922, 243))
        self.assertEqual(end, (922, 734))

    def test_missing_thumb_fails_closed(self):
        shot = frame([])
        self.addCleanup(shot.unlink)
        with self.assertRaisesRegex(RuntimeError, "scrollbar thumb"):
            module.connection_scroll_drag(shot, module.connection_dialog_region(W, H))


class DangerPixelsTest(unittest.TestCase):
    def setUp(self):
        self.region = module.connection_dialog_region(W, H)

    def test_counts_danger_text(self):
        path = frame([((400, 690, 420, 694), RED)])
        self.addCleanup(path.unlink)
        self.assertEqual(module.danger_text_pixels(path, self.region), 80)

    def test_amber_warning_and_dimmed_red_are_not_danger(self):
        path = frame([((400, 600, 500, 610), AMBER), ((400, 650, 500, 660), DIMMED_RED),
                      ((800, 600, 900, 624), BLUE)])
        self.addCleanup(path.unlink)
        self.assertEqual(module.danger_text_pixels(path, self.region), 0)

    def test_red_outside_region_is_ignored(self):
        path = frame([((10, 690, 60, 700), RED)])
        self.addCleanup(path.unlink)
        self.assertEqual(module.danger_text_pixels(path, self.region), 0)

    @unittest.skipUnless(CLOSED.is_file(), "fixture missing")
    def test_real_dark_frame_without_error_is_below_threshold(self):
        count = module.danger_text_pixels(CLOSED, self.region)
        self.assertLess(count, module.PROFILE_REFUSAL_MIN_RED, count)


class SubmitButtonTest(unittest.TestCase):
    def test_primary_fill_right_of_save_only(self):
        path = frame([((840, 694, 950, 718), BLUE)])
        self.addCleanup(path.unlink)
        found = module.connection_submit_button(path, SAVE_ONLY, W)
        self.assertIsNotNone(found)
        (x, y), box = found
        self.assertEqual(box, (840, 694, 950, 718))
        self.assertEqual((x, y), (895, 706))

    def test_no_fill_is_none(self):
        path = frame([])
        self.addCleanup(path.unlink)
        self.assertIsNone(module.connection_submit_button(path, SAVE_ONLY, W))

    def test_selected_toggle_on_another_row_is_not_submit(self):
        # flat_selected "Known hosts only" sits far above the button row.
        path = frame([((840, 300, 950, 324), BLUE)])
        self.addCleanup(path.unlink)
        self.assertIsNone(module.connection_submit_button(path, SAVE_ONLY, W))

    def test_fill_left_of_save_only_is_not_submit(self):
        path = frame([((600, 694, 740, 718), BLUE)])
        self.addCleanup(path.unlink)
        self.assertIsNone(module.connection_submit_button(path, SAVE_ONLY, W))

    @unittest.skipUnless(EDITOR.is_file(), "fixture missing")
    def test_tunnel_manager_save_is_not_profile_submit(self):
        # Real frame with a primary Save at 984..1035 x 661..684; an anchor
        # on another row must not borrow it.
        self.assertIsNone(module.connection_submit_button(EDITOR, (700, 400, 760, 412), W))


class RefusalVerdictTest(unittest.TestCase):
    ok = dict(file_before=b"[1]", file_after=b"[1]", title_visible=True,
              red_before=0, red_after=300, context="t")

    def test_refusal_passes(self):
        module.profile_edit_refused(**self.ok)

    def test_red_written_file_fails(self):
        with self.assertRaisesRegex(RuntimeError, "profile file"):
            module.profile_edit_refused(**{**self.ok, "file_after": b"[2]"})

    def test_red_dialog_closed_fails(self):
        with self.assertRaisesRegex(RuntimeError, "closed"):
            module.profile_edit_refused(**{**self.ok, "title_visible": False})

    def test_red_missed_click_fails(self):
        # File unchanged and dialog open, but no fresh error text.
        with self.assertRaisesRegex(RuntimeError, "no inline refusal"):
            module.profile_edit_refused(**{**self.ok, "red_before": 120, "red_after": 130})


class ProfileNamesTest(unittest.TestCase):
    def test_list_and_wrapped_layouts(self):
        for payload in ([{"name": "a"}, {"name": "b"}], {"sessions": [{"name": "a"}, {"name": "b"}]}):
            path = Path(tempfile.mkstemp(suffix=".json")[1])
            self.addCleanup(path.unlink)
            path.write_text(json.dumps(payload), encoding="utf-8")
            self.assertEqual(module.saved_profile_names(path), ["a", "b"])


def function_source(src: str, name: str) -> str:
    """Exact source segment of the single (async) def called `name`."""
    nodes = [node for node in ast.walk(ast.parse(src))
             if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name]
    if len(nodes) != 1:
        raise AssertionError(f"expected one def {name}, found {len(nodes)}")
    return ast.get_source_segment(src, nodes[0])


class ProbeWiringTest(unittest.TestCase):
    src = SOURCE.read_text(encoding="utf-8")

    def body(self) -> str:
        # Exactly profile_edit_probe's own source (via ast), so later sibling
        # probes can neither trip nor satisfy these checks.
        return function_source(self.src, "profile_edit_probe")

    def test_body_is_exactly_profile_edit_probe(self):
        body = self.body()
        self.assertTrue(body.startswith("async def profile_edit_probe("))
        self.assertNotIn("async def session_dropdown_probe", body)

    def test_runs_after_running_edit_delete_probe(self):
        first = self.src.index("await running_edit_delete_probe()")
        self.assertIn("await profile_edit_probe()", self.src[first:first + 120])

    def test_real_sessions_pane_path_and_submit_click(self):
        body = self.body()
        self.assertIn('"click", "3"', body)          # right-click the row
        self.assertIn('"Edit", env', body)           # context menu Edit
        self.assertIn("connection_submit_button", body)
        self.assertIn("profile_edit_refused", body)
        self.assertNotIn('"Return"', body)           # Submit is a click

    def test_scroll_bottom_drags_visible_scrollbar_not_wheels_in_form(self):
        body = function_source(self.src, "scroll_bottom")
        self.assertIn("connection_scroll_drag", body)
        self.assertIn('"mousedown", "1"', body)
        self.assertIn('"mouseup", "1"', body)
        self.assertNotIn('"click", "--repeat"', body)

    def test_switch_stops_alpha_after_refusals_before_sibling_edits(self):
        body = self.body()
        rename = body.index('require_refused(dialog, "rename"')
        spec_change = body.index('require_refused(dialog, "spec"')
        stop = body.index('await stop_alpha("profile-refused-stop")')
        sibling = body.index('"bravo-add"')
        self.assertLess(rename, spec_change)
        self.assertLess(spec_change, stop)
        self.assertLess(stop, sibling)
        self.assertIn('await stop_alpha("profile-final-stop")', body[sibling:])

    def test_each_refusal_keeps_alpha_echoing(self):
        body = self.body()
        for marker in ("GUI_FORWARD_ALPHA_PROFILE_RENAME", "GUI_FORWARD_ALPHA_PROFILE_SPEC"):
            self.assertIn(marker, body)


if __name__ == "__main__":
    unittest.main()
