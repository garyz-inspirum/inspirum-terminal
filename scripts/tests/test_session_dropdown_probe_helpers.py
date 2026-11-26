"""Helpers behind the Tunnel Settings session-dropdown GUI probe.

An empty Iced text_input draws its placeholder, so "+ opened an empty
draft" is proven positively: the strict digit reader refuses the row AND
the row's ink is the placeholder (placeholder colour, placeholder width).
Pinned on the real run-4 host frame of the new draft
(``connected-tunnels-dropdown-draft-default-host.png``) and on a filled
frame (``...-52241-host.png``), which must fail naming its digits.
"""
from __future__ import annotations

import importlib.util
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_dropdown", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

FIXTURES = Path(__file__).resolve().parent / "fixtures"
DRAFT = FIXTURES / "connected-tunnels-dropdown-draft-default-host.png"
FILLED = FIXTURES / "connected-tunnels-editor-selected-alpha-52241-host.png"
# Label OCR boxes from the GUI image (tesseract) on DRAFT.
SESSION_LABEL = (574, 299, 687, 306)
LISTEN_LABEL = (574, 334, 644, 338)
DEST_HOST_LABEL = (573, 369, 680, 373)
DEST_PORT_LABEL = (573, 404, 680, 408)
FILLED_LISTEN_LABEL = (574, 336, 644, 340)


def row(label):
    return module.editor_value_row_region(label, 1280)


def lumas(path):
    """``luma(x, y)`` over one decode of ``path``."""
    _w, _h, channels, rows = module._read_png(Path(path))

    def luma(x, y):
        r, g, b = rows[y][x * channels:x * channels + 3]
        return (r * 299 + g * 587 + b * 114) // 1000
    return luma


@unittest.skipUnless(DRAFT.is_file() and FILLED.is_file(), "dropdown fixtures missing")
class EmptyPortFieldTest(unittest.TestCase):
    def test_reader_stays_strict_on_placeholder(self):
        # The failure that motivated the helper: "Port" is not digits.
        with self.assertRaisesRegex(RuntimeError, "not a clear digit string"):
            module.read_port_digits(DRAFT, row(LISTEN_LABEL))

    def test_empty_listen_port_shows_placeholder(self):
        # "Port" ink 726..754 x 336..344, peak luma 170 (typed text 191).
        self.assertEqual(module.require_empty_port_field(DRAFT, row(LISTEN_LABEL)),
                         (726, 336, 755, 345))

    def test_empty_destination_fields(self):
        self.assertEqual(module.require_empty_port_field(DRAFT, row(DEST_PORT_LABEL)),
                         (726, 406, 755, 415))
        self.assertEqual(
            module.require_empty_port_field(DRAFT, row(DEST_HOST_LABEL), placeholder="Destination host"),
            (726, 369, 849, 380),
        )

    def test_filled_listen_port_fails_naming_digits(self):
        with self.assertRaisesRegex(RuntimeError, "holds digits '52241'"):
            module.require_empty_port_field(FILLED, row(FILLED_LISTEN_LABEL))

    def test_value_coloured_text_is_not_a_placeholder(self):
        # Session field: the default profile name in the value colour.
        with self.assertRaisesRegex(RuntimeError, r"value-coloured text.*peak luma 191"):
            module.require_empty_port_field(DRAFT, row(SESSION_LABEL))

    def test_placeholder_width_must_match(self):
        # Destination host placeholder is not the 4-glyph "Port".
        with self.assertRaisesRegex(RuntimeError, "px wide, 'Port'"):
            module.require_empty_port_field(DRAFT, row(DEST_HOST_LABEL))

    def test_blank_row_fails(self):
        # Right of "Not saved" in the Status (forward) row: no input at all.
        with self.assertRaisesRegex(RuntimeError, "is blank: no 'Port' placeholder"):
            module.require_empty_port_field(DRAFT, (850, 425, 1040, 461))

    def test_digits_from_reader_seam_fail(self):
        with self.assertRaisesRegex(RuntimeError, "holds digits '7'"):
            module.require_empty_port_field(DRAFT, row(LISTEN_LABEL),
                                            read_digits=lambda _p, _r: ("7", (0, 0, 1, 1)))


@unittest.skipUnless(DRAFT.is_file(), "dropdown fixture missing")
class SessionMenuRegionTest(unittest.TestCase):
    def test_menu_region_starts_below_field(self):
        region = module.session_menu_region(SESSION_LABEL, 1280, 800)
        self.assertEqual(region, (691, 319, 1040, 569))
        # Field 295..317: text (value or search query) ends at 309, the
        # bottom border is 317; the menu border is 318, option ink ~322.
        self.assertGreater(region[1], 317)
        self.assertLessEqual(region[1], 322 - 3)
        luma = lumas(DRAFT)
        self.assertGreaterEqual(luma(720, 317), 120)  # field bottom border
        for y in range(region[1] - 9, region[1] - 1):
            self.assertLess(max(luma(x, y) for x in range(721, 1020)), 150, y)

    def test_menu_region_clamped_to_window(self):
        self.assertEqual(module.session_menu_region(SESSION_LABEL, 1280, 500)[3], 500)


if __name__ == "__main__":
    unittest.main()
