"""Fixture profile metadata for INSPIRUM_GUI_AUTOSTART=1.

Alpha opts in to auto-start; Bravo and every default/populated run keep the
product default (field omitted == off). Auto-start mode also appends the
inherited-forward profile (opted-in Charlie whose ssh_config host adds a
LocalForward), which unattended start must skip.
"""
from __future__ import annotations

import asyncio
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
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
        profiles = module.fixture_profiles(PORTS, ECHO, populated=True, autostart=True,
                                           charlie_port=40004)
        self.assertEqual([p["name"] for p in profiles][:2],
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

    @unittest.skipIf(os.environ.get("INSPIRUM_GUI_AUTOSTART") == "1", "INSPIRUM_GUI_AUTOSTART set in env (as in CI)")
    def test_env_defaults_off(self):
        self.assertFalse(module.AUTOSTART)

    def test_checkbox_label_matches_product(self):
        source = (SOURCE.parents[1] / "src" / "iced_app.rs").read_text(encoding="utf-8")
        self.assertIn(
            f'const TUNNEL_AUTO_START_LABEL: &str = "{module.AUTOSTART_CHECKBOX_LABEL}";', source,
        )


# Exact pre-inherited-forward output (captured from the script before the
# third profile existed): non-autostart modes must not change by a byte.
LEGACY_POPULATED_JSON = (
    '[{"name": "CI connected fixture", "host": "native-smoke", "user": "native-smoke", '
    '"port": null, "strict": true}, {"name": "CI tunnel fixture", "host": "native-smoke", '
    '"user": "native-smoke", "port": null, "strict": true, "ssh": {"local_forwards": '
    '["127.0.0.1:40001:127.0.0.1:40003", "127.0.0.1:40002:127.0.0.1:40003"], '
    '"tunnel_labels": [{"kind": "local", "spec": "127.0.0.1:40001:127.0.0.1:40003", '
    '"label": "Alpha forward"}, {"kind": "local", "spec": "127.0.0.1:40002:127.0.0.1:40003", '
    '"label": "Bravo forward"}]}}]'
)
LEGACY_DEFAULT_JSON = (
    '[{"name": "CI connected fixture", "host": "native-smoke", "user": "native-smoke", '
    '"port": null, "strict": true}]'
)
CHARLIE = 40004
INHERITED = 40005


class InheritedForwardFixture(unittest.TestCase):
    def autostart_profiles(self):
        return module.fixture_profiles(PORTS, ECHO, populated=True, autostart=True,
                                       charlie_port=CHARLIE)

    def test_third_profile_only_with_autostart_and_last(self):
        names = [p["name"] for p in self.autostart_profiles()]
        self.assertEqual(names, ["CI connected fixture", "CI tunnel fixture",
                                 module.AUTOSTART_INHERIT_PROFILE])
        for autostart, populated in ((False, True), (False, False)):
            names = [p["name"] for p in module.fixture_profiles(
                PORTS if populated else (), ECHO if populated else None,
                populated=populated, autostart=autostart, charlie_port=CHARLIE)]
            self.assertNotIn(module.AUTOSTART_INHERIT_PROFILE, names)

    def test_third_profile_row_opted_in(self):
        third = self.autostart_profiles()[-1]
        self.assertEqual(third["host"], module.AUTOSTART_INHERIT_HOST)
        self.assertEqual(third["user"], "native-smoke")
        self.assertIs(third["strict"], True)
        spec = f"127.0.0.1:{CHARLIE}:127.0.0.1:{ECHO}"
        self.assertEqual(third["ssh"], {
            "local_forwards": [spec],
            "tunnel_labels": [{"kind": "local", "spec": spec,
                               "label": module.AUTOSTART_INHERIT_LABEL, "auto_start": True}],
        })

    def test_earlier_profiles_unchanged_by_autostart_extra(self):
        profiles = self.autostart_profiles()
        self.assertEqual(profiles[0]["host"], "native-smoke")
        self.assertEqual(profiles[1]["host"], "native-smoke")
        self.assertEqual([r["label"] for r in tunnel_rows(profiles)],
                         list(module.POPULATED_LABELS))

    def test_charlie_label_distinct_from_populated_labels(self):
        label = module.AUTOSTART_INHERIT_LABEL
        for other in module.POPULATED_LABELS:
            self.assertNotEqual(len(label), len(other), "tolerant segment match needs equal length")
            self.assertNotEqual(label.split()[0][:3], other.split()[0][:3])
            self.assertNotIn(other.split()[0], label)

    def test_non_autostart_output_byte_identical(self):
        self.assertEqual(json.dumps(module.fixture_profiles(
            PORTS, ECHO, populated=True, autostart=False)), LEGACY_POPULATED_JSON)
        self.assertEqual(json.dumps(module.fixture_profiles(
            (), None, populated=False, autostart=False)), LEGACY_DEFAULT_JSON)

    def test_autostart_requires_distinct_charlie_port(self):
        with self.assertRaises(RuntimeError):
            module.fixture_profiles(PORTS, ECHO, populated=True, autostart=True)
        with self.assertRaises(RuntimeError):
            module.fixture_profiles(PORTS, ECHO, populated=True, autostart=True,
                                    charlie_port=PORTS[0])

    def test_dropdown_fragment_still_singles_out_tunnel_profile(self):
        # session_dropdown_probe filters the owner combo with "tunnel".
        self.assertNotIn("tunnel", module.AUTOSTART_INHERIT_PROFILE.lower())


class InheritedForwardStanza(unittest.TestCase):
    def written_config(self, tmp: Path) -> Path:
        path = tmp / "ssh_config"
        module.fixture.write_config(
            path, port=2222, identity=tmp / "client", known_hosts=tmp / "known_hosts",
            global_known_hosts=tmp / "global_known_hosts",
        )
        return path

    def test_stanza_copies_native_smoke_and_adds_one_forward(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = self.written_config(Path(tmp))
            original = path.read_text(encoding="utf-8")
            module.append_inherited_forward_host(path, INHERITED, ECHO)
            text = path.read_text(encoding="utf-8")
        self.assertTrue(text.startswith(original), "native-smoke stanza must stay first, unchanged")
        stanzas = {}
        for block in text.split("\nHost "):
            head, *body = block.strip().splitlines()
            stanzas[head.removeprefix("Host ").strip()] = [line.strip() for line in body if line.strip()]
        self.assertEqual(set(stanzas), {"native-smoke", module.AUTOSTART_INHERIT_HOST})
        smoke, inherit = stanzas["native-smoke"], stanzas[module.AUTOSTART_INHERIT_HOST]
        self.assertEqual(inherit[:-1], smoke)
        self.assertEqual(inherit[-1], f"LocalForward 127.0.0.1:{INHERITED} 127.0.0.1:{ECHO}")
        for key in ("HostName 127.0.0.1", "Port 2222", "User native-smoke", "BatchMode yes"):
            self.assertIn(key, inherit)
        self.assertTrue(any(line.startswith("IdentityFile ") for line in inherit))
        self.assertTrue(any(line.startswith("UserKnownHostsFile ") for line in inherit))
        self.assertEqual(sum("Forward" in line for line in smoke), 0)

    def test_fixture_allows_inherited_forward_target(self):
        # On a build WITHOUT the skip the inherited listener must really
        # echo, so the fixture server must allow direct-tcpip to the echo
        # port regardless of which -L opened it.
        server = module.fixture.NativeServer.__new__(module.fixture.NativeServer)
        events = []
        server.events = type("E", (), {"write": lambda _self, line: events.append(line)})()
        server.jump_target_port = None
        server.echo_port = ECHO
        self.assertTrue(server.connection_requested("127.0.0.1", ECHO, "127.0.0.1", INHERITED))
        self.assertEqual(events, ["LOCAL_FORWARD_ALLOW"])

    def test_missing_native_smoke_stanza_rejected(self):
        with self.assertRaises(RuntimeError):
            module.inherited_forward_stanza("Host other\n HostName x\n", INHERITED, ECHO)

    @unittest.skipUnless(shutil.which("ssh"), "OpenSSH client not installed")
    def test_ssh_g_sees_exactly_one_inherited_forward(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = self.written_config(Path(tmp))
            module.append_inherited_forward_host(path, INHERITED, ECHO)
            out = subprocess.run(
                ["ssh", "-G", "-F", str(path), module.AUTOSTART_INHERIT_HOST],
                capture_output=True, text=True, timeout=20, check=True,
            ).stdout.lower().splitlines()
            smoke = subprocess.run(
                ["ssh", "-G", "-F", str(path), "native-smoke"],
                capture_output=True, text=True, timeout=20, check=True,
            ).stdout.lower().splitlines()
        forwards = [line for line in out if line.startswith("localforward ")]
        self.assertEqual(len(forwards), 1, forwards)
        self.assertIn(str(INHERITED), forwards[0])
        self.assertIn("port 2222", out)
        self.assertEqual([line for line in smoke if line.startswith("localforward ")], [])


class ForwardStaysRefused(unittest.TestCase):
    def test_listening_socket_raises_naming_port(self):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            sock.listen(4)
            port = sock.getsockname()[1]
            with self.assertRaises(RuntimeError) as caught:
                asyncio.run(module.forward_stays_refused(port, "CTX opened", seconds=0.5))
        self.assertIn(str(port), str(caught.exception))
        self.assertIn("CTX opened", str(caught.exception))

    def test_closed_port_returns_after_window(self):
        port = module.free_loopback_port()
        start = time.monotonic()
        asyncio.run(module.forward_stays_refused(port, "closed", seconds=0.3))
        self.assertGreaterEqual(time.monotonic() - start, 0.3)

    def test_pair_helper_names_inherited_port(self):
        closed = module.free_loopback_port()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            sock.listen(4)
            port = sock.getsockname()[1]
            with self.assertRaises(RuntimeError) as caught:
                asyncio.run(module.require_inherited_forwards_refused(closed, port, seconds=0.5))
        self.assertIn(f"UNEXPECTED_INHERITED_FORWARD: ssh_config LocalForward {port}",
                      str(caught.exception))


if __name__ == "__main__":
    unittest.main()
