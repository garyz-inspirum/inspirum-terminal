"""Saved SSH-config native GUI gate wiring and persistent-state oracles."""
import ast
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_saved", SOURCE)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SavedSshConfigGui(unittest.TestCase):
    def test_startup_sidecar_path_and_saved_value(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            profiles = Path(tmp) / "profiles.json"
            sidecar = profiles.with_name("profiles.json.startup.json")
            self.assertEqual(module.saved_ssh_config_path(profiles), sidecar)
            module.require_saved_ssh_config(profiles, None)
            with self.assertRaisesRegex(RuntimeError, "missing"):
                module.require_saved_ssh_config(profiles, "/fixture/ssh_config")
            sidecar.write_text(json.dumps({"behavior": "none", "ssh_config": "/fixture/other"}))
            with self.assertRaisesRegex(RuntimeError, "expected"):
                module.require_saved_ssh_config(profiles, "/fixture/ssh_config")
            sidecar.write_text(json.dumps({"behavior": "none", "ssh_config": "/fixture/ssh_config"}))
            module.require_saved_ssh_config(profiles, "/fixture/ssh_config")
            # An unrelated legacy state file must never stand in for the sidecar.
            (Path(tmp) / "ui-state.json").write_text(json.dumps({"ssh_config_path": "/fixture/other"}))
            module.require_saved_ssh_config(profiles, "/fixture/ssh_config")
            sidecar.write_text(json.dumps({"behavior": "none"}))
            module.require_saved_ssh_config(profiles, None)
            with self.assertRaisesRegex(RuntimeError, "expected"):
                module.require_saved_ssh_config(profiles, "/fixture/ssh_config")
            sidecar.write_text(json.dumps({"behavior": "none", "ssh_config": None}))
            module.require_saved_ssh_config(profiles, None)
            sidecar.write_text(json.dumps({"behavior": "none", "ssh_config": "/fixture/other"}))
            with self.assertRaisesRegex(RuntimeError, "expected"):
                module.require_saved_ssh_config(profiles, None)

    def test_saved_mode_is_mandatory_in_existing_connected_job(self):
        workflow = (SOURCE.parents[1] / ".github/workflows/iced-design.yml").read_text()
        step = workflow.split("- name: Connected production SSH GUI acceptance (Linux)", 1)[1].split("- name: Prepare preview artifact", 1)[0]
        self.assertIn("INSPIRUM_GUI_SAVED_SSH_CONFIG=1", step)
        self.assertIn("INSPIRUM_GUI_AUTOSTART=1", step)
        self.assertEqual(step.count("scripts/test-iced-connected-gui.py"), 1)

    def test_saved_mode_uses_actual_ui_then_relaunches_without_cli_config(self):
        tree = ast.parse(SOURCE.read_text())
        funcs = {node.name: node for node in tree.body if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))}
        self.assertIn("saved_ssh_config_probe", funcs)
        body = ast.get_source_segment(SOURCE.read_text(), funcs["saved_ssh_config_probe"])
        assert body is not None
        for token in ("Set SSH config for auto-start…", "Absolute SSH config file path", "Profiles", "Save", "Clear", "Cancel", "forward_echo", "require_inherited_forwards_refused", "request_window_close"):
            self.assertIn(token, body)
        self.assertNotIn("SSH config file for tunnel auto-start", body)
        self.assertIn("--ssh-config", body)
        self.assertIn("cwd=local_root", body)
        self.assertNotIn("primary_fill_run", body)  # product's Save is quiet, not primary
        main = SOURCE.read_text().split("async def main()", 1)[1]
        self.assertIn("saved_ssh_config_probe(", main)
        self.assertIn("profiles.write_text(json.dumps(fixture_profiles(", main)
        self.assertIn("app = None", main.split("await saved_ssh_config_probe(", 1)[0][-900:])
