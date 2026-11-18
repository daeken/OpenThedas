#!/usr/bin/env python3
"""Check the orders lesson through the hidden window's real mouse handlers.

Run under cargo-slot.sh and capped.sh on the shared Mac:
  THEDAS_INSTALL=... python3 tools/triage/check_orders_tutorial.py BIN SAVE OUT PORT
SAVE should be an exploration save with clear ground at 70% width, 60% height.
"""

import base64
import json
import subprocess
import sys
from pathlib import Path

import triagelib as t


def check(binary, save, out, port):
    out.mkdir(parents=True, exist_ok=True)
    if subprocess.run(["lsof", f"-iTCP:{port}", "-sTCP:LISTEN"],
                      capture_output=True).returncode == 0:
        raise RuntimeError(f"control port {port} is already in use")
    game = t.Control(binary, save, port, name="orders-tutorial", log=out / "engine.log")
    try:
        def world():
            return json.loads(game.req({"cmd": "state"})["save"])["world"]

        def capture(name):
            shot = game.req({"cmd": "screenshot"})
            data = base64.b64decode(shot["png"])
            (out / f"{name}.png").write_bytes(data)
            # PNG IHDR gives physical pixel dimensions (including Retina scaling).
            return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")

        def training():
            return world()["training"]

        game.input("trainingmode 2; run 1000")
        assert training()["expected"] == 1073, training()
        text = game.input("call popuplayer.gfx ExternalCommands.GetTlkString 398582")
        tag = "<RightClickDefaultCapped/>"
        assert tag in str(text.get("log")), text
        button = game.input(f"call popuplayer.gfx ExternalCommands.ReplaceDesignerTags {tag}")
        assert 'Some(String("Right-click"))' in str(button.get("log")), button
        width, height = capture("before")
        x, y = round(width * 0.70), round(height * 0.60)
        point = f"{x},{y}"

        # A drag that returns to its start must not count as the requested move.
        game.input(f"rpress {point}; hover {x + 80},{y}; hover {point}; rrelease {point}; run 200")
        assert training()["expected"] == 1073, "drag advanced the lesson"

        # A press on a menu followed by release over the world must not issue an order.
        menu = game.input("key j; run 1000")
        assert menu.get("screens") == "{Journal}", menu
        game.input(f"rpress {point}; key j; run 1000; rrelease {point}; run 200")
        assert training()["expected"] == 1073, "menu press leaked into the world"

        # Use the button named by the resolved TLK, then inspect the queued command and
        # the script-driven handoff to the Space/unpause step (not just a log message).
        game.input(f"rclick {point}; run 500")
        w = world()
        orders = w["objects"][str(w["controlled"] or w["hero"])]["commands"]
        assert any(c["name"] == "CommandMoveToLocation" for c in orders), orders
        assert w["training"]["step"] == 9, w["training"]
        assert w["training"]["expected"] == 1069, w["training"]
        assert w["training"]["paused"], "the lesson must wait for Space"
        capture("after")
        game.input("key space; run 300")
        assert training()["mode"] == 0, training()

        # Existing left-click ground orders stay usable after the lesson ends.
        game.input("key space")
        w = world()
        previous = w["objects"][str(w["controlled"] or w["hero"])]["commands"]
        game.input(f"click {x - 80},{y}; run 100")
        w = world()
        orders = w["objects"][str(w["controlled"] or w["hero"])]["commands"]
        assert any(c["name"] == "CommandMoveToLocation" for c in orders), orders
        assert orders != previous, "left-click did not replace the previous move"
        (out / "result.json").write_text(json.dumps({"text": text["log"],
            "resolved_button": button["log"],
            "right_click_training_step": 9, "left_click_move": True}, indent=2) + "\n")
        print("PASS: resolved Right-click queues a move and advances to Space; drag/menu guarded; left-click retained")
    finally:
        game.close()


if __name__ == "__main__":
    check(Path(sys.argv[1]).resolve(), Path(sys.argv[2]).resolve(),
          Path(sys.argv[3]).resolve(), int(sys.argv[4]))
