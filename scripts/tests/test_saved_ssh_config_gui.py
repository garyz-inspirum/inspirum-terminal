"""Saved SSH-config native GUI gate wiring and persistent-state oracles."""
import ast
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock

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
        for token in ("Profiles", "Save", "Clear", "Cancel", "forward_echo", "require_inherited_forwards_refused", "request_window_close", "open_saved_ssh_config_editor("):
            self.assertIn(token, body)
        for token in ("SSH config file for tunnel auto-start", "Saved file:", "Set SSH config for auto-start...", "Absolute SSH config file path"):
            self.assertIn(token, SOURCE.read_text())
        self.assertIn("--ssh-config", body)
        self.assertIn("cwd=local_root", body)
        self.assertNotIn("primary_fill_run", body)  # product's Save is quiet, not primary
        main = SOURCE.read_text().split("async def main()", 1)[1]
        self.assertIn("saved_ssh_config_probe(", main)
        self.assertIn("profiles.write_text(json.dumps(fixture_profiles(", main)
        self.assertIn("app = None", main.split("await saved_ssh_config_probe(", 1)[0][-900:])


class SavedGuiSection(unittest.IsolatedAsyncioTestCase):
    async def test_product_labels_locate_editor_with_bounded_ocr(self):
        # Product frame: the heading and saved row are separate; the old
        # "Saved SSH config:" label does not exist. Draft stays below them.
        labels = {
            "SSH config file for tunnel auto-start": (213, 582, 504, 596),
            "Saved file:": (213, 631, 290, 644),
            "Absolute SSH config file path": (225, 692, 435, 703),
            "Cancel": (355, 726, 405, 738),
        }
        seen = []

        async def locate(_window, _path, label, *, env, region, timeout=22):
            seen.append((label, region, timeout))
            if label not in labels:
                raise AssertionError(f"unexpected OCR lookup: {label}")
            box = labels[label]
            self.assertLessEqual(region[0], box[0])
            self.assertLessEqual(region[1], box[1])
            self.assertGreaterEqual(region[2], box[2])
            self.assertGreaterEqual(region[3], box[3])
            return box

        async def click(_window, _path, label, _env, *, region, timeout):
            self.assertEqual(label, "Set SSH config for auto-start...")
            self.assertLessEqual(region[1], 653)
            self.assertGreaterEqual(region[3], 676)
            self.assertLessEqual(timeout, 20)

        with mock.patch.object(module, "locate_rendered_label", side_effect=locate), \
             mock.patch.object(module, "click_label", side_effect=click):
            actions, field = await module.open_saved_ssh_config_editor(
                "window", Path("/frames"), "save", Path("/fixture/ssh_config"),
                {}, (200, 34, 1080, 778),
            )
        self.assertEqual(field, labels["Absolute SSH config file path"])
        self.assertTrue(actions[1] > field[3])
        self.assertEqual([label for label, _, _ in seen], [
            "SSH config file for tunnel auto-start", "Saved file:",
            "Absolute SSH config file path", "Cancel",
        ])
        self.assertTrue(all(timeout <= 20 for _, _, timeout in seen))
        self.assertLessEqual(seen[0][2], 5)
        self.assertLessEqual(seen[1][2], 5)


class SavedGuiFirstFrame(unittest.IsolatedAsyncioTestCase):
    async def test_locator_retries_initial_empty_frame_then_locates_real_frame(self):
        with tempfile.TemporaryDirectory() as tmp:
            frame = Path(tmp) / "saved-editor-ready.png"
            attempts = []

            def capture(window, path, env):
                attempts.append(path)
                if len(attempts) == 1:
                    path.write_bytes(b"empty")
                    raise RuntimeError(f"empty GUI screenshot: {path.name}")
                path.write_bytes(b"rendered frame" * 100)

            def locate(path, label, *, region):
                self.assertEqual(path.read_bytes(), b"rendered frame" * 100)
                self.assertEqual(label, "Tunnels")
                return (1, 2, 3, 4)

            with mock.patch.object(module, "screenshot", side_effect=capture), \
                 mock.patch.object(module, "locate_visible_label", side_effect=locate), \
                 mock.patch.object(module.asyncio, "sleep", new_callable=mock.AsyncMock):
                bounds = await module.locate_rendered_label(
                    "window", frame, "Tunnels", env={}, timeout=20,
                )
            self.assertEqual(bounds, (1, 2, 3, 4))
            self.assertEqual(attempts, [frame, frame])

    async def test_locator_does_not_retry_imagemagick_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            frame = Path(tmp) / "saved-editor-ready.png"
            with mock.patch.object(module, "screenshot", side_effect=RuntimeError("ImageMagick screenshot failed: bad binary")) as capture:
                with self.assertRaisesRegex(RuntimeError, "ImageMagick screenshot failed"):
                    await module.locate_rendered_label("window", frame, "Tunnels", env={}, timeout=20)
            capture.assert_called_once()

    async def test_locator_stops_on_empty_frame_at_deadline(self):
        with tempfile.TemporaryDirectory() as tmp:
            frame = Path(tmp) / "saved-editor-ready.png"
            with mock.patch.object(module, "screenshot", side_effect=RuntimeError("empty GUI screenshot: saved-editor-ready.png")) as capture:
                with self.assertRaisesRegex(RuntimeError, "empty GUI screenshot"):
                    await module.locate_rendered_label("window", frame, "Tunnels", env={}, timeout=0)
            capture.assert_called_once()


