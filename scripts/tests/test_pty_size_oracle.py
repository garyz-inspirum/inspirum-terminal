"""Pure helpers behind the required connected PTY-size check."""
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
spec = importlib.util.spec_from_file_location("iced_connected_gui_pty", SOURCE)
gui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gui)


class PtyColumnsFitTest(unittest.TestCase):
    def test_owner_case_160_in_1307_px_fails(self):
        # Old backend: floor(1287 / 8) = 160 columns where 153 are drawn.
        self.assertEqual(gui.pty_columns_fit(160, 1307), (False, 153))

    def test_owner_case_153_in_1307_px_passes(self):
        self.assertEqual(gui.pty_columns_fit(153, 1307), (True, 153))

    def test_1280_window_old_157_fails(self):
        self.assertEqual(gui.pty_columns_fit(157, 1280), (False, 150))

    def test_1280_window_150_and_149_pass(self):
        # 1260 / 8.4 is 150 up to float error; 150 must not be rounded to 149.
        self.assertEqual(gui.pty_columns_fit(150, 1280), (True, 150))
        self.assertEqual(gui.pty_columns_fit(149, 1280), (True, 150))

    def test_one_column_too_many_fails(self):
        self.assertEqual(gui.pty_columns_fit(151, 1280), (False, 150))

    def test_needlessly_narrow_fails(self):
        self.assertEqual(gui.pty_columns_fit(148, 1280), (False, 150))
        self.assertEqual(gui.pty_columns_fit(80, 1280), (False, 150))

    def test_split_half_of_1280(self):
        half = (1280 - gui.PANE_GRID_SPACING) / 2
        self.assertEqual(gui.pty_columns_fit(73, half), (True, 73))
        # Old backend: floor(619.5 / 8) = 77.
        self.assertEqual(gui.pty_columns_fit(77, half), (False, 73))


class PtyLogParsingTest(unittest.TestCase):
    LOG = "\n".join([
        "SESSION_OPEN",
        "SESSION_STARTED",
        gui.pty_size_event(80, 24, 1),
        "SHELL_PTY_RESIZE",
        gui.pty_size_event(150, 33, 1),
        gui.pty_size_event(150, 20, 2),
        "PTY_SIZE junk",
        gui.pty_size_event(73, 33, 1),
    ])

    def test_event_format(self):
        self.assertEqual(gui.pty_size_event(150, 33, 7), "PTY_SIZE 150 33 s7")

    def test_latest_per_session(self):
        self.assertEqual(
            gui.latest_pty_sizes(self.LOG), {1: (73, 33, 7), 2: (150, 20, 5)},
        )

    def test_problems(self):
        sizes = gui.latest_pty_sizes(self.LOG)
        self.assertEqual(gui.pty_fit_problems(sizes, [1], 639.5), [])
        problems = gui.pty_fit_problems(sizes, [1, 2, 3], 639.5)
        self.assertEqual(len(problems), 2)
        self.assertIn("s2: 150x20", problems[0])
        self.assertIn("fits 73 columns", problems[0])
        self.assertEqual(problems[1], "s3: no PTY_SIZE")


if __name__ == "__main__":
    unittest.main()
