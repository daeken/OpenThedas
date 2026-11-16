"""Shared helpers for the triage tools (Python 3 standard library only).

Paths come from the environment, with defaults for the shared Mac:
  THEDAS_TESTDATA   test assets (saves, captures, binary cache)   ~/projects/OpenThedas-testdata
  THEDAS_INSTALL    the Dragon Age: Origins install                 (required for game runs)
  TRIAGE_WORKTREE   the git worktree builds happen in               the repository holding this file
  TRIAGE_SCRATCH    scratch HOMEs and temporary files               <tmp>/thedas-triage
  FFMPEG            ffmpeg, for PSNR                                ffmpeg on PATH
Game runs are headless or in a hidden window, under a scratch HOME, with THEDAS_MODS=none and
no audio, so nothing is sent and the player's own data is not touched.
"""

import json
import os
import re
import shutil
import socket
import subprocess
import tempfile
import time
from pathlib import Path

HOME = Path.home()
TESTDATA = Path(os.environ.get("THEDAS_TESTDATA", HOME / "projects" / "OpenThedas-testdata"))
WORKTREE = Path(os.environ.get("TRIAGE_WORKTREE", Path(__file__).resolve().parents[2]))
SCRATCH = Path(os.environ.get("TRIAGE_SCRATCH", Path(tempfile.gettempdir()) / "thedas-triage"))
FFMPEG = os.environ.get("FFMPEG") or shutil.which("ffmpeg") or "/opt/homebrew/bin/ffmpeg"
CARGO = os.environ.get("CARGO") or shutil.which("cargo") or str(HOME / ".cargo" / "bin" / "cargo")
BIN_CACHE = TESTDATA / "bin"


def install() -> str:
    path = os.environ.get("THEDAS_INSTALL")
    if not path:
        raise SystemExit("THEDAS_INSTALL is not set (the Dragon Age: Origins install)")
    return path


def short(sha: str) -> str:
    return git("rev-parse", "--short=8", sha).strip()


def git(*args: str, cwd: Path = WORKTREE) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


def sh(cmd, cwd=None, env=None, log=None, timeout=None) -> int:
    """Runs a command, its output to `log` (a path) or inherited; returns the exit code."""
    out = open(log, "w") if log else None
    try:
        return subprocess.run(cmd, cwd=cwd, env=env, stdout=out, stderr=subprocess.STDOUT if out else None,
                              timeout=timeout).returncode
    except subprocess.TimeoutExpired:
        return 124
    finally:
        if out:
            out.close()


def scratch_home(name: str) -> Path:
    home = SCRATCH / "home" / name
    if home.exists():
        shutil.rmtree(home)
    home.mkdir(parents=True)
    return home


def game_env(home: Path, hidden: bool = False, **extra) -> dict:
    env = dict(os.environ, HOME=str(home), THEDAS_MODS="none", THEDAS_NO_AUDIO="1", THEDAS_INSTALL=install())
    env.pop("THEDAS_HIDDEN_WINDOW", None)
    if hidden:
        env["THEDAS_HIDDEN_WINDOW"] = "1"
    env.update({k: str(v) for k, v in extra.items()})
    return env


def build(sha: str, packages=("thedas-app", "thedas-mcp"), log=None) -> dict:
    """Release-builds `packages` at `sha` in the worktree (detached), caching each binary as
    BIN_CACHE/<package>-<sha8>. Returns {package: path}; a cached one is not built again."""
    s = short(sha)
    want = {p: BIN_CACHE / f"{p}-{s}" for p in packages}
    if all(p.exists() for p in want.values()):
        return want
    git("checkout", "-q", "--detach", sha)
    args = [CARGO, "build", "--release"] + [a for p in packages for a in ("-p", p)]
    if sh(args, cwd=WORKTREE, log=log) != 0:
        raise RuntimeError(f"build of {s} failed" + (f" (see {log})" if log else ""))
    BIN_CACHE.mkdir(parents=True, exist_ok=True)
    for p, dest in want.items():
        shutil.copy2(WORKTREE / "target" / "release" / p, dest)
    return want


def sha256(path) -> str:
    import hashlib
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def headless(binary, steps: str, capture: Path, save=None, new_game=False, size="1280x720",
             camera=None, name="run", extra_env=None, timeout=600) -> str:
    """A headless run (no window): load `save` or start a new game, play `steps`, capture.
    Returns the run's log text (also written beside the capture as .log)."""
    home = scratch_home(name)
    args = [str(binary), "--size", size, "--no-audio", "--saves", str(home / "saves"), "--capture", str(capture)]
    if save:
        args += ["--save", str(save)]
    if new_game:
        args += ["--new-game"]
    if camera:
        args += ["--camera", camera]
    args += ["--steps", steps]
    log = Path(str(capture) + ".log")
    sh(args, env=game_env(home, **(extra_env or {})), log=log, timeout=timeout)
    return log.read_text(errors="replace")


def psnr(a: Path, b: Path, crop: str) -> float:
    """PSNR (dB) of the `crop` region (ffmpeg's w:h:x:y) between two images; inf when equal."""
    p = subprocess.run([FFMPEG, "-nostdin", "-hide_banner", "-loglevel", "info", "-i", str(a), "-i", str(b),
                        "-lavfi", f"[0:v]crop={crop}[x];[1:v]crop={crop}[y];[x][y]psnr", "-f", "null", "-"],
                       capture_output=True, text=True)
    m = re.search(r"average:([0-9.]+|inf)", p.stderr)
    if not m:
        raise RuntimeError("ffmpeg gave no PSNR: " + p.stderr[-300:])
    return float(m.group(1))


class Control:
    """A hidden-window game driven over --control (one JSON request per line)."""

    def __init__(self, binary, save, port, name="control", size="1280x720", log=None):
        self.home = scratch_home(name)
        self.log = log or (SCRATCH / f"{name}.log")
        args = [str(binary), "--size", size, "--save", str(save), "--no-intro", "--no-audio",
                "--control", f"127.0.0.1:{port}"]
        self.proc = subprocess.Popen(args, env=game_env(self.home, hidden=True),
                                     stdout=open(self.log, "w"), stderr=subprocess.STDOUT)
        for _ in range(300):
            try:
                self.sock = socket.create_connection(("127.0.0.1", port))
                break
            except OSError:
                if self.proc.poll() is not None:
                    raise RuntimeError(f"the game exited early (see {self.log})")
                time.sleep(0.2)
        else:
            raise RuntimeError("no control connection")
        self.f = self.sock.makefile("rw")
        last = None
        for _ in range(600):  # until the world runs (past any loading screen)
            st = self.req({"cmd": "status"})
            if last is not None and st.get("time_ms") != last and "Load" not in str(st.get("screens")):
                break
            last = st.get("time_ms")
            time.sleep(0.25)

    def req(self, obj) -> dict:
        self.f.write(json.dumps(obj) + "\n")
        self.f.flush()
        line = self.f.readline()
        return json.loads(line) if line else {}

    def input(self, steps: str) -> dict:
        return self.req({"cmd": "input", "steps": steps})

    def close(self):
        self.proc.kill()
        self.proc.wait()


def free_port(start=7910, end=7929) -> int:
    """A free loopback port in triage's helper range."""
    for port in range(start, end + 1):
        with socket.socket() as s:
            if s.connect_ex(("127.0.0.1", port)) != 0:
                return port
    raise RuntimeError("no free control port")
