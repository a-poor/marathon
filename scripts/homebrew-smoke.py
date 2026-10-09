#!/usr/bin/env python3
"""Install/test/uninstall a generated snapshot cask on a disposable macOS runner."""

import argparse
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def xattr(*arguments):
    return subprocess.run(["/usr/bin/xattr", *map(str, arguments)], check=True,
                          capture_output=True, text=True, timeout=30).stdout.strip()


def prepare(dist, destination):
    archives = list(dist.glob("*.tar.gz"))
    if len(archives) != 1:
        raise RuntimeError("expected one native snapshot archive")
    source = (dist / "homebrew/Casks/marathon.rb").read_text(encoding="utf-8")
    # Test the generated DSL, checksum, hooks, and completion declarations as-is.
    # Only redirect its download to this job's unpublished snapshot archive.
    source, count = re.subn(r'(?m)^(\s*url )"[^"]+"',
                            lambda m: m[1] + '"' + archives[0].resolve().as_uri() + '"', source)
    if count != 1:
        raise RuntimeError("expected exactly one platform URL in the snapshot cask")
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(source, encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path)
    parser.add_argument("--prepare-only", type=Path, metavar="CASK",
                        help="write a local cask for inspection without installing")
    parser.add_argument("--quarantine-only", action="store_true",
                        help="test hook ordering with a disposable binary; do not install")
    args = parser.parse_args()
    if args.prepare_only and args.quarantine_only:
        parser.error("--prepare-only and --quarantine-only cannot be combined")
    if args.prepare_only:
        prepare(args.dist.resolve(), args.prepare_only)
        return
    if sys.platform != "darwin":
        parser.error("Homebrew quarantine checks require macOS")
    if not args.quarantine_only and os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        parser.error("installation is restricted to disposable GitHub-hosted runners; "
                     "use --prepare-only or --quarantine-only locally")

    env = {**os.environ, "HOMEBREW_NO_AUTO_UPDATE": "1",
           "HOMEBREW_NO_INSTALL_CLEANUP": "1", "HOMEBREW_NO_ANALYTICS": "1"}

    def brew(*arguments, **kwargs):
        # `brew ruby` otherwise persists developer mode in Homebrew's settings.
        command_env = {**env, "HOMEBREW_DEV_CMD_RUN": "1"} if arguments[0] == "ruby" else env
        return subprocess.run(["brew", *arguments], env=command_env, check=True, timeout=180, **kwargs)

    # This regression check also runs locally without touching an installed cask.
    suite = runpy.run_path(str(ROOT / "scripts/release-smoke.py"))
    with tempfile.TemporaryDirectory(prefix="marathon-homebrew-smoke-") as work:
        work = Path(work)
        cask = work / "marathon.rb"
        prepare(args.dist.resolve(), cask)
        binary = suite["unpack"](args.dist.resolve(), work)
        brew("ruby", str(ROOT / "scripts/homebrew-quarantine-smoke.rb"), str(cask), str(binary))
    if args.quarantine_only:
        return

    prefix = Path(brew("--prefix", capture_output=True, text=True).stdout.strip())
    repository = Path(brew("--repository", capture_output=True, text=True).stdout.strip())
    installed = prefix / "bin/marathon"
    completions = [prefix / "etc/bash_completion.d/marathon",
                   prefix / "share/zsh/site-functions/_marathon",
                   prefix / "share/fish/vendor_completions.d/marathon.fish"]
    paths = [installed, prefix / "Caskroom/marathon", *completions]
    if any(path.exists() or path.is_symlink() for path in paths):
        raise RuntimeError("refusing to replace an existing Marathon installation or completion")

    # Homebrew accepts casks within a tap directory; no git repository is needed.
    tap = repository / "Library/Taps/marathon/homebrew-release-smoke"
    tap.mkdir(parents=True, exist_ok=False)
    cask = tap / "Casks/marathon.rb"
    cask_name = "marathon/release-smoke/marathon"
    attempted_install = False
    trusted = False
    try:
        prepare(args.dist.resolve(), cask)
        # Recent Homebrew requires explicit trust for locally generated taps.
        if "trust" in brew("commands", "--quiet", capture_output=True, text=True).stdout.split():
            brew("trust", "--cask", cask_name)
            trusted = True
        # Homebrew quarantines cask downloads by default. Current versions no
        # longer accept the old --quarantine flag on fetch or install.
        brew("fetch", "--cask", str(cask))
        cached = Path(brew("--cache", "--cask", str(cask), capture_output=True, text=True).stdout.strip())
        # A file:// snapshot must exercise the same quarantine path as a download.
        quarantine = xattr("-p", "com.apple.quarantine", cached)
        if not quarantine or int(quarantine.split(";", 1)[0], 16) & 0x0040:
            raise RuntimeError("expected a quarantined download without prior user approval")
        attempted_install = True
        brew("install", "--cask", str(cask))
        if "com.apple.quarantine" in xattr(installed.resolve()).splitlines():
            raise RuntimeError("installed binary is still quarantined")
        with tempfile.TemporaryDirectory(prefix="marathon-installed-smoke-") as work:
            suite["smoke"](installed, Path(work))
        for completion in completions:
            if "marathon" not in completion.read_text(encoding="utf-8"):
                raise RuntimeError(f"missing or invalid installed completion: {completion}")
    finally:
        try:
            if attempted_install:
                brew("uninstall", "--cask", "--force", str(cask))
        finally:
            try:
                if trusted:
                    brew("untrust", "--cask", cask_name)
            finally:
                shutil.rmtree(tap)
    if any(path.exists() or path.is_symlink() for path in paths):
        raise RuntimeError("uninstall left a binary, completion, or cask directory behind")
    print("PASS: generated Homebrew cask installs, runs, generates completions, and uninstalls.")


if __name__ == "__main__":
    main()
