"""ctrl_f_probe helpers: alt-screen byte report, query plan, counter oracle, wiring."""
import ast
from pathlib import Path
import re
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
SRC = SOURCE.read_text(encoding="utf-8")

# Load only the pure helpers (and the constants they use) without importing
# the script, so these tests run without asyncssh/Pillow.
_WANTED = {
    "ALT_BYTE_PREFIX", "_ALT_BYTE_SKIP", "alt_screen_control_tokens", "alt_byte_count",
    "CTRL_F_FIRST_QUERY", "CTRL_F_NOHIT_SUFFIX", "CTRL_F_REPLACEMENT",
    "ctrl_f_failure_queries", "ctrl_f_plan_problems", "ctrl_f_counter_problem",
    "LOGGED_ECHO_LINES",
}
_tree = ast.parse(SRC)
_nodes = []
for _node in _tree.body:
    if isinstance(_node, ast.FunctionDef) and _node.name in _WANTED:
        _nodes.append(_node)
    elif isinstance(_node, ast.Assign) and any(
        isinstance(t, ast.Name) and t.id in _WANTED for t in _node.targets
    ):
        _nodes.append(_node)
NS: dict = {}
exec(compile(ast.Module(body=_nodes, type_ignores=[]), str(SOURCE), "exec"), NS)


def function_source(name: str) -> str:
    nodes = [n for n in ast.walk(_tree)
             if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)) and n.name == name]
    assert len(nodes) == 1, f"{name}: {len(nodes)} definitions"
    return ast.get_source_segment(SRC, nodes[0])


def fixture_lines() -> list[str]:
    """Every line the fixture shell prints to a pane."""
    lines = ["NATIVE_SMOKE_READY", "NATIVE_FULLSCREEN_ACTIVE", "NATIVE_REMOTE_CURSOR_LEFT"]
    lines += [f"NATIVE_ECHO:{line[5:]}" for line in NS["LOGGED_ECHO_LINES"]]
    return lines


class AltByteTest(unittest.TestCase):
    def test_ctrl_f_is_reported_and_removed(self):
        tokens, kept = NS["alt_screen_control_tokens"]("\x06")
        self.assertEqual(tokens, ["ALT_BYTE:06"])
        self.assertEqual(kept, "")

    def test_each_control_byte_counts_in_order(self):
        tokens, kept = NS["alt_screen_control_tokens"]("a\x06\x06b\x03")
        self.assertEqual(tokens, ["ALT_BYTE:06", "ALT_BYTE:06", "ALT_BYTE:03"])
        self.assertEqual(kept, "ab")

    def test_line_ends_and_escape_sequences_untouched(self):
        # screen-off<CR> and cursor-left must keep their existing handling.
        chunk = "screen-off\r\n\x1b[D\x1bOD"
        self.assertEqual(NS["alt_screen_control_tokens"](chunk), ([], chunk))
        tokens, kept = NS["alt_screen_control_tokens"]("\x06screen-off\r")
        self.assertEqual((tokens, kept), (["ALT_BYTE:06"], "screen-off\r"))

    def test_printable_text_is_never_logged(self):
        tokens, _ = NS["alt_screen_control_tokens"]("secret f F")
        self.assertEqual(tokens, [])

    def test_count_matches_whole_lines_only(self):
        log = "ALT_BYTE:06\nX ALT_BYTE:06\nALT_BYTE:066\nALT_BYTE:03\nALT_BYTE:06\n"
        self.assertEqual(NS["alt_byte_count"](log, 0x06), 2)
        self.assertEqual(NS["alt_byte_count"](log, 0x03), 1)
        self.assertEqual(NS["alt_byte_count"]("", 0x06), 0)


