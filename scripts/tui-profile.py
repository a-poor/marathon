#!/usr/bin/env python3
"""Coarse packaged-TUI latency/RSS probes; requires tmux and Unix ps, not a benchmark."""

import argparse
import json
from pathlib import Path
import runpy
import shlex
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
Tui = runpy.run_path(str(ROOT / "scripts/tui-smoke.py"))["Tui"]


def rss(pid):
    return int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(pid)], text=True))


def document(cells):
    return "\n".join(
        f"## Step {i}: café 界\n\nSome context for a local operation.\n\n"
        f"```sh id=step-{i}\nprintf 'step {i}\\n'\n```\n"
        for i in range(cells)
    ) + "\nprofile-end-marker\n"


def navigation(binary, work, cells):
    started = time.monotonic()
    tui = Tui(binary, work, document(cells))
    try:
        startup = time.monotonic() - started
        started = time.monotonic()
        tui.keys("/")
        tui.text("profile-end-marker")
        tui.shown("1/1")
        search = time.monotonic() - started
        result = {"cells": cells, "startup_ms": round(startup * 1000),
                  "search_ms": round(search * 1000), "rss_kib": rss(tui.pid)}
        tui.quit()
        return result
    finally:
        tui.close()


def output(binary, work, cells):
    python = shlex.quote(sys.executable)
    book = f'''```sh
{python} -c 'import sys,time; chunk="x"*1023+"\\n"; [(sys.stdout.write(chunk*1024), sys.stdout.flush(), time.sleep(.025)) for _ in range(32)]'
printf done > completed
```
''' + document(cells)
    tui = Tui(binary, work, book)
    try:
        baseline = rss(tui.pid)
        peak = baseline
        started = time.monotonic()
        tui.keys("Down", "Enter")
        deadline = started + 120
        while not (work / "completed").exists():
            if time.monotonic() > deadline:
                raise RuntimeError("32 MiB output did not finish within 120 seconds")
            peak = max(peak, rss(tui.pid))
            time.sleep(0.1)
        tui.wait(lambda: "done" in tui.screen().splitlines()[-1], "completed output capture")
        final = rss(tui.pid)
        peak = max(peak, final)
        result = {"context_cells": cells, "output_mib": 32,
                  "elapsed_s": round(time.monotonic() - started, 2),
                  "baseline_rss_kib": baseline, "peak_sampled_rss_kib": peak,
                  "final_rss_kib": final}
        tui.quit()
        return result
    finally:
        tui.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path)
    args = parser.parse_args()
    unpack = runpy.run_path(str(ROOT / "scripts/release-smoke.py"))["unpack"]
    with tempfile.TemporaryDirectory(prefix="mrthn-perf-") as directory:
        root = Path(directory)
        binary = unpack(args.dist.resolve(), root)
        for cells in (100, 1000, 5000):
            print(json.dumps(navigation(binary, root / f"nav-{cells}", cells)), flush=True)
        for cells in (0, 1000):
            print(json.dumps(output(binary, root / f"out-{cells}", cells)), flush=True)


if __name__ == "__main__":
    main()
