"""Per-forward toggle click targeting in the Tunnels quick menu.

Ubuntu native preview run 45110697617 logged ``tunnel toggle start-b: Bravo
forward row (1014,143,1105,147) -> (1157,152) via pixels``: the old long-band
scan sampled the row background on a subtitle glyph, merged the band into
foreground and picked a 12 px subtitle run, so the Bravo switch (x~1251) was
never clicked and the listener never echoed. ``connected-tunnels-start-b-
before-ci.png`` is that exact CI screenshot; the ``-local`` fixture is the
local populated run, where the old scan fell back to geometry.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
FIXTURES = Path(__file__).resolve().parent / "fixtures"
spec = importlib.util.spec_from_file_location("iced_connected_gui", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

WIDTH = 1280
CI_B = FIXTURES / "connected-tunnels-start-b-before-ci.png"
CI_A = FIXTURES / "connected-tunnels-start-a-before-ci.png"
LOCAL_B = FIXTURES / "connected-tunnels-start-b-before-local.png"


def old_long_band_scan(path: Path, label_box, window_width: int):
    """Frozen copy of the pre-fix selection, kept to prove the CI fixture
    still reproduces the wrong click."""
    width, height, channels, rows = module._read_png(path)
    y1, y2 = max(0, label_box[1] - 2), min(height, label_box[3] + 16)
    x1, x2 = min(width - 1, label_box[2] + 8), min(width, window_width - 6)
    cy = (label_box[1] + label_box[3]) // 2 + 7
    columns = []
    for x in range(x1, x2):
        for y in range(y1, y2):
            bg = rows[y][x1 * channels: x1 * channels + 3]
            px = rows[y][x * channels: x * channels + 3]
            if sum(abs(a - b) for a, b in zip(px, bg)) > 60:
                columns.append(x)
                break
    runs = []
    for x in columns:
        if runs and x - runs[-1][1] <= 2:
            runs[-1][1] = x
        else:
            runs.append([x, x])
    for left, right in reversed(runs):
        if 10 <= right - left + 1 <= 60:
            return ((left + right) // 2, cy), "pixels"
    return (window_width - 29, cy), "geometry"


class TogglerTargetTest(unittest.TestCase):
    def assert_in_switch(self, target, cy):
        (x, y), _source = target
        left, right = module.toggler_slot(WIDTH)
        self.assertEqual((left, right), (WIDTH - 43, WIDTH - 15))
        self.assertTrue(left <= x <= right, f"x={x} outside switch {left}..{right}")
        self.assertLessEqual(abs(x - (WIDTH - 29)), 3)
        self.assertNotEqual(x, 1157)
        self.assertEqual(y, cy)

    def test_ci_fixture_reproduces_old_subtitle_click(self):
        (x, _y), source = old_long_band_scan(CI_B, (1014, 143, 1105, 147), WIDTH)
        self.assertEqual((x, source), (1157, "pixels"))

    def test_ci_bravo_click_lands_on_right_hand_switch(self):
        target = module.toggler_target(CI_B, (1014, 143, 1105, 147), WIDTH)
        self.assert_in_switch(target, 152)
        self.assertEqual(target[1], "pixels")

    def test_ci_alpha_click_lands_on_right_hand_switch(self):
        self.assert_in_switch(module.toggler_target(CI_A, (1013, 87, 1105, 94), WIDTH), 97)

    def test_local_populated_bravo_click_lands_on_right_hand_switch(self):
        target = module.toggler_target(LOCAL_B, (1014, 142, 1106, 147), WIDTH)
        self.assert_in_switch(target, 151)
        self.assertEqual(target[1], "pixels")

    def test_row_without_switch_falls_back_to_slot_centre(self):
        # Alpha's title row band on the closed-popup fixture has no switch.
        closed = FIXTURES / "connected-tunnels-closed.png"
        (x, _y), source = module.toggler_target(closed, (1013, 87, 1105, 94), WIDTH)
        self.assertEqual((x, source), (WIDTH - 29, "geometry"))


if __name__ == "__main__":
    unittest.main()
