"""Fixture profile metadata for INSPIRUM_GUI_AUTOSTART=1.

Alpha alone opts in to auto-start; Bravo and every default/populated run
keep the product default (field omitted == off).
"""
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_gui", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

PORTS = (40001, 40002)
ECHO = 40003


def tunnel_rows(profiles):
    by_name = {p["name"]: p for p in profiles}
    return by_name["CI tunnel fixture"]["ssh"]["tunnel_labels"]


class AutostartFixtureProfiles(unittest.TestCase):
    def test_autostart_marks_alpha_only(self):
        profiles = module.fixture_profiles(PORTS, ECHO, populated=True, autostart=True)
        self.assertEqual([p["name"] for p in profiles],
                         ["CI connected fixture", "CI tunnel fixture"])
        self.assertNotIn("ssh", profiles[0])
        rows = tunnel_rows(profiles)
        self.assertEqual([r["label"] for r in rows], list(module.POPULATED_LABELS))
        self.assertIs(rows[0]["auto_start"], True)
        self.assertNotIn("auto_start", rows[1])
        ssh = profiles[1]["ssh"]
        self.assertEqual(ssh["local_forwards"], [r["spec"] for r in rows])
        for port, row in zip(PORTS, rows):
            self.assertEqual(row["spec"], f"127.0.0.1:{port}:127.0.0.1:{ECHO}")
            self.assertTrue(row["spec"].startswith("127.0.0.1:"), "loopback only")

    def test_populated_without_autostart_has_no_flag(self):
        rows = tunnel_rows(module.fixture_profiles(PORTS, ECHO, populated=True, autostart=False))
        self.assertTrue(all("auto_start" not in r for r in rows), rows)

    def test_default_mode_single_profile(self):
        profiles = module.fixture_profiles((), None, populated=False, autostart=False)
        self.assertEqual(len(profiles), 1)
        self.assertNotIn("ssh", profiles[0])

    def test_autostart_requires_populated(self):
        with self.assertRaises(RuntimeError):
            module.fixture_profiles((), None, populated=False, autostart=True)

    def test_unknown_autostart_label_rejected(self):
        with self.assertRaises(RuntimeError):
            module.populated_profile_ssh(PORTS, ECHO, auto_start=("Charlie forward",))

    @unittest.skipIf(os.environ.get("INSPIRUM_GUI_AUTOSTART") == "1", "opt-in env set")
    def test_env_defaults_off(self):
        self.assertFalse(module.AUTOSTART)

    def test_checkbox_label_matches_product(self):
        source = (SOURCE.parents[1] / "src" / "iced_app.rs").read_text(encoding="utf-8")
        self.assertIn(
            f'const TUNNEL_AUTO_START_LABEL: &str = "{module.AUTOSTART_CHECKBOX_LABEL}";', source,
        )


if __name__ == "__main__":
    unittest.main()
