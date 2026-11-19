"""Synthetic metric and subprocess-contract tests; no game/GPU required."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import textwrap
import unittest
from unittest.mock import patch

from PIL import Image


MODULE_SPEC = importlib.util.spec_from_file_location(
    "capture_runner", Path(__file__).resolve().parents[1] / "capture_runner.py")
runner = importlib.util.module_from_spec(MODULE_SPEC)
MODULE_SPEC.loader.exec_module(runner)


class MetricsTests(unittest.TestCase):
    def test_strict_threshold_channel_max_and_mean(self):
        before = Image.new("RGB", (3, 1), (20, 20, 20))
        after = before.copy()
        after.putpixel((0, 0), (28, 20, 20))  # Exactly threshold: not counted.
        after.putpixel((1, 0), (20, 11, 20))  # Negative difference also counts.
        after.putpixel((2, 0), (20, 20, 255))
        measured, maximum = runner.metrics(before, after, 8)
        self.assertEqual(measured["changed_pixels"], 2)
        self.assertAlmostEqual(measured["changed_percent"], 200 / 3)
        self.assertEqual(measured["max_delta"], 235)
        self.assertEqual(measured["nonzero_pixels"], 3)
        self.assertEqual(measured["mean_abs_delta"], (8 + 9 + 235) / 9)
        self.assertEqual([maximum.getpixel((x, 0)) for x in range(3)], [8, 9, 235])

    def test_zero_and_alpha_ignored(self):
        before = Image.new("RGBA", (2, 2), (50, 60, 70, 0))
        after = Image.new("RGBA", (2, 2), (50, 60, 70, 255))
        measured, _ = runner.metrics(before, after, 0)
        self.assertEqual(measured["max_delta"], 0)
        self.assertEqual(measured["changed_percent"], 0)

    def test_size_mismatch_rejected(self):
        with self.assertRaisesRegex(ValueError, "sizes differ"):
            runner.metrics(Image.new("RGB", (2, 1)), Image.new("RGB", (1, 2)), 8)

    def test_heatmap_scale_clamps(self):
        maximum = Image.new("L", (3, 1))
        maximum.putdata([0, 32, 100])
        heatmap = runner.heatmap(maximum, 4)
        self.assertEqual(heatmap.getpixel((0, 0)), (0, 0, 0))
        self.assertEqual(heatmap.getpixel((1, 0)), (255, 0, 0))
        self.assertEqual(heatmap.getpixel((2, 0)), (255, 255, 0))


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="capture-runner-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.save = self.root / "save.json"
        self.save.write_text('{"test": "synthetic"}')
        self.spec_path = self.root / "scenario.json"
        self.output = self.root / "output"
        self.binary = self.root / "fake-app"
        # Real subprocess: verifies argv/env/isolation and creates a tiny PNG.
        self.binary.write_text(f"#!{sys.executable}\n" + textwrap.dedent('''
            import json, os, pathlib, sys, time
            from PIL import Image
            args = sys.argv[1:]
            output = pathlib.Path(args[args.index('--capture') + 1])
            assert os.environ['THEDAS_HIDDEN_WINDOW'] == '1'
            assert os.environ['THEDAS_NO_AUDIO'] == '1'
            assert os.environ['THEDAS_MODS'] == 'none'
            assert 'THEDAS_UNREQUESTED' not in os.environ
            assert 'WGPU_UNREQUESTED' not in os.environ
            home = pathlib.Path(os.environ['HOME'])
            assert home.is_dir() and not (home / 'touched').exists()
            (home / 'touched').touch()
            if '--save' in args:
                assert pathlib.Path(args[args.index('--save') + 1]).read_text()
            print(json.dumps({'argv': args, 'home': str(home)}), flush=True)
            mode = os.environ.get('FAKE_MODE')
            if mode == 'fail': sys.exit(7)
            if mode == 'timeout': time.sleep(20)
            if mode == 'missing': sys.exit(0)
            size = next(arg.split('=', 1)[1] for arg in args if arg.startswith('--size='))
            size = tuple(map(int, size.split('x')))
            if mode == 'size': size = (1, 1)
            value = int(os.environ.get('FAKE_VALUE', '0'))
            if mode == 'unstable' and '.repeat-' in output.name: value += 1
            Image.new('RGB', size, (value, 0, 0)).save(output)
        '''))
        self.binary.chmod(0o755)
        self.spec = {"save": "save.json", "steps": "key space; run 500",
                     "options": {"size": "4x2"},
                     "cameras": [{"name": "first", "camera": "1,2,3,4,5", "steps": "run 100"},
                                 {"name": "follow", "camera": None}]}

    def invoke(self, *extra):
        self.spec_path.write_text(json.dumps(self.spec))
        args = [str(self.spec_path), "--baseline", str(self.binary), "--candidate", str(self.binary),
                "--install", str(self.root), "--output", str(self.output), *extra]
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return runner.main(args)

    def test_end_to_end_isolation_steps_reports_and_repeats(self):
        with patch.dict(os.environ, {"THEDAS_UNREQUESTED": "ignore", "WGPU_UNREQUESTED": "ignore"}):
            self.assertEqual(self.invoke("--candidate-env", "FAKE_VALUE=9"), 0)
        report = json.loads((self.output / "report.json").read_text())
        self.assertEqual(len(report["frames"]), 2)
        self.assertEqual(report["frames"][0]["changed_percent"], 100)
        self.assertEqual(report["frames"][0]["max_delta"], 9)
        self.assertEqual(report["save_sha256"], runner.sha256(self.save))
        self.assertEqual(report["repeatability"], "passed (exact RGB pixels)")
        self.assertEqual(report["runs"]["baseline"]["sha256"], runner.sha256(self.binary))
        args = report["runs"]["candidate"]["frames"][0]["argv"]
        self.assertEqual(args[args.index("--steps") + 1], "key space; run 500; run 100")
        self.assertIn("--camera=1,2,3,4,5", args)
        follow = report["runs"]["candidate"]["frames"][1]["argv"]
        self.assertFalse(any(arg.startswith("--camera") for arg in follow))
        self.assertNotEqual(args[args.index("--save") + 1], str(self.save))
        self.assertFalse((self.output / "candidate" / "user").exists())
        for frame in report["frames"]:
            self.assertTrue((self.output / frame["heatmap"]).is_file())
            with Image.open(self.output / frame["comparison"]) as panels:
                self.assertEqual(panels.size, (960, 62))
        self.assertIn("100.000000", (self.output / "report.md").read_text())

    def test_identical_frames_report_zero(self):
        self.assertEqual(self.invoke("--repeat", "1", "--fail-above", "0"), 0)
        report = (self.output / "report.md").read_text()
        self.assertIn("exactly zero", report)
        self.assertIn("not checked", report)

    def test_fail_above_strict_boundary(self):
        self.assertEqual(self.invoke("--repeat", "1", "--threshold", "8", "--fail-above", "0",
                                     "--candidate-env", "FAKE_VALUE=8"), 0)
        self.output = self.root / "output2"
        self.assertEqual(self.invoke("--repeat", "1", "--fail-above", "0",
                                     "--candidate-env", "FAKE_VALUE=9"), 1)

    def test_repeat_mismatch_fails_even_below_threshold(self):
        self.assertEqual(self.invoke("--candidate-env", "FAKE_MODE=unstable"), 1)
        report = json.loads((self.output / "report.json").read_text())
        self.assertIn("FAILED", report["repeatability"])
        self.assertEqual(report["runs"]["candidate"]["repeatability"][0]["max_delta"], 1)

    def test_explicit_repeat_tolerance_is_separate_and_recorded(self):
        self.assertEqual(self.invoke("--candidate-env", "FAKE_MODE=unstable",
                                     "--repeat-threshold", "1"), 0)
        report = json.loads((self.output / "report.json").read_text())
        self.assertEqual(report["repeat_threshold"], 1)
        self.assertIn("not exact", report["repeatability"])
        repeat = report["runs"]["candidate"]["repeatability"][0]
        self.assertEqual(repeat["nonzero_pixels"], 8)
        self.assertEqual(repeat["changed_pixels"], 0)
        self.assertTrue((self.output / repeat["comparison"]).is_file())

    def test_capture_errors_preserve_failure_logs(self):
        for mode in ("fail", "missing", "size", "timeout"):
            with self.subTest(mode=mode):
                self.output = self.root / mode
                extra = ["--timeout", "0.2"] if mode == "timeout" else []
                self.assertEqual(self.invoke("--env", f"FAKE_MODE={mode}", *extra), 2)
                self.assertTrue((self.output / "failure.json").is_file())
                self.assertTrue((self.output / "baseline" / "first.log").is_file())
                self.assertFalse((self.output / "report.md").exists())

    def test_existing_output_never_overwritten(self):
        self.output.mkdir()
        marker = self.output / "marker"
        marker.write_text("keep")
        self.assertEqual(self.invoke(), 2)
        self.assertEqual(marker.read_text(), "keep")
        self.assertFalse((self.output / "failure.json").exists())

    def test_bad_specs_rejected_before_launch(self):
        cases = [
            {"cameras": []}, {"cameras": [{"name": "../escape"}]},
            {"cameras": [{"name": "duplicate"}, {"name": "duplicate"}]},
            {"cameras": [{"name": "case"}, {"name": "CASE"}]},
            {"cameras": [{"name": "nan", "camera": "1,2,3,4,nan"}]},
            {"cameras": [{"name": "four", "camera": "1,2,3,4"}]},
            {"options": {"capture": "wrong.png"}},
            {"env": {"THEDAS_HIDDEN_WINDOW": "0"}},
            {"candidate": {"env": {"HOME": "/tmp"}}},
            {"candidate": {"env": []}}, {"candidate": {"options": []}},
            {"baseline": {"options": {"size": "5x2"}}},
            {"unknown": 1}, {"area": "hrt201d"},
        ]
        initial = self.spec.copy()
        for replacement in cases:
            with self.subTest(replacement=replacement):
                self.spec = initial | replacement
                self.assertEqual(self.invoke(), 2)
                self.assertFalse(self.output.exists())

    def test_explicit_movies_and_replay_steps_cannot_mix(self):
        original = self.spec
        movies = {"ui": "hud"}
        self.spec = original | {"options": original["options"] | movies}
        self.assertEqual(self.invoke(), 2)
        self.assertFalse(self.output.exists())

        # A per-camera replay and a per-side movie override still conflict.
        self.spec = original | {"steps": "", "candidate": {"options": movies}}
        self.assertEqual(self.invoke(), 2)
        self.assertFalse(self.output.exists())

        # A static movie capture remains valid, including one over a saved scene.
        self.spec = self.spec | {"cameras": [{"name": "movie"}]}
        self.assertEqual(self.invoke("--repeat", "1"), 0)

    def test_invalid_cli_controls(self):
        for extra in (("--threshold", "-1"), ("--amplify", "nan"), ("--repeat", "0"),
                      ("--repeat-threshold", "256"),
                      ("--env", "NOT_AN_ASSIGNMENT"), ("--fail-above", "101"),
                      ("--candidate-label", "bad|markdown")):
            with self.subTest(extra=extra):
                self.assertEqual(self.invoke(*extra), 2)
                self.assertFalse(self.output.exists())

    def test_viewer_and_save_override(self):
        self.spec = {"area": "hrt201d", "options": {"size": "4x2"},
                     "cameras": [{"name": "hall"}]}
        self.assertEqual(self.invoke("--repeat", "1"), 0)
        report = json.loads((self.output / "report.json").read_text())
        self.assertIsNone(report["save_sha256"])
        self.output = self.root / "override"
        self.assertEqual(self.invoke("--repeat", "1", "--save", str(self.save)), 0)
        report = json.loads((self.output / "report.json").read_text())
        self.assertEqual(report["save_source"], str(self.save.resolve()))

    def test_cli_overrides_json(self):
        self.spec["env"] = {"FAKE_VALUE": "1"}
        self.spec["candidate"] = {"env": {"FAKE_VALUE": "2"}}
        self.assertEqual(self.invoke("--repeat", "1", "--env", "FAKE_VALUE=3",
                                     "--candidate-env", "FAKE_VALUE=4",
                                     "--candidate-renderer", "deferred"), 0)
        report = json.loads((self.output / "report.json").read_text())
        self.assertEqual(report["runs"]["baseline"]["env"]["FAKE_VALUE"], "3")
        self.assertEqual(report["runs"]["candidate"]["env"]["FAKE_VALUE"], "4")
        self.assertEqual(report["runs"]["candidate"]["options"]["renderer"], "deferred")


if __name__ == "__main__":
    unittest.main()
