#!/usr/bin/env python3
"""Regress the live follow-camera frame order against a running --control game.

Start an exploration save with THEDAS_HIDDEN_WINDOW=1 under cargo-slot.sh.
Run through capped.sh with PORT OUT.json, optionally --seconds (default 3).
The hero needs room to walk forward. The trace includes every synchronized frame
and marks which frames render to the surface or a requested offscreen capture.
"""

import argparse
import json
import math
from pathlib import Path
import socket
import time


def request(port, body):
    with socket.create_connection(("127.0.0.1", port), timeout=5) as stream:
        stream.settimeout(120)
        stream.sendall((json.dumps(body) + "\n").encode())
        response = json.loads(stream.makefile("rb").readline())
    if not response.get("ok"):
        raise RuntimeError(response)
    return response


def check(port, output, seconds):
    request(port, {"cmd": "motion_trace", "enabled": True})
    try:
        request(port, {"cmd": "input", "steps": "windowkey down KeyW"})
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            request(port, {"cmd": "screenshot", "scale": 16})
    finally:
        request(port, {"cmd": "input", "steps": "windowkey up KeyW"})
        trace = request(port, {"cmd": "motion_trace", "enabled": False})
        output.write_text(json.dumps(trace, indent=2) + "\n")
    validate(trace)


def validate(trace):
    frames = [f for f in trace["frames"] if f["hero"]["sim_position"] is not None]
    assert len(frames) >= 10, f"only {len(frames)} synchronized frames"
    rendered = sum(f["rendered"] for f in frames)
    assert rendered >= 10, f"only {rendered} rendered frames"
    start = frames[0]["hero"]["sim_position"]
    end = frames[-1]["hero"]["sim_position"]
    distance = math.dist(start[:2], end[:2])
    assert distance > 1, f"walk only covered {distance:.6f} m"
    assert all(f["target"] is not None for f in frames), "follow camera was inactive"
    errors = [math.dist(f["hero"]["sim_position"][:2], f["target"][:2]) for f in frames]
    assert max(errors) < 0.0001, f"camera target trails current hero by {max(errors):.6f} m"
    assert all(f["sim_ticks"] <= 1 for f in frames), "unexpected simulation Tick count"
    print(f"PASS {rendered} rendered / {len(frames)} synchronized frames, "
          f"{distance:.3f} m walked, max target XY error {max(errors):.9f} m")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port", type=int)
    parser.add_argument("output", type=Path)
    parser.add_argument("--seconds", type=float, default=3)
    args = parser.parse_args()
    check(args.port, args.output, args.seconds)
