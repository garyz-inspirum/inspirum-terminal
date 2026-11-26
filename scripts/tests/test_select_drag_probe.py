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
        self.assertEqual(reply, "NATIVE_ECHO:SELROW DRAGPROBE PAD TO COL42 echo:SEL_OK=G")
        col = gui.reply_column(reply, gui.SELECT_PROBE_TARGET)
        self.assertEqual(col, 42)
        self.assertEqual(col + len(gui.SELECT_PROBE_TARGET) - 1, 52)
        # Printable non-space guard right after the target (columns 53..54).
        self.assertEqual(reply[53:], "=G")
        self.assertTrue(reply[53].isprintable() and not reply[53].isspace())
        self.assertEqual(len(reply), 55)

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


class ClassifyPasteTest(unittest.TestCase):
    def test_target_and_sentinel(self):
        self.assertEqual(gui.classify_selection_paste("echo:SEL_OK"), "SELECTION_PASTE_OK")
        self.assertEqual(gui.classify_selection_paste("echo:CLICK_SENTINEL"),
                         "SELECTION_PASTE_SENTINEL")

    def test_empty(self):
        self.assertEqual(gui.classify_selection_paste(""), "SELECTION_PASTE_EMPTY")

    def test_near_misses_name_columns(self):
        cases = {
            "cho:SEL_O": "SELECTION_PASTE_MISMATCH cols 43..51",  # old 8 px bug
            "echo:SEL_O": "SELECTION_PASTE_MISMATCH cols 42..51",
            " echo:SEL_OK": "SELECTION_PASTE_MISMATCH cols 41..52",
            "2 echo:SEL_OK": "SELECTION_PASTE_MISMATCH cols 40..52",
            # One-cell overrun now picks up the guard.
            "echo:SEL_OK=": "SELECTION_PASTE_MISMATCH cols 42..53",
            "cho:SEL_OK=": "SELECTION_PASTE_MISMATCH cols 43..53",
        }
        for line, marker in cases.items():
            self.assertEqual(gui.classify_selection_paste(line), marker, line)

    def test_unknown_never_contains_input(self):
        self.assertEqual(gui.classify_selection_paste("hunter2 secret"),
                         "SELECTION_PASTE_UNKNOWN len=14")
        for line in ("hunter2 secret", "echo:TERMINAL_OWNED", "zz echo:SEL_OK"):
            marker = gui.classify_selection_paste(line)
            self.assertTrue(marker.startswith("SELECTION_PASTE_UNKNOWN len="), line)
            self.assertNotIn(line, marker)

    def test_repeated_substring_is_unknown(self):
        # "E" occurs several times in the reply row: no unique columns.
        self.assertEqual(gui.classify_selection_paste("E"), "SELECTION_PASTE_UNKNOWN len=1")

    def test_markers_parse_in_order(self):
        log = ("SESSION_OPEN\nSELECTION_PASTE_OK\nNATIVE_ECHO:SEL_OK\n"
               "SELECTION_PASTE_MISMATCH cols 43..51\nSELECTION_PASTE_UNKNOWN len=3\n"
               "SELECTION_PASTE_SENTINEL\nSELECTION_PASTE_EMPTY\n")
        self.assertEqual(gui.selection_paste_markers(log), [
            "SELECTION_PASTE_OK", "SELECTION_PASTE_MISMATCH cols 43..51",
            "SELECTION_PASTE_UNKNOWN len=3", "SELECTION_PASTE_SENTINEL",
            "SELECTION_PASTE_EMPTY",
        ])

    def test_fixture_never_logs_received_text(self):
        text = SOURCE.read_text(encoding="utf-8")
        self.assertNotIn("SELECTION_PASTE_" + "LINE", text)
        self.assertNotIn("selection_paste_" + "mismatch(", text)


if __name__ == "__main__":
    unittest.main()
