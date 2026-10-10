"""Privacy and percentile regressions for the opt-in Iced trace summarizer."""
import importlib.util
from pathlib import Path
import unittest


SOURCE = Path(__file__).resolve().parents[1] / "analyze-iced-trace.py"
spec = importlib.util.spec_from_file_location("iced_trace_analyzer", SOURCE)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class IcedTraceAnalysisTest(unittest.TestCase):
    def test_only_sanitized_click_timings_are_parsed(self):
        lines = [
            "SSH host user@private.example",
            "iced slow terminal_canvas_draw: 11.2 ms (threshold 1 ms)",
            "iced slow click_to_canvas_draw: 23.4 ms (threshold 1 ms)",
            "iced slow click_to_canvas_draw: 7.0 ms (threshold 1 ms)",
            "iced slow click_to_canvas_draw: sensitive 88 ms (threshold 1 ms)",
        ]
        self.assertEqual(module.click_samples(lines), [23.4, 7.0])

    def test_nearest_rank_and_empty_rejection(self):
        samples = list(range(1, 101))
        self.assertEqual(module.percentile(samples, 50), 50)
        self.assertEqual(module.percentile(samples, 95), 95)
        self.assertEqual(module.percentile([19], 95), 19)
        with self.assertRaises(ValueError):
            module.percentile([], 50)


if __name__ == "__main__":
    unittest.main()
