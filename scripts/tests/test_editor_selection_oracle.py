"""Tunnel editor selection must be proven by the form, not the list label.

The list row label ("Alpha forward") is rendered whether or not the click
selected anything, so the oracle requires the selected forward's unique
listen port inside the right-hand form and the sibling's port absent. A
no-op click (form unchanged after clicking Bravo) must fail.
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_sel", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FORM = (640, 100, 1280, 700)


def fake_locator(visible: dict[str, tuple[int, int, int, int]]):
    def locate(_path, label, *, region=None, budget=None):
        box = visible.get(label)
        if box is None or module._bounds_inside(box, region) is None:
            raise RuntimeError(f"{label!r} not visible")
        return box
    return locate


def fake_reader(digits: str, box=(725, 295, 759, 304)):
    def read(_path, _row):
        return digits, (box if digits else None)
    return read


class EditorSelectionOracleTest(unittest.TestCase):
    def test_selected_port_in_form_passes(self):
        loc = fake_locator({"41001": (700, 200, 760, 215)})
        box = module.require_editor_selection(
            Path("x.png"), 41001, 41002, FORM, locate=loc,
        )
        self.assertEqual(box, (700, 200, 760, 215))

    def test_noop_click_keeps_sibling_port_fails(self):
        # Negative control: clicked Bravo but the form still shows Alpha.
        loc = fake_locator({"41001": (700, 200, 760, 215)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 41002, 41001, FORM, locate=loc,
            )

    def test_both_ports_in_form_fails(self):
        loc = fake_locator({"41001": (700, 200, 760, 215),
                            "41002": (700, 260, 760, 275)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 41002, 41001, FORM, locate=loc,
            )

    def test_port_only_in_left_list_fails(self):
        # Port rendered outside the right-hand form (e.g. list subtitle).
        loc = fake_locator({"41002": (100, 200, 160, 215)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 41002, 41001, FORM, locate=loc,
            )

    def test_editor_form_region_is_the_editor_card(self):
        # Dialog 220..1060 at 1280 wide, editor card from x=557.
        self.assertEqual(module.editor_form_region(1280, 800), (551, 42, 1060, 800))
        # Minimum window: dialog still 840 wide, centred at x=60.
        self.assertEqual(module.editor_form_region(960, 640), (391, 42, 900, 640))

    def test_listen_row_region_excludes_destination_port_row(self):
        heading = (249, 151, 367, 159)   # measured OCR box, 1280x800 frame
        row = module.listen_port_row_region(heading, 1280)
        listen_digits = (725, 295, 759, 304)
        destination_digits = (729, 362, 760, 372)
        name_field = (719, 190, 1023, 213)
        label = (569, 289, 650, 302)
        self.assertIsNotNone(module._bounds_inside(listen_digits, row))
        for other in (destination_digits, name_field, label):
            self.assertIsNone(module._bounds_inside(other, row), other)

    def test_port_in_form_but_not_listen_row_fails(self):
        # The selected port rendered as the destination port (wrong row)
        # satisfies the card region but must not satisfy the row oracle.
        row = module.listen_port_row_region((249, 151, 367, 159), 1280)
        loc = fake_locator({"41002": (729, 362, 760, 372)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 41002, 41001, FORM, row_region=row, locate=loc,
                read_digits=fake_reader(""),
            )

    def test_row_digits_must_equal_port_exactly(self):
        row = module.listen_port_row_region((249, 151, 367, 159), 1280)
        loc = fake_locator({})
        box = module.require_editor_selection(
            Path("x.png"), 47031, 40001, FORM, row_region=row, locate=loc,
            read_digits=fake_reader("47031"),
        )
        self.assertEqual(box, (725, 295, 759, 304))
        # Superstring, prefix and one-digit-off readings are all misses.
        for reading in ("470310", "4703", "47032", "147031", ""):
            with self.assertRaises(RuntimeError, msg=reading):
                module.require_editor_selection(
                    Path("x.png"), 47031, 40001, FORM, row_region=row, locate=loc,
                    read_digits=fake_reader(reading),
                )

    def test_row_ok_but_sibling_elsewhere_in_form_fails(self):
        row = module.listen_port_row_region((249, 151, 367, 159), 1280)
        loc = fake_locator({"40001": (729, 362, 760, 372)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 47031, 40001, FORM, row_region=row, locate=loc,
                read_digits=fake_reader("47031"),
            )


FIXTURE = Path(__file__).resolve().parent / "fixtures" / "connected-tunnels-editor-selected-alpha.png"
# Real 1280x800 populated dark frame: Alpha selected, Listen Port 43231,
# Destination Port 40369 (the echo service), heading OCR top y=151.
HEADING = (249, 151, 367, 159)


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(FIXTURE.is_file(), "editor fixture missing")
class EditorSelectionOnRealFrameTest(unittest.TestCase):
    def setUp(self):
        self.form = module.editor_form_region(1280, 800)
        self.row = module.listen_port_row_region(HEADING, 1280)

    def test_listen_port_digits_read_inside_row(self):
        box = module.require_editor_selection(
            FIXTURE, 43231, 41999, self.form, row_region=self.row,
        )
        self.assertIsNotNone(module._bounds_inside(box, self.row))

    def test_destination_port_cannot_pass_as_selection(self):
        # Negative control: 40369 is visible in the card (Destination Port)
        # so the old card-wide oracle accepted it; the row oracle must not.
        module.locate_visible_label(FIXTURE, "40369", region=self.form)
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                FIXTURE, 40369, 41999, self.form, row_region=self.row,
            )

    def test_visible_sibling_in_card_fails(self):
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                FIXTURE, 43231, 40369, self.form, row_region=self.row,
            )


FIXTURES = Path(__file__).resolve().parent / "fixtures"
# 1280x800 dark frames of the Tunnel Settings editor; the Listen Port input
# shows the port in the name (zoomed visual read). "-ci" = GitHub ubuntu
# runner, "-host" = gsb-dev GUI container. Heading OCR top y=152 on CI
# frames (row 691,276..322) and y=151 on host frames (row 691,275..321).
# The host 45271 frame was saved as gsbdev-52045.png, but its input shows
# 45271 (the 32045 misread frame itself is not available).
CI_HEADING = (249, 152, 367, 160)
HOST_HEADING = (249, 151, 367, 159)
TEMPLATE_FRAMES = {
    "connected-tunnels-edit-alpha-draft-47031-ci.png": (CI_HEADING, "47031"),
    "connected-tunnels-edit-alpha-draft-58881-ci.png": (CI_HEADING, "58881"),
    "connected-tunnels-edit-alpha-draft-35649-ci.png": (CI_HEADING, "35649"),
    # Unfocused row (no caret, grey border).
    "connected-tunnels-edit-alpha-selected-38569-ci.png": (CI_HEADING, "38569"),
    "connected-tunnels-edit-bravo-draft-45257-ci.png": (CI_HEADING, "45257"),
    "connected-tunnels-edit-bravo-draft-50835-ci.png": (CI_HEADING, "50835"),
    "connected-tunnels-edit-alpha-draft-45271-host.png": (HOST_HEADING, "45271"),
    "connected-tunnels-editor-selected-alpha.png": (HOST_HEADING, "43231"),
}
# Input boxes are 35 px apart: Listen 287..310, Destination Host 322..345,
# Destination Port 357..380.
ROW_PITCH = 35
W47031 = "connected-tunnels-edit-alpha-draft-47031-ci.png"


def _absent(_path, label, *, region=None, budget=None):
    raise RuntimeError(f"{label} not visible")


def _row(name):
    return module.listen_port_row_region(TEMPLATE_FRAMES[name][0], 1280)


class ListenPortTemplateReaderTest(unittest.TestCase):
    """Template reader on real frames. Pure Python: no tesseract/Pillow."""

    form = module.editor_form_region(1280, 800)

    def test_each_frame_reads_exactly_its_port(self):
        for name, (_heading, port) in TEMPLATE_FRAMES.items():
            with self.subTest(name=name):
                digits, box = module.read_port_digits(FIXTURES / name, _row(name))
                print(f"read_port_digits {name}: {digits!r} {box}", flush=True)
                self.assertEqual(digits, port)
                self.assertIsNotNone(module._bounds_inside(box, _row(name)))

    def test_leave_one_frame_out_still_reads_frame(self):
        worst, gap = 0.0, float("inf")
        for held, (_heading, port) in TEMPLATE_FRAMES.items():
            others = [(FIXTURES / n, _row(n), d) for n, (_h, d) in TEMPLATE_FRAMES.items() if n != held]
            templates = module.build_port_templates(others)
            with self.subTest(held=held):
                digits, _box = module.read_port_digits(FIXTURES / held, _row(held), templates=templates)
                self.assertEqual(digits, port)
            cells = module._port_glyph_cells(FIXTURES / held, _row(held))
            for (_l, _t, patch), digit in zip(cells, port):
                d = {}
                for t_digit, _src, tpl in templates:
                    d[t_digit] = min(d.get(t_digit, 1e9), module._port_patch_distance(patch, tpl))
                own = d[digit]
                other = min(v for k, v in d.items() if k != digit)
                worst, gap = max(worst, own), min(gap, other - own)
        print(f"leave-one-out worst best distance {worst:.2f}, smallest gap {gap:.2f}", flush=True)
        self.assertLessEqual(worst, module._PORT_MAX_DISTANCE)
        self.assertGreaterEqual(gap, module._PORT_MIN_GAP)

    def test_draft_frames_pass_require_editor_selection(self):
        for name, (_heading, port) in TEMPLATE_FRAMES.items():
            with self.subTest(name=name):
                box = module.require_editor_selection(
                    FIXTURES / name, int(port), 40001, self.form, row_region=_row(name),
                    locate=_absent,
                )
                self.assertIsNotNone(module._bounds_inside(box, _row(name)))

    def test_wrong_port_by_one_digit_fails(self):
        with self.assertRaisesRegex(RuntimeError, "reads '47031'"):
            module.require_editor_selection(
                FIXTURES / W47031, 47032, 40001, self.form, row_region=_row(W47031),
                locate=_absent,
            )

    def test_pre_edit_sibling_port_fails_on_draft_frame(self):
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                FIXTURES / W47031, 40001, 47031, self.form, row_region=_row(W47031),
                locate=_absent,
            )

    def test_destination_rows_do_not_satisfy_listen_port(self):
        x1, y1, x2, y2 = _row(W47031)
        host = (x1, y1 + ROW_PITCH, x2, y2 + ROW_PITCH)
        with self.assertRaisesRegex(RuntimeError, "not a clear digit string|not a port"):
            module.read_port_digits(FIXTURES / W47031, host)
        dest = (x1, y1 + 2 * ROW_PITCH, x2, y2 + 2 * ROW_PITCH)
        digits, _box = module.read_port_digits(FIXTURES / W47031, dest)
        self.assertEqual(digits, "41451")
        for region in (host, dest):
            with self.subTest(region=region), self.assertRaises(RuntimeError):
                module.require_editor_selection(
                    FIXTURES / W47031, 47031, 40001, self.form, row_region=region,
                    locate=_absent,
                )

    def test_empty_row_reads_nothing(self):
        # The Listen Port input to the right of the digits and caret: input
        # fill and border only.
        _x1, y1, x2, y2 = _row(W47031)
        empty = (775, y1, x2, y2)
        self.assertEqual(module.read_port_digits(FIXTURES / W47031, empty), ("", None))
        with self.assertRaisesRegex(RuntimeError, "reads ''"):
            module.require_editor_selection(
                FIXTURES / W47031, 47031, 40001, self.form, row_region=empty, locate=_absent,
            )

    def test_non_digit_glyph_is_rejected(self):
        # Replace the 0 (third glyph) with a synthetic "-" bar: ink, but no digit.
        cells = module._port_glyph_cells(FIXTURES / W47031, _row(W47031))
        bar = tuple(tuple(1.0 if y == 5 and 2 <= x <= 7 else 0.0 for x in range(10))
                    for y in range(11))
        cells[2] = (cells[2][0], cells[2][1], bar)
        digits, report, rejected = module.classify_port_cells(
            cells, module._port_templates_from_table())
        print("non-digit cell report:", " ".join(report), flush=True)
        self.assertTrue(rejected)
        self.assertIn("!", report[2])

    def test_light_row_has_no_templates(self):
        light = FIXTURES / "tour-tunnel-manager-light-empty.png"
        with self.assertRaisesRegex(RuntimeError, "no listen-port digit templates for light theme"):
            module.read_port_digits(light, (300, 200, 600, 240))

    def test_embedded_table_matches_fixture_patches(self):
        # Every embedded template is within quantisation error of the
        # fixture glyph it names, so the table is traceable to the frames.
        for digit, source, tpl in module._port_templates_from_table():
            name, index = source.split(":", 1)[1].rsplit("#", 1)
            cells = module._port_glyph_cells(FIXTURES / name, _row(name))
            self.assertEqual(TEMPLATE_FRAMES[name][1][int(index)], digit)
            self.assertLess(module._port_patch_distance(cells[int(index)][2], tpl), 3.0, source)


if __name__ == "__main__":
    unittest.main()
