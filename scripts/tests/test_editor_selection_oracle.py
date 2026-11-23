"""Tunnel editor selection must be proven by the form, not the list label.

The list row label ("Alpha forward") is rendered whether or not the click
selected anything, so the oracle requires the selected forward's unique
listen port inside the right-hand form and the sibling's port absent. A
no-op click (form unchanged after clicking Bravo) must fail.
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

    def test_port_only_in_left_list_fails(self):
        # Port rendered outside the right-hand form (e.g. list subtitle).
        loc = fake_locator({"41002": (100, 200, 160, 215)})
        with self.assertRaises(RuntimeError):
            module.require_editor_selection(
                Path("x.png"), 41002, 41001, FORM, locate=loc,
            )

    def test_editor_form_region_is_right_half(self):
        self.assertEqual(module.editor_form_region(1280, 800)[0], 640)
        self.assertEqual(module.editor_form_region(960, 640),
                         (480, 42, 960, 640))


if __name__ == "__main__":
    unittest.main()
