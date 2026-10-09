#!/usr/bin/env python3
"""Smoke-test the binary from a single-target GoReleaser snapshot archive."""

import argparse
import hashlib
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def unpack(dist, destination):
    archives = list(dist.glob("*.tar.gz")) + list(dist.glob("*.zip"))
    require(len(archives) == 1, f"expected one release archive, found {archives}")
    archive = archives[0]
    checksums = list(dist.glob("*checksums.txt"))
    require(len(checksums) == 1, "expected one release checksum file")
    entries = dict(line.split(maxsplit=1)[::-1]
                   for line in checksums[0].read_text().splitlines())
    require(hashlib.sha256(archive.read_bytes()).hexdigest() == entries.get(archive.name),
            f"missing or incorrect checksum for {archive.name}")

    binary_name = "marathon.exe" if os.name == "nt" else "marathon"
    binary = destination / binary_name
    required_files = {binary_name, "README.md", "LICENSE-MIT.txt", "LICENSE-APACHE.txt"}
    if archive.suffix == ".zip":
        require(os.name == "nt", "unexpected Windows archive on a Unix runner")
        with zipfile.ZipFile(archive) as package:
            require(required_files <= set(package.namelist()), "incomplete release archive")
            binary.write_bytes(package.read(binary_name))
    else:
        require(os.name != "nt", "expected a zip archive for Windows")
        with tarfile.open(archive) as package:
            require(required_files <= set(package.getnames()), "incomplete release archive")
            member = package.getmember(binary_name)
            require(member.isfile() and member.mode & 0o111, "binary is not executable")
            binary.write_bytes(package.extractfile(member).read())
            binary.chmod(member.mode & 0o777)
    print(f"Testing {archive.name}", flush=True)
    return binary


def smoke(binary, work):
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("MARATHON_SMOKE_")}

    def run(*args, status=0):
        result = subprocess.run(
            [str(binary), *args], cwd=work, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30,
        )
        require(result.returncode == status,
                f"{args}: expected exit {status}, got {result.returncode}\n"
                f"stdout: {result.stdout!r}\nstderr: {result.stderr!r}")
        return result

    version = re.search(r'^version = "([^"]+)"',
                        (ROOT / "Cargo.toml").read_text(), re.MULTILINE).group(1)
    require(run("--version").stdout.strip() == f"marathon {version}".encode(),
            "packaged binary version differs from Cargo.toml")
    require(b"Usage:" in run("--help").stdout, "missing CLI help")
    for shell in ("bash", "zsh", "fish", "elvish", "powershell"):
        require(b"marathon" in run("completions", shell).stdout,
                f"empty or incorrect {shell} completions")

    run("new", "starter.md")
    run("validate", "starter.md")
    run("check", "starter.md")
    require(b"#1" in run("exec", "starter.md", "--list").stdout,
            "scaffold has no runnable cell")
    original = (work / "starter.md").read_bytes()
    run("new", "starter.md", status=1)
    require((work / "starter.md").read_bytes() == original, "scaffold was overwritten")
    (work / "invalid.md").write_text(
        '```json mrthn=input\n{"type":"input","prompt":"?","target":"BAD-NAME"}\n```\n'
    )
    run("validate", "invalid.md", status=1)
    for sample in sorted((ROOT / "samples").glob("*.md")):
        run("validate", str(sample))

    if os.name == "nt":
        print("PASS: Windows archive/startup/CLI checks; POSIX execution is not covered.")
        return

    require(run("exec", "starter.md", "--yes").stdout == b"Hello from your new runbook\n",
            "scaffold execution failed")
    (work / "success.md").write_text(r'''---
env:
  MARATHON_SMOKE_ENV: frontmatter
---
```sh id=prepare
printf '%s' "$TMP_DIR" > scratch-path
printf 'artifact' > "$TMP_DIR/artifact"
```
```json mrthn=input id=answer
{"type":"input","prompt":"Label?","target":"MARATHON_SMOKE_ANSWER","default":"default"}
```
```sh id=consume needs=prepare
test "$MARATHON_SMOKE_ENV" = override
test "$(cat "$TMP_DIR/artifact")" = artifact
printf '%s\n' "$MARATHON_SMOKE_ANSWER"
printf 'stderr\n' >&2
printf 'raw\r\n\000\377'
```
```sh skip=true
exit 99
```
```sh id=summary
printf 'summary: %s\n' "$MARATHON_SMOKE_ANSWER"
```
''')
    run("validate", "success.md")
    listing = run("exec", "success.md", "--list").stdout
    require(all(ref in listing for ref in (b"(prepare)", b"(answer)", b"(consume)", b"(summary)")),
            "missing cell references")
    require(not (work / "scratch-path").exists(), "--list executed a command")
    output = run("exec", "success.md", "--yes", "-e", "MARATHON_SMOKE_ENV=override")
    require(output.stdout == b"default\nstderr\nraw\r\n\x00\xffsummary: default\n",
            f"execution/output mismatch: {output.stdout!r}")
    require(not Path((work / "scratch-path").read_text()).exists(),
            "scratch directory survived successful execution")
    output = run("exec", "success.md", "--yes", "--cell", "summary",
                 "-e", "MARATHON_SMOKE_ANSWER=supplied")
    require(output.stdout == b"summary: supplied\n", "partial execution replayed skipped cells")

    (work / "failure.md").write_text('''```sh
printf '%s' "$TMP_DIR" > failed-scratch-path
printf 'before failure\\n'
exit 23
```
```sh
touch should-not-exist
```
''')
    output = run("exec", "failure.md", "--yes", status=23)
    require(output.stdout == b"before failure\n", "diagnostics leaked into command stdout")
    require(not (work / "should-not-exist").exists(), "execution continued after failure")
    require(not Path((work / "failed-scratch-path").read_text()).exists(),
            "scratch directory survived failed execution")
    print("PASS: archive, CLI, execution, partial selection, byte output, exit status, cleanup.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path, help="single-target GoReleaser output directory")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="marathon-release-smoke-") as directory:
        root = Path(directory)
        binary = unpack(args.dist.resolve(), root)
        work = root / "work"
        work.mkdir()
        smoke(binary, work)


if __name__ == "__main__":
    main()
