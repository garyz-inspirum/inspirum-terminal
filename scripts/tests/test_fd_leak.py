"""Descriptor and temporary-file regression checks for connected GUI OCR."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SOURCE = Path(__file__).resolve().parents[1] / "test-iced-connected-gui.py"
spec = importlib.util.spec_from_file_location("iced_connected_fd", SOURCE)
assert spec is not None and spec.loader is not None
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


@unittest.skipUnless(Path("/proc/self/fd").is_dir(), "requires /proc/self/fd")
class TemporaryPngDescriptors(unittest.TestCase):
    def test_temp_png_repeatedly_closes_descriptor(self):
        before = len(os.listdir("/proc/self/fd"))
        for _ in range(50):
            path = module._temp_png()
            try:
                self.assertTrue(path.exists())
            finally:
                path.unlink(missing_ok=True)
        self.assertEqual(len(os.listdir("/proc/self/fd")), before)

    def test_region_ocr_repeatedly_closes_descriptor_and_removes_crop(self):
        with tempfile.TemporaryDirectory() as directory:
            frame = Path(directory) / "frame.png"
            module._write_png(frame, 2, 2, bytes((255, 255, 255)) * 4)
            crops = []

            def ocr(_tesseract, path, _psm):
                crops.append(path)
                self.assertTrue(path.exists())
                return ""

            before = len(os.listdir("/proc/self/fd"))
            with mock.patch.object(module, "_ocr_tsv", side_effect=ocr):
                for _ in range(50):
                    self.assertEqual(module.region_upscaled_rows(frame, "unused", (0, 0, 2, 2)), [])
            self.assertEqual(len(os.listdir("/proc/self/fd")), before)
            self.assertEqual(len(crops), 50)
            self.assertTrue(all(not path.exists() for path in crops))


if __name__ == "__main__":
    unittest.main()
