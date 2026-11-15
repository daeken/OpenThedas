"""Compare shadow_walk sequences (requires numpy and Pillow).

Usage: python shadow_walk_metrics.py BEFORE AFTER [BEFORE_CONTROL AFTER_CONTROL]
Controls use --unshadowed; all sequences must use identical camera paths/settings.
"""
import csv
import json
from pathlib import Path
import sys

import numpy as np
from PIL import Image


def summary(values):
    return {
        "mean": float(np.mean(values)),
        "p95": float(np.percentile(values, 95)),
        "max": float(np.max(values)),
    }


def matrices(folder):
    rows = list(csv.DictReader((folder / "shadow-data.csv").open()))
    data = np.zeros((max(int(r["frame"]) for r in rows) + 1, 2, 3, 4))
    for r in rows:
        data[int(r["frame"]), int(r["partition"]), int(r["row"])] = [
            float(r[k]) for k in ("x", "y", "z", "w")
        ]
    return data, int(rows[0]["size"])


def rgb(path):
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.float32)


def measure(folder, control):
    data, size = matrices(folder)
    camera = next(csv.DictReader((folder / "cameras.csv").open()))
    origin = np.array([float(camera[k]) for k in ("x", "y", "z")])
    # Fixed world anchors, not moving camera coordinates: measure the map sliding
    # across static geometry separately from ordinary image motion.
    anchors = np.array([
        [*(origin + [x, y, z]), 1.0]
        for x in (-5, 0, 5) for y in (-5, 0, 5) for z in (-2, 0, 2)
    ])
    texels = np.einsum("fpij,aj->fpai", data[:, :, :2], anchors) * size
    movement = np.linalg.norm(np.diff(texels, axis=0), axis=-1)
    result = {
        "frames": len(data),
        "shadow_grid_motion_texels": summary(movement),
        "stationary_partition_frames_percent_threshold_0.001_texel": float(
            np.mean(np.max(movement, axis=-1) < 0.001) * 100
        ),
    }
    frames = sorted(folder.glob("[0-9][0-9][0-9][0-9].png"))
    previous = None
    delta_mean, delta_changed = [], []
    for frame in frames:
        image = rgb(frame)
        if control:
            # Signed difference removes the same frame's ordinary unshadowed
            # lighting/geometry; the runtime shadow contribution remains. Deferred's
            # control also suppresses its point-shadow casters.
            image = rgb(control / frame.name) - image
        if previous is not None:
            delta = np.abs(image - previous)
            delta_mean.append(np.mean(delta))
            delta_changed.append(np.mean(np.max(delta, axis=-1) > 8) * 100)
        previous = image
    result["shadow_residual_delta_rgb_0_255" if control else "frame_delta_rgb_0_255"] = summary(delta_mean)
    result["changed_pixels_percent_threshold_8"] = summary(delta_changed)
    return result


before, after = map(Path, sys.argv[1:3])
controls = list(map(Path, sys.argv[3:5])) if len(sys.argv) == 5 else [None, None]
assert len(controls) == 2
camera_path = (before / "cameras.csv").read_text()
for folder in [after, *controls]:
    if folder:
        assert (folder / "cameras.csv").read_text() == camera_path, "camera paths differ"
print(json.dumps({"before": measure(before, controls[0]), "after": measure(after, controls[1])}, indent=2))
