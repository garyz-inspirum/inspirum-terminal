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

    def test_sibling_read_timed_out_is_not_absence(self):
        # A tesseract timeout returns no words; that must not certify the
        # sibling port absent (seen on a loaded host: 41 s per crop against
        # the 15 s per-call limit, sibling 40369 "missing" from a frame
        # that renders it).
        shown = fake_locator({"41001": (700, 200, 760, 215)})

        def starved(path, label, *, region=None, budget=None):
            if label == "41002":
                setattr(module, "OCR_TIMEOUTS", getattr(module, "OCR_TIMEOUTS", 0) + 1)
                raise RuntimeError("visible label not found in screenshot: 41002")
            return shown(path, label, region=region, budget=budget)

        with self.assertRaisesRegex(RuntimeError, "cannot prove sibling port 41002 absent"):
            module.require_editor_selection(
                Path("x.png"), 41001, 41002, FORM, locate=starved,
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
        # Must fail BECAUSE 40369 was read in the card. An OCR timeout on a
        # slow host raises the inconclusive error instead, which does not
        # count here (set INSPIRUM_GUI_TIME_SCALE as the GUI container does).
        with self.assertRaisesRegex(RuntimeError, "still shows sibling port 40369"):
            module.require_editor_selection(
                FIXTURE, 43231, 40369, self.form, row_region=self.row,
            )


if __name__ == "__main__":
    unittest.main()
