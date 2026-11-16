#!/usr/bin/env python3
"""A bisect_merges.py check: prints the City Elf opening's bunk-region PSNR (dB) against BUNK_REF.

Usage: tools/triage/check_bunk_psnr.py <thedas-app> <out-dir>
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import checks  # noqa: E402

if __name__ == "__main__":
    out = Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    print(f"{checks.bunk_psnr(Path(sys.argv[1]), out):.3f}")
