"""Regression checks for Find close residue and the PTY observation gate."""
import importlib.util
from pathlib import Path
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui_find_close", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

BAND = (100, 200, 400, 240)
PREV = (230, 207, 265, 222)
NEXT = (280, 207, 315, 222)
COUNTER = (165, 207, 190, 222)


class FindClosePredicateTest(unittest.TestCase):
    def test_all_controls_present(self):
        residue = module.find_strip_residue(
            [("Prev", PREV), ("Next", NEXT), ("0/0", COUNTER)], BAND
        )
        self.assertEqual(residue, {"Prev": PREV, "Next": NEXT, "counter": (0, 0, "0/0")})

    def test_missing_prev_does_not_mean_closed(self):
        # RED on the old predicate: Prev vanished while Next and 0/0 remained.
        residue = module.find_strip_residue([("Next", NEXT), ("0/0", COUNTER)], BAND)
        self.assertEqual(residue["Prev"], None)
        self.assertEqual(residue["Next"], NEXT)
        self.assertEqual(residue["counter"], (0, 0, "0/0"))

    def test_only_counter_remains(self):
        residue = module.find_strip_residue([("0/0", COUNTER)], BAND)
        self.assertEqual(residue, {"Prev": None, "Next": None, "counter": (0, 0, "0/0")})

    def test_empty_and_unrelated_words_are_closed(self):
        for words in ([], [("unrelated", PREV)]):
            with self.subTest(words=words):
                self.assertEqual(module.find_strip_residue(words, BAND),
                                 {"Prev": None, "Next": None, "counter": None})

    def test_labels_outside_band_are_ignored(self):
        outside = (230, 241, 265, 255)
        self.assertEqual(module.find_strip_residue(
            [("Prev", outside), ("Next", outside), ("0/0", outside)], BAND
        ), {"Prev": None, "Next": None, "counter": None})


class EventSink:
    def __init__(self):
        self.events = []

    def write(self, marker):
        self.events.append(marker)


class PtyInputGateTest(unittest.TestCase):
    def test_find_phase_observes_harmless_byte(self):
        events = EventSink()
        module.observe_pty_chunk(events, {"phase": "FIND"}, " ")
        self.assertEqual(events.events, ["UNEXPECTED_FIND_PTY_INPUT"])

    def test_unarmed_phase_ignores_harmless_byte(self):
        events = EventSink()
        module.observe_pty_chunk(events, {"phase": None}, " ")
        self.assertEqual(events.events, [])


if __name__ == "__main__":
    unittest.main()
