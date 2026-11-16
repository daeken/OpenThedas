#!/usr/bin/env python3
"""Replay fixed captures against another thedas-app binary. See capture-runner/README.md."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFont, ImageOps, __version__ as pillow_version


# No arbitrary argv: capture/control, file writes, audio and journal flags must not
# override the runner. Add rendering-only switches here as the engine grows them.
OPTIONS = {
    "size": "1280x720", "time": 3, "renderer": "faithful", "detail": "medium",
    "shaders": "original", "fov": 60, "antialiasing": 1,
    "frame-effects": "on", "ui-frames": 30,
}
EXTRA_OPTIONS = {"lighting", "ui", "vfx"}
FORCED_ENV = {"THEDAS_HIDDEN_WINDOW": "1", "THEDAS_NO_AUDIO": "1"}
DEFAULT_ENV = {"THEDAS_MODS": "none"}
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]{0,79}\Z")


def sha256(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def fields(value, allowed, context):
    if not isinstance(value, dict):
        raise ValueError(f"{context} must be an object")
    unknown = value.keys() - allowed
    if unknown:
        raise ValueError(f"unknown {context} fields: {', '.join(sorted(unknown))}")


def options(value):
    fields(value, OPTIONS.keys() | EXTRA_OPTIONS, "options")
    for key, item in value.items():
        if isinstance(item, bool) or not isinstance(item, (str, int, float)):
            raise ValueError(f"option {key} must be a string or number")
        if isinstance(item, float) and not math.isfinite(item):
            raise ValueError(f"option {key} must be finite")
    merged = OPTIONS | value
    if not re.fullmatch(r"[1-9][0-9]*x[1-9][0-9]*", str(merged["size"])):
        raise ValueError("size must be WIDTHxHEIGHT, both positive")
    if merged["renderer"] not in ("faithful", "deferred"):
        raise ValueError("renderer must be faithful or deferred")
    return merged


def environment(value):
    fields(value, value.keys() if isinstance(value, dict) else set(), "env")
    for key, item in value.items():
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key) or not isinstance(item, str):
            raise ValueError("env must map environment variable names to strings")
        if key in ("HOME", "USERPROFILE", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "THEDAS_USER_DIR"):
            raise ValueError(f"{key} is isolated by the runner")
        if key in FORCED_ENV and item != FORCED_ENV[key]:
            raise ValueError(f"{key} is forced to {FORCED_ENV[key]}")
        if "\0" in item:
            raise ValueError(f"env {key} contains a NUL")
    return value


def env_args(values):
    result = {}
    for item in values:
        key, sep, value = item.partition("=")
        if not sep:
            raise ValueError(f"expected NAME=VALUE, got {item!r}")
        result[key] = value
    return environment(result)


def load_spec(path, save_override=None):
    spec = json.loads(path.read_text())
    fields(spec, {"save", "area", "module", "steps", "options", "env", "cameras",
                  "baseline", "candidate"}, "spec")
    if save_override:
        spec["save"] = str(save_override.resolve())
        spec.pop("area", None)
    if ("save" in spec) == ("area" in spec):
        raise ValueError("spec needs exactly one of save or area")
    if "save" in spec:
        if not isinstance(spec["save"], str):
            raise ValueError("save must be a path string")
        save = (path.parent / spec["save"]).resolve()
        if not save.is_file():
            raise ValueError(f"save does not exist: {save}")
        spec["save"] = str(save)
    elif not isinstance(spec["area"], str) or not spec["area"]:
        raise ValueError("area must be a nonempty resource name")
    for key in ("module", "steps"):
        if key in spec and not isinstance(spec[key], str):
            raise ValueError(f"{key} must be a string")
    if "area" in spec and spec.get("steps", "").strip():
        raise ValueError("steps need a save (the viewer does not play them)")
    spec["options"] = options(spec.get("options", {}))
    spec["env"] = environment(spec.get("env", {}))
    for side in ("baseline", "candidate"):
        run = spec.setdefault(side, {})
        fields(run, {"label", "options", "env"}, side)
        if "label" in run and (not isinstance(run["label"], str) or
                               not re.fullmatch(r"[\w .:/@+-]{1,100}", run["label"])):
            raise ValueError(f"{side} label must be 1-100 plain characters")
        options(run.get("options", {}))
        environment(run.get("env", {}))
        run["options"] = options(spec["options"] | run.get("options", {}))
        run["env"] = environment(spec["env"] | run.get("env", {}))
    cameras = spec.get("cameras")
    if not isinstance(cameras, list) or not cameras:
        raise ValueError("cameras must be a nonempty list")
    names = set()
    for camera in cameras:
        fields(camera, {"name", "camera", "steps"}, "camera")
        name = camera.get("name", "")
        if not isinstance(name, str) or not NAME.fullmatch(name) or name.casefold() in names:
            raise ValueError("camera names must be unique safe filenames (letters/digits/_/-)")
        names.add(name.casefold())
        view = camera.get("camera")
        if view is not None:
            if not isinstance(view, str):
                raise ValueError("camera must be x,y,z,yaw,pitch or null (follow camera)")
            numbers = [float(v) for v in view.split(",")]
            if len(numbers) != 5 or not all(math.isfinite(v) for v in numbers):
                raise ValueError("camera must have five finite numbers: x,y,z,yaw,pitch")
        if not isinstance(camera.get("steps", ""), str):
            raise ValueError("camera steps must be a string")
        if "area" in spec and camera.get("steps", "").strip():
            raise ValueError("camera steps need a save")
    return spec


def metrics(before, after, threshold):
    """8-bit output RGB, not linear light. A changed pixel has max-channel delta > T."""
    if before.size != after.size:
        raise ValueError(f"frame sizes differ: {before.size} vs {after.size}")
    delta = ImageChops.difference(before.convert("RGB"), after.convert("RGB"))
    channels = delta.split()
    maximum = ImageChops.lighter(ImageChops.lighter(channels[0], channels[1]), channels[2])
    histogram = maximum.histogram()
    total = before.width * before.height
    changed = sum(histogram[threshold + 1:])
    mean = sum(i * count for channel in channels
               for i, count in enumerate(channel.histogram())) / (total * 3)
    return {
        "pixels": total, "changed_pixels": changed,
        "changed_percent": 100.0 * changed / total,
        "nonzero_pixels": total - histogram[0],
        "max_delta": maximum.getextrema()[1], "mean_abs_delta": mean,
    }, maximum


def heatmap(maximum, amplify):
    amplified = maximum.point([round(min(255, i * amplify)) for i in range(256)])
    return ImageOps.colorize(amplified, black="black", mid="red", white="yellow", midpoint=128)


def triptych(before, after, delta, labels, caption):
    width, height = before.size
    # Labels/caption live outside the captured pixels. Keep full-resolution panels.
    width = max(width, 320)
    font_size = max(12, min(24, width // 50))
    font = ImageFont.load_default(size=font_size)
    band = font_size + 18
    result = Image.new("RGB", (width * 3, height + band * 2), "#16191e")
    draw = ImageDraw.Draw(result)
    for index, (frame, label) in enumerate(zip((before, after, delta), labels)):
        # Long labels are shortened in the image; reports retain the full label.
        while draw.textlength(label, font=font) > width - 16:
            label = label[:-4] + "..."
        draw.text((width * index + 8, 8), label, fill="white", font=font)
        result.paste(frame.convert("RGB"), (width * index, band))
    draw.text((8, height + band + 8), caption, fill="white", font=font)
    return result


def run_capture(binary, spec, camera, output, run_env, run_options, timeout):
    args = [str(binary), "--install", spec["install"], "--capture", str(output), "--no-audio",
            "--saves", str(Path(run_env["HOME"]) / "saves")]
    if "save" in spec:
        args += ["--save", spec["save"], "--steps", "; ".join(
            step for step in (spec.get("steps", ""), camera.get("steps", "")) if step.strip())]
    else:
        args += ["--area", spec["area"]]
    if spec.get("module"):
        args += ["--module", spec["module"]]
    if camera.get("camera") is not None:
        args.append("--camera=" + camera["camera"])
    args += [f"--{key}={value}" for key, value in sorted(run_options.items())]
    log_path = output.with_suffix(".log")
    with log_path.open("wb") as log:
        try:
            result = subprocess.run(args, env=run_env, cwd=output.parent, stdout=log,
                                    stderr=subprocess.STDOUT, timeout=timeout, check=False)
        except subprocess.TimeoutExpired as error:
            raise ValueError(f"capture timed out after {timeout}s; see {log_path}") from error
    if result.returncode:
        raise ValueError(f"capture exited {result.returncode}; see {log_path}")
    if not output.is_file():
        raise ValueError(f"capture produced no PNG; see {log_path}")
    with Image.open(output) as frame:
        frame.load()
        expected = tuple(int(x) for x in str(run_options["size"]).split("x"))
        if frame.format != "PNG" or frame.size != expected:
            raise ValueError(f"invalid PNG or unexpected size at {output}")
    return args


def capture_side(side, binary, spec, root, repeat, timeout, repeat_threshold, amplify):
    directory = root / side
    directory.mkdir()
    # Recreate the same empty user state for every shot/repeat, and keep the real
    # process HOME untouched. Engine paths are overridden only in the child env.
    state = directory / "user"
    run = spec[side]
    overrides = DEFAULT_ENV | run["env"] | FORCED_ENV
    inherited = {k: v for k, v in os.environ.items()
                 if not k.startswith(("THEDAS_", "WGPU_"))}
    child_env = inherited | overrides | {
        "HOME": str(state), "USERPROFILE": str(state),
        "XDG_DATA_HOME": str(state / "data"), "XDG_CONFIG_HOME": str(state / "config"),
        "THEDAS_USER_DIR": str(state / "game"),
    }
    identity = {"binary": str(binary), "sha256": sha256(binary),
                "label": run.get("label", side), "options": run["options"],
                "env": overrides, "frames": [], "repeatability": []}
    for camera in spec["cameras"]:
        first = directory / (camera["name"] + ".png")
        for attempt in range(repeat):
            if state.exists():
                shutil.rmtree(state)
            state.mkdir()
            output = first if attempt == 0 else directory / f"{camera['name']}.repeat-{attempt}.png"
            print(f"{side}: {camera['name']} ({attempt + 1}/{repeat})", flush=True)
            args = run_capture(binary, spec, camera, output, child_env, run["options"], timeout)
            if attempt == 0:
                identity["frames"].append({"name": camera["name"], "argv": args,
                                           "png_sha256": sha256(output)})
            else:
                with Image.open(first) as a, Image.open(output) as b:
                    measured, maximum = metrics(a, b, repeat_threshold)
                    delta = heatmap(maximum, amplify)
                    delta.save(output.with_suffix(".delta.png"))
                    caption = (f"{measured['changed_percent']:.6f}% pixels changed "
                               f"(max RGB delta > {repeat_threshold}); max delta {measured['max_delta']}/255")
                    triptych(a, b, delta, [f"{side}: first", f"{side}: repeat {attempt}",
                                           f"Delta x{amplify:g} (max RGB)"], caption).save(
                        output.with_suffix(".comparison.png"))
                identity["repeatability"].append({"name": camera["name"],
                    "repeat": attempt, **measured,
                    "comparison": str(output.with_suffix(".comparison.png").relative_to(root))})
    if identity["sha256"] != sha256(binary):
        raise ValueError(f"{side} binary changed during capture")
    shutil.rmtree(state)
    return identity


def compare(spec, root, runs, threshold, amplify):
    directory = root / "delta"
    directory.mkdir()
    frames = []
    for camera in spec["cameras"]:
        name = camera["name"]
        with Image.open(root / "baseline" / f"{name}.png") as before, \
                Image.open(root / "candidate" / f"{name}.png") as after:
            measured, maximum = metrics(before, after, threshold)
            delta = heatmap(maximum, amplify)
            delta.save(directory / f"{name}.png")
            caption = (f"{measured['changed_percent']:.6f}% pixels changed (max RGB delta > {threshold}); "
                       f"max delta {measured['max_delta']}/255; black=0, red={128 / amplify:g}, "
                       f"yellow>={255 / amplify:g}")
            labels = [f"Baseline: {runs['baseline']['label']}",
                      f"Candidate: {runs['candidate']['label']}", f"Delta x{amplify:g} (max RGB)"]
            triptych(before, after, delta, labels, caption).save(directory / f"{name}.comparison.png")
        frames.append({"name": name, **measured,
                       "heatmap": f"delta/{name}.png", "comparison": f"delta/{name}.comparison.png"})
    return frames


def markdown(report):
    lines = ["# Capture comparison", "",
             f"Baseline: {report['runs']['baseline']['label']}  ",
             f"Candidate: {report['runs']['candidate']['label']}", "",
             f"Changed = max absolute 8-bit RGB channel delta **> {report['threshold']}**. "
             f"Heatmap amplification: **x{report['amplify']:g}**, clamped at 255; "
             "black=zero, red=half scale, yellow=full scale. Alpha is ignored.", "",
             "| Frame | Changed pixels | Changed % | Max delta /255 | Mean absolute delta | Comparison |",
             "| --- | ---: | ---: | ---: | ---: | --- |"]
    for frame in report["frames"]:
        lines.append(f"| {frame['name']} | {frame['changed_pixels']}/{frame['pixels']} | "
                     f"{frame['changed_percent']:.6f} | {frame['max_delta']} | "
                     f"{frame['mean_abs_delta']:.6f} | [panels]({frame['comparison']}) |")
    lines += ["", "Repeatability: " + report["repeatability"] + ".", ""]
    if all(frame["max_delta"] == 0 for frame in report["frames"]):
        lines += ["Pixel delta is exactly zero for every frame.", ""]
    lines += ["Binary/save SHA-256 hashes, resolved inputs, commands, environment overrides and "
              "repeat metrics are in [report.json](report.json).", ""]
    return "\n".join(lines)


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("spec", type=Path, help="JSON capture scenario")
    result.add_argument("--baseline", type=Path, required=True, help="baseline thedas-app binary")
    result.add_argument("--candidate", type=Path, required=True, help="candidate thedas-app binary")
    result.add_argument("--install", type=Path, required=True)
    result.add_argument("--save", type=Path, help="override the scenario save path")
    result.add_argument("--output", type=Path, required=True, help="new output directory (never overwritten)")
    result.add_argument("--baseline-label")
    result.add_argument("--candidate-label")
    result.add_argument("--env", action="append", default=[], metavar="NAME=VALUE")
    for side in ("baseline", "candidate"):
        result.add_argument(f"--{side}-env", action="append", default=[], metavar="NAME=VALUE")
        result.add_argument(f"--{side}-renderer", choices=("faithful", "deferred"))
    result.add_argument("--threshold", type=int, default=8, help="changed when max RGB delta > T (0..255)")
    result.add_argument("--amplify", type=float, default=4, help="heatmap max-channel amplification")
    result.add_argument("--repeat", type=int, default=2, help="captures per side/camera, default 2 (exact pixel check)")
    result.add_argument("--repeat-threshold", type=int, default=0,
                        help="allowed repeat max RGB delta, default 0 (exact); reported separately")
    result.add_argument("--timeout", type=float, default=900, help="seconds per engine process")
    result.add_argument("--fail-above", type=float, help="exit 1 if any changed percent exceeds this limit")
    return result


def main(argv=None):
    args = parser().parse_args(argv)
    root = args.output.resolve()
    created = False
    try:
        if not 0 <= args.threshold <= 255 or not 0 <= args.repeat_threshold <= 255:
            raise ValueError("threshold and repeat-threshold must be 0..255")
        if not math.isfinite(args.amplify) or args.amplify <= 0:
            raise ValueError("amplify must be positive and finite")
        if args.repeat < 1 or not math.isfinite(args.timeout) or args.timeout <= 0:
            raise ValueError("repeat and timeout must be positive")
        if args.fail_above is not None and (not math.isfinite(args.fail_above) or not 0 <= args.fail_above <= 100):
            raise ValueError("fail-above must be 0..100")
        spec = load_spec(args.spec.resolve(), args.save)
        install = args.install.resolve()
        if not install.is_dir():
            raise ValueError(f"install directory does not exist: {install}")
        spec["install"] = str(install)
        binaries = {side: getattr(args, side).resolve() for side in ("baseline", "candidate")}
        for side, binary in binaries.items():
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise ValueError(f"{side} binary is not executable: {binary}")
            run = spec[side]
            run["env"] = environment(run["env"] | env_args(args.env) |
                                     env_args(getattr(args, side + "_env")))
            if label := getattr(args, side + "_label"):
                if not re.fullmatch(r"[\w .:/@+-]{1,100}", label):
                    raise ValueError("labels must be 1-100 plain characters")
                run["label"] = label
            if renderer := getattr(args, side + "_renderer"):
                run["options"]["renderer"] = renderer
        if spec["baseline"]["options"]["size"] != spec["candidate"]["options"]["size"]:
            raise ValueError("baseline and candidate sizes must match")
        root.mkdir(parents=True, exist_ok=False)
        created = True
        # Snapshot the input so every process reads identical bytes even if a drive
        # script updates the original save during the run. Never copy it into git.
        save_hash = None
        save_source = spec.get("save")
        if "save" in spec:
            snapshot = root / "input-save.json"
            shutil.copyfile(spec["save"], snapshot)
            save_hash = sha256(snapshot)
            spec["save"] = str(snapshot)
        write_json(root / "resolved-spec.json", spec)
        runs = {}
        for side, binary in binaries.items():
            runs[side] = capture_side(side, binary, spec, root, args.repeat, args.timeout,
                                      args.repeat_threshold, args.amplify)
            write_json(root / f"{side}.json", runs[side])
        frames = compare(spec, root, runs, args.threshold, args.amplify)
        unstable = any(item["changed_pixels"] for run in runs.values() for item in run["repeatability"])
        passed = ("passed (exact RGB pixels)" if args.repeat_threshold == 0 else
                  f"passed (max RGB delta <= {args.repeat_threshold}; not exact)")
        status = "FAILED (repeat pixels differ)" if unstable else (
            passed if args.repeat > 1 else "not checked (--repeat 1)")
        report = {"schema_version": 1, "spec": spec, "save_source": save_source,
                  "save_sha256": save_hash, "runs": runs,
                  "tool": {"sha256": sha256(__file__), "python": sys.version,
                           "pillow": pillow_version, "platform": sys.platform},
                  "threshold": args.threshold, "amplify": args.amplify, "frames": frames,
                  "repeatability": status, "repeat_threshold": args.repeat_threshold}
        write_json(root / "report.json", report)
        (root / "report.md").write_text(markdown(report))
        print(markdown(report))
        exceeded = args.fail_above is not None and any(
            frame["changed_percent"] > args.fail_above for frame in frames)
        return 1 if unstable or exceeded else 0
    except (ValueError, OSError) as error:
        print(f"capture-runner: {error}", file=sys.stderr)
        if created:
            write_json(root / "failure.json", {"error": str(error)})
        return 2


if __name__ == "__main__":
    sys.exit(main())
