#!/usr/bin/env python3
"""Exercise a packaged Marathon TUI in an isolated tmux server (macOS/Linux)."""

import argparse
import os
from pathlib import Path
import re
import runpy
import shutil
import signal
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


class Tui:
    def __init__(self, binary, work, book):
        self.work = work
        work.mkdir()
        (work / "book.md").write_text(book, encoding="utf-8")
        self.socket = str(work / "tmux.sock")
        # Ignore personal tmux configuration and never use the user's server.
        self.tmux("new-session", "-d", "-x", "120", "-y", "40", "-c", str(work),
                  str(binary), "run", "book.md")
        self.tmux("set-option", "remain-on-exit", "on")
        self.pid = int(self.tmux("display-message", "-p", "#{pane_pid}"))
        try:
            self.wait(lambda: "ready" in self.screen(), "initial frame")
        except Exception:
            self.close()
            raise

    def tmux(self, *args):
        return subprocess.run(
            ["tmux", "-S", self.socket, "-f", "/dev/null", *args],
            check=True, capture_output=True, text=True, timeout=10,
        ).stdout

    def screen(self):
        return self.tmux("capture-pane", "-p")

    def keys(self, *keys):
        self.tmux("send-keys", *keys)

    def text(self, text):
        self.tmux("send-keys", "-l", text)

    def wait(self, predicate, label, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.05)
        raise RuntimeError(f"Timed out: {label}\n{self.screen()}")

    def shown(self, text):
        self.wait(lambda: text in self.screen(), repr(text))

    def file(self, name):
        path = self.work / name
        self.wait(lambda: path.exists() and path.stat().st_size > 0, name)
        return path.read_text()

    def quit(self):
        self.keys("C-c")
        self.wait(lambda: self.tmux("display-message", "-p", "#{pane_dead}").strip() == "1",
                  "clean TUI exit")
        require(self.tmux("display-message", "-p", "#{pane_dead_status}").strip() == "0",
                f"TUI failed on exit\n{self.screen()}")

    def close(self):
        if alive(self.pid):
            os.kill(self.pid, signal.SIGTERM)
            self.wait(lambda: not alive(self.pid), "shutdown after test")
        self.tmux("kill-server")


def recovery(binary, work):
    tui = Tui(binary, work, '''```sh id=prepare
printf '%s' "$TMP_DIR" > scratch
printf 'once\n' >> prepared
```
```json mrthn=input id=choice
{"type":"select","prompt":"Pick a region","target":"REGION","option_file":"choices.txt"}
```
```sh id=retry needs=prepare
test -f allow
printf '%s' "$REGION" > result
```
```sh id=finish
printf 'done' > finished
```
''')
    try:
        tui.keys("r")
        tui.shown("choices.txt")
        tui.shown("!")
        require(not (work / "result").exists(), "input failure advanced execution")
        tui.keys("Escape")
        tui.shown("r remaining")
        (work / "choices.txt").write_text("east\nwest\n")
        tui.keys("Enter")
        tui.shown("east")
        tui.keys("Enter", "r")
        tui.shown("exit 1")
        require(not (work / "finished").exists(), "failed command advanced execution")
        (work / "allow").touch()
        tui.keys("r")
        tui.file("finished")
        require(tui.file("result") == "east", "input answer was not retained")
        require(tui.file("prepared") == "once\n", "successful prerequisite was replayed")
        scratch = Path(tui.file("scratch"))
        tui.quit()
        require(not scratch.exists(), "scratch survived quit")
    finally:
        tui.close()
    print("PASS: run-remaining, input error/reload, failure/retry, retained state, cleanup")


def concurrency(binary, work):
    tui = Tui(binary, work, '''```sh id=first
printf '%s' "$TMP_DIR" > scratch
printf '%s' "$$" > first.pid
sleep 60 &
printf '%s' "$!" > child.pid
wait
```
```sh id=second
printf '%s' "$$" > second.pid
sleep 60
```
```sh id=dependent needs=first
printf 'bad' > forbidden
```
''')
    try:
        tui.keys("G", "Enter")
        tui.shown("blocked")
        require(not (work / "forbidden").exists(), "prerequisite guard failed")
        tui.keys("g", "Down", "Enter")
        first = int(tui.file("first.pid"))
        child = int(tui.file("child.pid"))
        tui.keys("Enter")
        second = int(tui.file("second.pid"))
        require(alive(first) and alive(second), "manual runs did not overlap")
        tui.keys("g", "Down", "BSpace")
        tui.wait(lambda: not alive(first) and not alive(child), "interrupt process group")
        require(alive(second), "interrupt canceled unrelated command")
        scratch = Path(tui.file("scratch"))
        tui.quit()
        tui.wait(lambda: not alive(second), "quit cleanup")
        require(not (work / "forbidden").exists(), "blocked cell executed")
        require(not scratch.exists(), "active-run scratch survived quit")
    finally:
        tui.close()
    print("PASS: prerequisites, concurrent manual runs, interrupt descendants, quit active run")


def display(binary, work):
    tui = Tui(binary, work, '''```sh id=output
awk 'BEGIN {for (i=0; i<4000; i++) printf "row-%04d: abcdefghijklmnopqrstuvwxyz\\n", i}'
printf 'complete' > completed
```

Search needle: café 界 🌍
''')
    try:
        tui.keys("Down", "Enter")
        tui.file("completed")
        tui.shown("done")
        tui.keys("g", "Down", "C-o", "[")
        # Search the page label to bring it into view, even for a tall cell.
        tui.keys("/")
        tui.text("output page")
        tui.shown("1/1")
        tui.keys("Enter")
        tui.shown("output page")
        def page():
            match = re.search(r"output page (\d+)/(\d+)", tui.screen())
            return int(match[1]) if match else None

        previous = page()
        require(previous is not None and previous > 1, "previous page was not displayed")
        tui.keys("]")
        tui.wait(lambda: page() == previous + 1, "next output page")
        tui.keys("[")
        tui.wait(lambda: page() == previous, "previous output page")
        tui.keys("Escape")
        tui.wait(lambda: "/output page" not in tui.screen(), "clear search")
        tui.keys("/")
        tui.text("café 界 🌍")
        tui.shown("1/1")
        tui.keys("Enter", "n", "N")
        tui.shown("Search needle")
        for width, height in [(24, 8), (1, 1), (80, 24), (120, 40)]:
            tui.tmux("resize-window", "-x", str(width), "-y", str(height))
            # Allow a frame at each size, including the one-column viewport.
            time.sleep(0.15)
            require(alive(tui.pid), f"TUI exited at {width}x{height}")
        tui.shown("Search needle")
        tui.keys("Escape")
        tui.wait(lambda: "n/N match" not in tui.screen(), "clear Unicode search")
        tui.keys("?")
        tui.shown("quit")
        tui.keys("Escape")
        tui.quit()
    finally:
        tui.close()
    print("PASS: output paging, Unicode search, navigation, resize, help")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path, help="single-target GoReleaser archive directory")
    args = parser.parse_args()
    if os.name != "posix" or shutil.which("tmux") is None:
        parser.error("requires macOS/Linux and tmux")
    suite = runpy.run_path(str(ROOT / "scripts/release-smoke.py"))
    with tempfile.TemporaryDirectory(prefix="mrthn-tui-") as directory:
        root = Path(directory)
        binary = suite["unpack"](args.dist.resolve(), root)
        for case in (recovery, concurrency, display):
            case(binary, root / case.__name__)


if __name__ == "__main__":
    main()