class QueryPlanTest(unittest.TestCase):
    def test_plan_discriminates_against_fixture_lines(self):
        self.assertEqual(NS["ctrl_f_plan_problems"](fixture_lines()), [])

    def test_replacement_has_no_f(self):
        self.assertNotIn("f", NS["CTRL_F_REPLACEMENT"].lower())

    def test_weak_plans_are_rejected(self):
        lines = fixture_lines()
        plan = NS["ctrl_f_plan_problems"]
        self.assertTrue(plan(lines, replacement="FULLSCREEN"))  # contains f
        self.assertTrue(plan(lines, suffix=""))  # intermediate 0/0 never happens
        self.assertTrue(plan(lines, replacement="ABSENTQQ"))  # no match at all
        self.assertTrue(plan(lines + ["NATIVEZZQSMOKE"]))  # append would match
        self.assertTrue(plan(lines + ["xfsmoke"]))  # typed f would match (case-fold)

    def test_failure_queries_cover_append_and_f(self):
        got = NS["ctrl_f_failure_queries"]("A", "B", "C")
        for query in ("ABC", "fC", "ABfC"):
            self.assertIn(query, got)


class CounterOracleTest(unittest.TestCase):
    def test_hits(self):
        check = NS["ctrl_f_counter_problem"]
        self.assertIsNone(check((1, 3, "1/3"), want_hits=True))
        self.assertIsNone(check((3, 3, "3/3"), want_hits=True))
        self.assertIsNotNone(check((0, 0, "0/0"), want_hits=True))
        self.assertIsNotNone(check((0, 2, "0/2"), want_hits=True))
        self.assertIsNotNone(check((4, 3, "4/3"), want_hits=True))
        self.assertIsNotNone(check(None, want_hits=True))

    def test_no_hits(self):
        check = NS["ctrl_f_counter_problem"]
        self.assertIsNone(check((0, 0, "0/0"), want_hits=False))
        self.assertIsNotNone(check((1, 2, "1/2"), want_hits=False))
        self.assertIsNotNone(check((0, 1, "0/1"), want_hits=False))
        self.assertIsNotNone(check(None, want_hits=False))


class WiringTest(unittest.TestCase):
    def test_sentinel_is_allowlisted(self):
        self.assertIn("echo:AFTER_CTRL_F", NS["LOGGED_ECHO_LINES"])

    def test_main_probe_presses_plain_ctrl_f_and_arms_before(self):
        body = function_source("ctrl_f_probe")
        self.assertNotIn("ctrl+shift+f", body)
        armed = body.index('probe_state["phase"] = "FIND"')
        first_key = body.index('"ctrl+f"')
        self.assertLess(armed, first_key)
        self.assertEqual(body.count('"ctrl+f"'), 2)
        self.assertIn("connected-find-ctrl-f.png", body)
        self.assertIn("wait_find_strip_closed", body)
        self.assertIn("NATIVE_ECHO:AFTER_CTRL_F", body)
        # The sentinel is typed after the phase is disarmed.
        self.assertLess(body.index('probe_state["phase"] = None'),
                        body.index('"echo:AFTER_CTRL_F"'))

    def test_alt_probe_requires_one_0x06_and_no_strip(self):
        body = function_source("ctrl_f_altscreen_probe")
        self.assertIn('"ctrl+f"', body)
        self.assertNotIn("ctrl+shift+f", body)
        self.assertIn("after - before != 1", body)
        self.assertIn("find_strip_residue", body)
        self.assertIn("connected-find-ctrl-f-altscreen.png", body)

    def test_probes_are_called_from_main_in_order(self):
        main = function_source("main")
        p3 = main.index('await find_echo("FIND_P3_AFTER")')
        call = main.index("await ctrl_f_probe(")
        screen_on = main.index('"screen-on"')
        alt = main.index("await ctrl_f_altscreen_probe(")
        screen_off = main.index('"screen-off"')
        self.assertLess(p3, call)
        self.assertLess(screen_on, alt)
        self.assertLess(alt, screen_off)

    def test_fixture_reports_alt_bytes_only_inside_alt_screen(self):
        body = function_source("monitored_shell")
        self.assertIn("alt_screen = True", body)
        self.assertIn("alt_screen = False", body)
        self.assertTrue(re.search(r"if alt_screen:\s+alt_tokens, chunk = alt_screen_control_tokens", body))
        self.assertLess(body.index("alt_screen_control_tokens"), body.index("buffer += chunk"))


if __name__ == "__main__":
    unittest.main()
