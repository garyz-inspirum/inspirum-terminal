"""Tunnel editor selection must be proven by the form, not the list label.

The list row label ("Alpha forward") is rendered whether or not the click
selected anything, so the oracle requires the selected forward's unique
listen port inside the right-hand form and the sibling's port absent. A
no-op click (form unchanged after clicking Bravo) must fail.
"""
from __future__ import annotations

import importlib.util
import shutil
import subprocess
import sys
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_sel", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FORM = (640, 100, 1280, 700)

# OCR-located editor label boxes (locate_visible_label in the GUI image) on
# the 1280x800 dark frames. OLD = it11 layout (Session / profile was a
# plain-text row); NEW = combo-box Session / profile row, which moved every
# row below it down by 44 px. Rows are 35 px apart in both layouts.
LISTEN_LABEL = {"old": (574, 292, 644, 296), "new": (574, 336, 644, 340)}
SESSION_LABEL = {"old": (573, 264, 687, 271), "new": (573, 301, 687, 308)}
DEST_HOST_LABEL = {"old": (573, 327, 680, 331), "new": (573, 371, 680, 375)}
DEST_PORT_LABEL = {"old": (573, 362, 680, 366), "new": (573, 406, 680, 410)}


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

    def test_tunnel_list_region_starts_at_the_dialog(self):
        heading = (245, 147, 372, 158)
        self.assertEqual(module.tunnel_list_region(heading, 1280, 800), (233, 42, 640, 800))
        self.assertEqual(module.tunnel_list_region((5, 147, 130, 158), 1280, 800)[0], 0)
        with self.assertRaises(RuntimeError):
            module.tunnel_list_region((600, 147, 727, 158), 1280, 800)

    def test_listen_row_region_excludes_destination_port_row(self):
        # Pixel-measured boxes (_read_png): input borders and digit ink.
        layouts = {
            "old": dict(listen_input=(719, 287, 1023, 310), listen_digits=(725, 294, 765, 303),
                        session=(722, 261, 841, 271), dest_host=(719, 322, 1023, 345),
                        destination_digits=(725, 364, 762, 373), name_field=(719, 190, 1023, 213)),
            "new": dict(listen_input=(719, 332, 1023, 355), listen_digits=(726, 338, 765, 347),
                        session=(719, 297, 1023, 320), dest_host=(719, 367, 1023, 390),
                        destination_digits=(726, 408, 762, 417), name_field=(719, 227, 1023, 250)),
        }
        for layout, boxes in layouts.items():
            with self.subTest(layout=layout):
                row = module.listen_port_row_region(LISTEN_LABEL[layout], 1280)
                self.assertIsNotNone(module._bounds_inside(boxes["listen_input"], row))
                self.assertIsNotNone(module._bounds_inside(boxes["listen_digits"], row))
                self.assertGreater(row[0], LISTEN_LABEL[layout][2], "label ink in the row band")
                for name in ("session", "dest_host", "destination_digits", "name_field"):
                    other = boxes[name]
                    # Not even one pixel row of a neighbour inside the band.
                    self.assertTrue(other[3] <= row[1] or other[1] >= row[3], (layout, name, row))

    def test_label_region_is_the_label_column(self):
        heading = (249, 188, 367, 196)
        region = module.tunnel_editor_label_region(heading, 1280, 800)
        self.assertEqual(region, (551, 188, 717, 800))
        for layout in ("old", "new"):
            for label in (LISTEN_LABEL, SESSION_LABEL, DEST_PORT_LABEL):
                self.assertIsNotNone(module._bounds_inside(label[layout], (551, 42, 717, 800)))
        # Values start at the input column (x >= 719): never inside.
        self.assertIsNone(module._bounds_inside((726, 338, 765, 347), region))

    def test_input_centre_inside_listen_input(self):
        for layout, (y1, y2) in (("old", (287, 310)), ("new", (332, 355))):
            x, y = module.listen_input_centre(LISTEN_LABEL[layout], 1280)
            self.assertTrue(719 < x < 1023 and y1 + 4 <= y <= y2 - 4, (layout, x, y))

    def test_port_in_form_but_not_listen_row_fails(self):
        # The selected port rendered as the destination port (wrong row)
        # satisfies the card region but must not satisfy the row oracle.
        row = module.listen_port_row_region(LISTEN_LABEL["old"], 1280)
        loc = fake_locator({"41002": (729, 362, 760, 372)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 41002, 41001, FORM, row_region=row, locate=loc,
                read_digits=fake_reader(""),
            )

    def test_row_digits_must_equal_port_exactly(self):
        row = module.listen_port_row_region(LISTEN_LABEL["old"], 1280)
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
        row = module.listen_port_row_region(LISTEN_LABEL["old"], 1280)
        loc = fake_locator({"40001": (729, 362, 760, 372)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 47031, 40001, FORM, row_region=row, locate=loc,
                read_digits=fake_reader("47031"),
            )


FIXTURE = Path(__file__).resolve().parent / "fixtures" / "connected-tunnels-editor-selected-alpha.png"
# Real 1280x800 populated dark frame (OLD layout): Alpha selected, Listen
# Port 43231, Destination Port 40369 (the echo service).
FIXTURE_LISTEN_LABEL = LISTEN_LABEL["old"]
# Same step on the NEW layout (build host): Listen Port 52241, Session /
# profile combo box "CI tunnel fixture", Destination Port 36607.
NEW_FIXTURE = Path(__file__).resolve().parent / "fixtures" / "connected-tunnels-editor-selected-alpha-52241-host.png"


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(FIXTURE.is_file(), "editor fixture missing")
class EditorSelectionOnRealFrameTest(unittest.TestCase):
    def setUp(self):
        self.form = module.editor_form_region(1280, 800)
        self.row = module.listen_port_row_region(FIXTURE_LISTEN_LABEL, 1280)

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

    def test_listen_label_located_on_both_layouts(self):
        # The row band is derived from this located label at run time.
        for frame, heading, layout in ((FIXTURE, (249, 151, 367, 159), "old"),
                                       (NEW_FIXTURE, (249, 188, 367, 196), "new")):
            with self.subTest(layout=layout):
                region = module.tunnel_editor_label_region(heading, 1280, 800)
                box = module.locate_visible_label(frame, "Listen Port", region=region)
                print(f"Listen Port label {layout}: {box}", flush=True)
                for got, want in zip(box, LISTEN_LABEL[layout]):
                    self.assertLessEqual(abs(got - want), 2, (box, LISTEN_LABEL[layout]))
                digits, _box = module.read_port_digits(
                    frame, module.listen_port_row_region(box, 1280))
                self.assertEqual(digits, "43231" if layout == "old" else "52241")


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(NEW_FIXTURE.is_file(), "new-layout editor fixture missing")
class EditorSelectionOnNewLayoutFrameTest(unittest.TestCase):
    """Combo-box layout: Destination Port 36607 is visible in the card."""

    def setUp(self):
        _reset_ocr_state()
        self.form = module.editor_form_region(1280, 800)
        self.row = module.listen_port_row_region(LISTEN_LABEL["new"], 1280)

    def test_listen_port_passes_with_absent_sibling(self):
        box = module.require_editor_selection(NEW_FIXTURE, 52241, 61234, self.form,
                                              row_region=self.row)
        self.assertIsNotNone(module._bounds_inside(box, self.row))

    def test_destination_port_cannot_pass_as_selection(self):
        module.locate_visible_label(NEW_FIXTURE, "36607", region=self.form)
        with self.assertRaisesRegex(RuntimeError, "reads '52241'"):
            module.require_editor_selection(NEW_FIXTURE, 36607, 61234, self.form,
                                            row_region=self.row)

    def test_visible_sibling_in_card_fails(self):
        with self.assertRaisesRegex(RuntimeError, "still shows sibling port 36607"):
            module.require_editor_selection(NEW_FIXTURE, 52241, 36607, self.form,
                                            row_region=self.row)


def _reset_ocr_state():
    module._OCR_FAILURES.clear()
    module._LOCATE_CUT_SHORT.clear()
    module._PIXEL_CACHE.clear()


class RequireLabelAbsentTest(unittest.TestCase):
    """Absence needs a completed search; an OCR failure is not absence."""

    def setUp(self):
        _reset_ocr_state()

    def test_clean_miss_is_absent(self):
        self.assertIsNone(module.require_label_absent(
            Path("x.png"), "41001", region=FORM, locate=fake_locator({})))

    def test_found_label_raises_visible(self):
        loc = fake_locator({"41001": (700, 200, 760, 215)})
        with self.assertRaises(RuntimeError) as ctx:
            module.require_label_absent(Path("x.png"), "41001", region=FORM, locate=loc)
        self.assertNotIsInstance(ctx.exception, module.OcrInconclusive)
        self.assertIn("(700, 200, 760, 215)", str(ctx.exception))

    def test_failure_on_every_attempt_is_inconclusive(self):
        calls = []

        def locate(_path, label, *, region=None, budget=None):
            calls.append(label)
            module._OCR_FAILURES.append("tesseract timed out")
            raise RuntimeError("miss")
        with self.assertRaisesRegex(module.OcrInconclusive, "timed out"):
            module.require_label_absent(Path("x.png"), "41001", region=FORM,
                                        locate=locate, attempts=3)
        self.assertEqual(len(calls), 3)

    def test_failure_then_clean_miss_is_absent(self):
        calls = []

        def locate(_path, label, *, region=None, budget=None):
            calls.append(label)
            if len(calls) == 1:
                module._OCR_FAILURES.append("tesseract exited 1")
            raise RuntimeError("miss")
        self.assertIsNone(module.require_label_absent(
            Path("x.png"), "41001", region=FORM, locate=locate))
        self.assertEqual(len(calls), 2)

    def test_budget_cut_miss_is_inconclusive(self):
        def locate(_path, label, *, region=None, budget=None):
            module._LOCATE_CUT_SHORT.append("pixel rows stage skipped by budget")
            raise RuntimeError("miss")
        with self.assertRaisesRegex(module.OcrInconclusive, "budget"):
            module.require_label_absent(Path("x.png"), "41001", region=FORM, locate=locate)


def _fake_run(returncode=None, timeout=False):
    def run(args, **kwargs):
        if timeout:
            raise subprocess.TimeoutExpired(args, kwargs.get("timeout", 1))
        return subprocess.CompletedProcess(args, returncode, "", "fake tesseract failure")
    return run


@unittest.skipUnless(FIXTURE.is_file(), "editor fixture missing")
class SiblingCheckOcrFailureOnRealFrameTest(unittest.TestCase):
    """Reviewer's case: template reader reads 43231, OCR down, 40369 visible.

    Real ``read_port_digits`` and real ``locate_visible_label``; only
    tesseract discovery / ``subprocess.run`` are faked, so no tesseract is
    needed and these run in CI.
    """

    def setUp(self):
        _reset_ocr_state()
        self.form = module.editor_form_region(1280, 800)
        self.row = module.listen_port_row_region(FIXTURE_LISTEN_LABEL, 1280)
        self.assertEqual(module.read_port_digits(FIXTURE, self.row)[0], "43231")

    def _require(self):
        return module.require_editor_selection(
            FIXTURE, 43231, 40369, self.form, row_region=self.row)

    def test_tesseract_unavailable_is_inconclusive_not_pass(self):
        with mock.patch.object(module.shutil, "which", return_value=None):
            with self.assertRaisesRegex(module.OcrInconclusive, "tesseract not found"):
                self._require()

    def test_tesseract_timeout_is_inconclusive_not_pass(self):
        with mock.patch.object(module.shutil, "which", return_value="/fake/tesseract"), \
                mock.patch.object(module.subprocess, "run", _fake_run(timeout=True)):
            with self.assertRaisesRegex(module.OcrInconclusive, "timed out"):
                self._require()

    def test_tesseract_nonzero_exit_is_inconclusive_not_pass(self):
        with mock.patch.object(module.shutil, "which", return_value="/fake/tesseract"), \
                mock.patch.object(module.subprocess, "run", _fake_run(returncode=1)):
            with self.assertRaisesRegex(module.OcrInconclusive, "exited 1"):
                self._require()

    def test_failed_reads_are_not_cached(self):
        for run in (_fake_run(timeout=True), _fake_run(returncode=1)):
            _reset_ocr_state()
            with mock.patch.object(module.shutil, "which", return_value="/fake/tesseract"), \
                    mock.patch.object(module.subprocess, "run", run):
                with self.assertRaises(module.OcrInconclusive):
                    self._require()
            self.assertTrue(module._OCR_FAILURES)
            self.assertEqual(module._PIXEL_CACHE, {})


@unittest.skipUnless(shutil.which("tesseract"), "tesseract is not installed")
@unittest.skipUnless(FIXTURE.is_file(), "editor fixture missing")
class SiblingCheckHealthyOcrOnRealFrameTest(unittest.TestCase):
    def setUp(self):
        _reset_ocr_state()
        self.form = module.editor_form_region(1280, 800)
        self.row = module.listen_port_row_region(FIXTURE_LISTEN_LABEL, 1280)

    def test_visible_sibling_is_found_not_inconclusive(self):
        with self.assertRaisesRegex(RuntimeError, "still shows sibling port 40369") as ctx:
            module.require_editor_selection(
                FIXTURE, 43231, 40369, self.form, row_region=self.row)
        self.assertNotIsInstance(ctx.exception, module.OcrInconclusive)

    def test_healthy_call_after_failed_read_still_finds_sibling(self):
        with mock.patch.object(module.subprocess, "run", _fake_run(returncode=1)):
            with self.assertRaises(module.OcrInconclusive):
                module.require_editor_selection(
                    FIXTURE, 43231, 40369, self.form, row_region=self.row)
        with self.assertRaisesRegex(RuntimeError, "still shows sibling port 40369") as ctx:
            module.require_editor_selection(
                FIXTURE, 43231, 40369, self.form, row_region=self.row)
        self.assertNotIsInstance(ctx.exception, module.OcrInconclusive)

    def test_absent_sibling_passes(self):
        box = module.require_editor_selection(
            FIXTURE, 43231, 61234, self.form, row_region=self.row)
        self.assertIsNotNone(module._bounds_inside(box, self.row))


FIXTURES = Path(__file__).resolve().parent / "fixtures"
# 1280x800 dark frames of the Tunnel Settings editor; the Listen Port input
# shows the port in the name (zoomed visual read). "-ci" = GitHub ubuntu
# runner, "-host" = gsb-dev GUI container. Every frame maps to (rendering,
# layout, port); the row comes from the layout's located Listen Port label
# (OCR on each frame in the GUI image: (574,292,643|644,296) on every old
# frame, (574,336,644,340) on the new one).
# The host 45271 frame was saved as gsbdev-52045.png, but its input shows
# 45271 (the 32045 misread frame itself is not available).
TEMPLATE_FRAMES = {
    "connected-tunnels-edit-alpha-draft-47031-ci.png": ("gh", "old", "47031"),
    "connected-tunnels-edit-alpha-draft-58881-ci.png": ("gh", "old", "58881"),
    "connected-tunnels-edit-alpha-draft-35649-ci.png": ("gh", "old", "35649"),
    # Unfocused row (no caret, grey border).
    "connected-tunnels-edit-alpha-selected-38569-ci.png": ("gh", "old", "38569"),
    "connected-tunnels-edit-bravo-draft-45257-ci.png": ("gh", "old", "45257"),
    "connected-tunnels-edit-bravo-draft-50835-ci.png": ("gh", "old", "50835"),
    "connected-tunnels-edit-alpha-draft-45271-host.png": ("host", "old", "45271"),
    "connected-tunnels-editor-selected-alpha.png": ("host", "old", "43231"),
    # Host frames of two later runs. Ground truth: every glyph clusters with
    # glyphs of the same digit across frames (distance < 4); 1-5 and 7 match
    # the existing host templates, 52045 was read visually, and the run's
    # saved-port pair {58191, 60989} / {41675, 54813} fixes the rest (only
    # 58191 fits glyph pattern A B 1 C 1; 60989 has equal glyphs at 2 and 4).
    "connected-tunnels-edit-bravo-draft-52045-host.png": ("host", "old", "52045"),
    "connected-tunnels-editor-selected-alpha-60989-host.png": ("host", "old", "60989"),
    "connected-tunnels-editor-selected-bravo-58191-host.png": ("host", "old", "58191"),
    "connected-tunnels-editor-selected-bravo-41675-host.png": ("host", "old", "41675"),
    # NEW layout (combo-box Session / profile row), build host. Its
    # Destination Port shows 36607 and the Session row "CI tunnel fixture".
    "connected-tunnels-editor-selected-alpha-52241-host.png": ("host", "new", "52241"),
}
W47031 = "connected-tunnels-edit-alpha-draft-47031-ci.png"
W52241 = "connected-tunnels-editor-selected-alpha-52241-host.png"


def _absent(_path, label, *, region=None, budget=None):
    raise RuntimeError(f"{label} not visible")


def _row(name):
    return module.listen_port_row_region(LISTEN_LABEL[TEMPLATE_FRAMES[name][1]], 1280)


def _port(name):
    return TEMPLATE_FRAMES[name][2]


class ListenPortTemplateReaderTest(unittest.TestCase):
    """Template reader on real frames. Pure Python: no tesseract/Pillow."""

    form = module.editor_form_region(1280, 800)

    def test_each_frame_reads_exactly_its_port(self):
        for name, (_env, _layout, port) in TEMPLATE_FRAMES.items():
            with self.subTest(name=name):
                digits, box = module.read_port_digits(FIXTURES / name, _row(name))
                print(f"read_port_digits {name}: {digits!r} {box}", flush=True)
                self.assertEqual(digits, port)
                self.assertIsNotNone(module._bounds_inside(box, _row(name)))

    def test_host_52045_draft_reads_exactly(self):
        # The frame that failed the full build-host run (0 had no host template).
        name = "connected-tunnels-edit-bravo-draft-52045-host.png"
        digits, _box = module.read_port_digits(FIXTURES / name, _row(name))
        self.assertEqual(digits, "52045")

    def test_template_table_covers_every_digit_in_both_renderings(self):
        table = module._PORT_TEMPLATE_TABLE
        for env in ("gh", "host"):
            with self.subTest(env=env):
                covered = {digit for digit, e, _src, _rows in table if e == env}
                self.assertEqual(covered, set("0123456789"), f"{env} templates missing digits")
        for env in ("gh", "host"):
            covered = {d for _n, (e, _layout, port) in TEMPLATE_FRAMES.items() for d in port
                       if e == env}
            self.assertEqual(covered, set("0123456789"), f"{env} fixture frames missing digits")

    def test_leave_one_frame_out_still_reads_frame(self):
        worst, gap = 0.0, float("inf")
        for held, (_env, _layout, port) in TEMPLATE_FRAMES.items():
            others = [(FIXTURES / n, _row(n), _port(n)) for n in TEMPLATE_FRAMES if n != held]
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
        for name, (_env, _layout, port) in TEMPLATE_FRAMES.items():
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
        # Neighbouring rows come from their own located labels, per layout,
        # through the same band rule as the Listen Port row.
        for name, layout, dest_port in ((W47031, "old", "41451"), (W52241, "new", "36607")):
            frame, port = FIXTURES / name, int(_port(name))
            host = module.editor_value_row_region(DEST_HOST_LABEL[layout], 1280)
            dest = module.editor_value_row_region(DEST_PORT_LABEL[layout], 1280)
            session = module.editor_value_row_region(SESSION_LABEL[layout], 1280)
            with self.subTest(name=name, row="host"):
                with self.assertRaisesRegex(RuntimeError, "not a clear digit string|not a port"):
                    module.read_port_digits(frame, host)
            with self.subTest(name=name, row="dest"):
                digits, _box = module.read_port_digits(frame, dest)
                print(f"read_port_digits {name} Destination Port row {dest}: {digits!r}", flush=True)
                self.assertEqual(digits, dest_port)
            with self.subTest(name=name, row="session"):
                # Old: plain text; new: combo box showing "CI tunnel fixture".
                with self.assertRaisesRegex(RuntimeError, "not a clear digit string|not a port"):
                    module.read_port_digits(frame, session)
            for region in (host, dest, session):
                with self.subTest(name=name, region=region), self.assertRaises(RuntimeError):
                    module.require_editor_selection(
                        frame, port, 40001, self.form, row_region=region,
                        locate=_absent,
                    )

    def test_heading_offset_band_fails_on_new_layout(self):
        # The pre-fix band (heading top 184 + (124, 170)) on the new frame
        # covers the Session combo box: the strict reader refuses it, which
        # is the failure the full run hit. Kept as evidence the reader did
        # not have to be loosened.
        with self.assertRaisesRegex(RuntimeError, "ink is 39 px tall"):
            module.read_port_digits(FIXTURES / W52241, (691, 308, 1040, 354))

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
            self.assertEqual(_port(name)[int(index)], digit)
            self.assertLess(module._port_patch_distance(cells[int(index)][2], tpl), 3.0, source)


if __name__ == "__main__":
    unittest.main()