FIXTURES = Path(__file__).resolve().parent / "fixtures"
CI_FRAME = FIXTURES / "ci3-saved-cancel-field.png"
LOCAL_FRAME = FIXTURES / "local3-saved-cancel-field.png"
CI_PATH = "/home/runner/work/_temp/iced-connected-fixture-5bpw841n/ssh_config"
LOCAL_PATH = "/tmp/iced-connected-fixture-m9ldf5zx/ssh_config"
PANEL = (200, 34, 1080, 778)
# Measured on both 1280x800 frames: the Tools -> Profiles text input sits
# directly below the repeated "SSH config file for tunnel auto-start" field
# label; Save/Clear/Cancel are the row under it, Saved file is above.
INPUT = (213, 674, 1056, 701)
SAVED_ROW_BOTTOM = 645
BUTTONS_TOP = 706
BUTTONS_BOTTOM = 729
NOTE_TOP = 736
# OCR boxes locate_visible_label returned on the real frames (tesseract 4.1.1).
OCR = {
    CI_FRAME: {"heading": (218, 586, 496, 592), "saved": (218, 634, 285, 638),
               "label": (218, 658, 496, 665), "cancel": (350, 715, 387, 719)},
    LOCAL_FRAME: {"heading": (218, 585, 498, 592), "saved": (218, 634, 286, 638),
                  "label": (218, 657, 498, 665), "cancel": (350, 714, 387, 719)},
}


def inside(point, box):
    return box[0] <= point[0] <= box[2] and box[1] <= point[1] <= box[3]


class SavedFieldPixelLocator(unittest.TestCase):
    """The saved draft is a random absolute path; never OCR it exactly."""

    def test_fixture_is_the_real_ci_frame(self):
        from PIL import Image
        with Image.open(CI_FRAME) as image:
            self.assertEqual(image.size, (1280, 800))
        self.assertLess(CI_FRAME.stat().st_size, 120_000)

    def test_field_click_lands_inside_input_on_ci_and_local(self):
        for frame in (CI_FRAME, LOCAL_FRAME):
            with self.subTest(frame=frame.name):
                field = module.saved_ssh_config_field(frame, OCR[frame]["label"], PANEL)
                point = module.center(field)
                self.assertTrue(inside(point, INPUT), (field, point))
                # Interior only: inside the border, never the Saved file
                # summary above it nor the Save/Clear/Cancel row below.
                self.assertGreater(field[0], INPUT[0] - 1)
                self.assertLess(field[2], INPUT[2] + 1)
                self.assertGreaterEqual(field[1], INPUT[1])
                self.assertLessEqual(field[3], INPUT[3])
                self.assertGreater(field[1], SAVED_ROW_BOTTOM)
                self.assertLess(field[3], BUTTONS_TOP)
                self.assertGreater(field[2] - field[0], 700)

    def _erased(self, frame, box, tmp):
        from PIL import Image, ImageDraw
        out = Path(tmp) / f"erased-{frame.name}"
        with Image.open(frame) as source:
            image = source.convert("RGB")
        ImageDraw.Draw(image).rectangle(box, fill=image.getpixel((205, 650)))
        image.save(out)
        return out

    def test_refuses_when_input_is_erased(self):
        with tempfile.TemporaryDirectory() as tmp:
            for frame in (CI_FRAME, LOCAL_FRAME):
                with self.subTest(frame=frame.name):
                    erased = self._erased(frame, (INPUT[0] - 1, INPUT[1] - 1, INPUT[2] + 1, INPUT[3] + 1), tmp)
                    with self.assertRaisesRegex(RuntimeError, "SSH config input"):
                        module.saved_ssh_config_field(erased, OCR[frame]["label"], PANEL)

    def test_saved_file_summary_is_never_a_field(self):
        # Anchored on the heading instead of the field label, the only rows
        # below are Source/Saved file text: no input, so refuse.
        for frame in (CI_FRAME, LOCAL_FRAME):
            with self.subTest(frame=frame.name):
                with self.assertRaisesRegex(RuntimeError, "SSH config input"):
                    module.saved_ssh_config_field(frame, OCR[frame]["heading"], PANEL)

    def test_refuses_input_not_aligned_under_label(self):
        # The field label must sit directly above the input's left edge.
        label = OCR[CI_FRAME]["label"]
        shifted = (label[0] + 300, label[1], label[2] + 300, label[3])
        with self.assertRaisesRegex(RuntimeError, "SSH config input"):
            module.saved_ssh_config_field(CI_FRAME, shifted, PANEL)

    @unittest.skipUnless(os.environ.get("INSPIRUM_SLOW_OCR_TESTS") == "1" and shutil.which("tesseract"),
                         "slow real-OCR evidence; set INSPIRUM_SLOW_OCR_TESTS=1")
    def test_exact_random_path_ocr_misses_on_real_ci_frame(self):
        # The pre-fix oracle: exact absolute path in the field region.
        with self.assertRaisesRegex(RuntimeError, "visible label not found"):
            module.locate_visible_label(CI_FRAME, CI_PATH, region=(200, 639, 1080, 772))

    @unittest.skipUnless(os.environ.get("INSPIRUM_SLOW_OCR_TESTS") == "1" and shutil.which("tesseract"),
                         "slow real-OCR evidence; set INSPIRUM_SLOW_OCR_TESTS=1")
    def test_real_ocr_label_band_then_pixel_field(self):
        # Same bands open_saved_ssh_config_editor derives from Saved file.
        for frame in (CI_FRAME, LOCAL_FRAME):
            with self.subTest(frame=frame.name):
                saved = OCR[frame]["saved"]
                section = (PANEL[0], saved[3] + 1, PANEL[2], min(PANEL[3], saved[3] + 134))
                band = (section[0], section[1], section[2], min(section[3], saved[3] + 40))
                label = module.locate_visible_label(frame, module.SAVED_SSH_CONFIG_LABEL, region=band)
                field = module.saved_ssh_config_field(frame, label, section)
                self.assertTrue(inside(module.center(field), INPUT), (label, field))
                actions = (section[0], field[3] + 1, section[2], min(section[3], field[3] + 34))
                for button in ("Cancel", "Clear"):
                    box = module.locate_visible_label(frame, button, region=actions)
                    self.assertGreaterEqual(box[1], BUTTONS_TOP - 2, (button, box))
                    self.assertLessEqual(box[3], BUTTONS_BOTTOM + 2, (button, box))


