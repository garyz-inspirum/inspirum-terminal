"""Every literal NATIVE_ECHO:<TOKEN> the GUI test waits for must be fixture-logged."""
import ast
from pathlib import Path
import re
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"


def logged_echo_lines() -> tuple:
    # AST read so the check runs without the GUI test's asyncssh/Pillow deps.
    tree = ast.parse(SOURCE.read_text(encoding="utf-8"))
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id == "LOGGED_ECHO_LINES" for t in node.targets
        ):
            return ast.literal_eval(node.value)
    raise AssertionError("LOGGED_ECHO_LINES not found")


class EchoAllowlistTest(unittest.TestCase):
    def test_waited_echo_tokens_are_logged_by_fixture(self):
        text = SOURCE.read_text(encoding="utf-8")
        waited = set(re.findall(r"NATIVE_ECHO:([A-Z][A-Z0-9_]*)", text))
        self.assertIn("AFTER_TUNNEL_TOGGLES", waited)
        missing = sorted(t for t in waited if f"echo:{t}" not in logged_echo_lines())
        self.assertEqual(missing, [], "NATIVE_ECHO tokens awaited but not in LOGGED_ECHO_LINES")

    def test_typed_after_probes_are_logged_by_fixture(self):
        text = SOURCE.read_text(encoding="utf-8")
        typed = set(re.findall(r'"echo:(AFTER_[A-Z0-9_]+)"', text))
        missing = sorted(t for t in typed if f"echo:{t}" not in logged_echo_lines())
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
