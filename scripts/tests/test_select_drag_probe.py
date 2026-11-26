"""Pure helpers behind the required connected drag-selection probe."""
from __future__ import annotations

import ast
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_select", SOURCE)
gui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gui)


def logged_echo_lines() -> tuple:
    tree = ast.parse(SOURCE.read_text(encoding="utf-8"))
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == "LOGGED_ECHO_LINES" for t in node.targets
        ):
            return ast.literal_eval(node.value)
    raise AssertionError("LOGGED_ECHO_LINES not found")


class ProbeDesignTest(unittest.TestCase):
    def test_reply_and_target_column(self):
        reply = gui.select_probe_reply()
        self.assertEqual(reply, "NATIVE_ECHO:SELROW DRAGPROBE PAD TO COL42 echo:SEL_OK")
        col = gui.reply_column(reply, gui.SELECT_PROBE_TARGET)
        self.assertEqual(col, 42)
        self.assertGreaterEqual(col, 40)
        # Target ends the line: last cell is column 52, the final column.
        self.assertEqual(col + len(gui.SELECT_PROBE_TARGET) - 1, len(reply) - 1)
        self.assertEqual(len(reply), 53)

    def test_anchor_is_one_word_at_column_19(self):
        reply = gui.select_probe_reply()
        self.assertEqual(gui.reply_column(reply, gui.SELECT_PROBE_ANCHOR), 19)
        self.assertIn(f" {gui.SELECT_PROBE_ANCHOR} ", reply)

    def test_both_probe_commands_are_allowlisted(self):
        lines = logged_echo_lines()
        self.assertIn(gui.SELECT_PROBE_LINE, lines)
        self.assertIn(gui.SELECT_PROBE_TARGET, lines)
        self.assertEqual(gui.LOGGED_ECHO_LINES, lines)

    def test_reply_column_rejects_repeats(self):
        with self.assertRaises(ValueError):
            gui.reply_column("ab ab", "ab")


class CellGeometryTest(unittest.TestCase):
    def test_cell_x_bounds(self):
        self.assertAlmostEqual(gui.cell_x(0, 0), 10.0)
        self.assertAlmostEqual(gui.cell_x(42, 0), 10 + 8.4 * 42)
        self.assertAlmostEqual(gui.cell_x(42, 1), 10 + 8.4 * 43)
        self.assertAlmostEqual(gui.cell_x(1, 0, pane_left=641), 641 + 10 + 8.4)

    def test_points_for_target_cells(self):
        press, release = gui.drag_select_points(42, 52)
        self.assertEqual((press, release), (368, 450))
        self.assertEqual(gui.x_to_col(press), 42)
        self.assertEqual(gui.x_to_col(release), 52)
        # Right half of the first cell, left half of the last.
        self.assertGreater(press, gui.cell_x(42, .5))
        self.assertLess(release, gui.cell_x(52, .5))

    def test_old_8px_cell_maps_press_and_release_elsewhere(self):
        press, release = gui.drag_select_points(42, 52)
        self.assertEqual(gui.x_to_col(press, cell_w=gui.OLD_TERMINAL_CELL_W), 44)
        self.assertEqual(gui.x_to_col(release, cell_w=gui.OLD_TERMINAL_CELL_W), 55)

    def test_half_cell_rounding_rule_would_be_off_by_one(self):
        # A rule that rounds to the nearest cell boundary starts one late
        # and stops one early for these points.
        press, release = gui.drag_select_points(42, 52)
        self.assertEqual(round((press - 10) / 8.4), 43)
        self.assertEqual(round((release - 10) / 8.4) - 1, 51)

    def test_points_raise_when_rounding_leaves_cell(self):
        with self.assertRaises(ValueError):
            gui.drag_select_points(0, 0, cell_w=1.0)


class MismatchTest(unittest.TestCase):
    def test_exact_and_probe_lines_are_not_misses(self):
        self.assertIsNone(gui.selection_paste_mismatch("echo:SEL_OK"))
        self.assertIsNone(gui.selection_paste_mismatch(gui.SELECT_PROBE_LINE))
        self.assertIsNone(gui.selection_paste_mismatch("echo:TERMINAL_OWNED"))

    def test_near_misses_are_named(self):
        for line in ("cho:SEL_OK", "ho:SEL_OK", "echo:SEL_O", " echo:SEL_OK",
                     "2 echo:SEL_OK"):
            self.assertEqual(
                gui.selection_paste_mismatch(line),
                f"SELECTION_PASTE_MISMATCH {line!r}",
            )

    def test_paste_lines_parse(self):
        log = "SESSION_OPEN\nSELECTION_PASTE_LINE 'echo:SEL_OK'\nX\nSELECTION_PASTE_LINE 'S'\n"
        self.assertEqual(gui.selection_paste_lines(log), ["'echo:SEL_OK'", "'S'"])


if __name__ == "__main__":
    unittest.main()
