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
    def test_ui_state_path_must_be_exactly_saved(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as tmp:
            path = Path(tmp) / "ui-state.json"
            with self.assertRaises(RuntimeError):
                module.require_saved_ssh_config(path, "/fixture/ssh_config")
            path.write_text(json.dumps({"ssh_config_path": "/fixture/other"}))
            with self.assertRaises(RuntimeError):
                module.require_saved_ssh_config(path, "/fixture/ssh_config")
            path.write_text(json.dumps({"ssh_config_path": "/fixture/ssh_config"}))
            module.require_saved_ssh_config(path, "/fixture/ssh_config")
            path.write_text("{}")
            module.require_saved_ssh_config(path, None)

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
        for token in ("SSH config file for tunnel auto-start", "Profiles", "Save", "Clear", "Cancel", "forward_echo", "require_inherited_forwards_refused", "request_window_close"):
            self.assertIn(token, body)
        self.assertIn("--ssh-config", body)  # explicit negative argv assertion
        main = SOURCE.read_text().split("async def main()", 1)[1]
        self.assertIn("saved_ssh_config_probe(", main)
        self.assertIn("profiles.write_text(json.dumps(fixture_profiles(", main)