class SavedGuiSavedFieldSection(unittest.IsolatedAsyncioTestCase):
    async def _open(self, frame, tag, *, drop=()):
        boxes = OCR[frame]
        names = {
            "SSH config file for tunnel auto-start": None,
            "Saved file:": boxes["saved"],
            "Cancel": boxes["cancel"],
        }
        seen = []

        async def locate(_window, path, label, *, env, region, timeout=22):
            seen.append((label, region, Path(path).name))
            if label in drop and not Path(path).name.endswith("-heading.png"):
                raise RuntimeError(f"visible label not found in screenshot: {label}")
            if label == "SSH config file for tunnel auto-start":
                # Heading before Set, field label after; both are bounded.
                box = boxes["heading"] if Path(path).name.endswith("-heading.png") else boxes["label"]
                shutil.copyfile(frame, path)
            elif label in names:
                box = names[label]
            else:
                raise AssertionError(f"unexpected OCR lookup (random path?): {label}")
            for i in (0, 1):
                self.assertLessEqual(region[i], box[i])
                self.assertGreaterEqual(region[i + 2], box[i + 2])
            return box

        async def click(*_args, **_kwargs):
            return None

        with tempfile.TemporaryDirectory() as tmp, \
             mock.patch.object(module, "locate_rendered_label", side_effect=locate), \
             mock.patch.object(module, "click_label", side_effect=click):
            result = await module.open_saved_ssh_config_editor(
                "window", Path(tmp), tag, Path(CI_PATH if frame == CI_FRAME else LOCAL_PATH),
                {}, PANEL,
            )
        return result, seen

    async def test_cancel_and_clear_target_input_without_exact_path_ocr(self):
        for frame in (CI_FRAME, LOCAL_FRAME):
            for tag in ("cancel", "clear"):
                with self.subTest(frame=frame.name, tag=tag):
                    (actions, field), seen = await self._open(frame, tag)
                    self.assertTrue(inside(module.center(field), INPUT))
                    labels = [label for label, _, _ in seen]
                    self.assertNotIn(CI_PATH, labels)
                    self.assertNotIn(LOCAL_PATH, labels)
                    self.assertEqual(labels[-1], "Cancel")
                    # Buttons stay constrained to the row directly below the field.
                    self.assertGreater(actions[1], field[3])
                    self.assertLessEqual(actions[1], INPUT[3] + 6)
                    self.assertGreaterEqual(actions[3], BUTTONS_BOTTOM)
                    self.assertLess(actions[3], NOTE_TOP)  # Saved path applies only ...
                    self.assertTrue(inside(module.center(OCR[frame]["cancel"]), actions))

    async def test_missing_field_label_refuses(self):
        with self.assertRaises(RuntimeError):
            await self._open(CI_FRAME, "cancel", drop=("SSH config file for tunnel auto-start",))
