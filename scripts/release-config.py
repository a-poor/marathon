#!/usr/bin/env python3
"""Derive smoke-test jobs and per-target snapshots from the release configuration."""

import argparse
import json
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parents[1]
RUNNERS = {
    "x86_64-unknown-linux-gnu": "ubuntu-24.04",
    "aarch64-unknown-linux-gnu": "ubuntu-24.04-arm",
    "x86_64-apple-darwin": "macos-15-intel",
    "aarch64-apple-darwin": "macos-15",
    "x86_64-pc-windows-gnu": "windows-2025",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", nargs="?", help="omit to print the CI matrix")
    args = parser.parse_args()
    config = yaml.safe_load((ROOT / ".goreleaser.yaml").read_text(encoding="utf-8"))
    targets = []
    for build in config["builds"]:
        if build["builder"] != "rust":
            parser.error("add smoke-test support for the new release builder")
        targets.extend(build["targets"])
    if not targets or len(targets) != len(set(targets)):
        parser.error("expected unique, explicit release targets")
    unknown = set(targets) - RUNNERS.keys()
    if unknown:
        parser.error(f"add native smoke runners for: {sorted(unknown)}")

    if args.target is None:
        # New release targets must have a runner; they cannot silently miss CI.
        print(json.dumps({"include": [
            {"target": target, "os": RUNNERS[target]} for target in targets
        ]}))
        return
    if args.target not in targets:
        parser.error(f"not a configured release target: {args.target}")

    # Keep the release builder, flags, and archive rules. Narrow the targets and
    # isolate generated files from real releases.
    config["builds"] = [
        {**build, "targets": [args.target]}
        for build in config["builds"] if args.target in build["targets"]
    ]
    # A Windows-only snapshot has no archive that a Homebrew cask can install.
    # Keep cask generation for Linux/macOS snapshots and the full release config.
    if "-windows-" in args.target:
        config.pop("homebrew_casks", None)
    config["dist"] = "target/release-smoke"
    output = ROOT / "target/release-smoke.yaml"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(yaml.safe_dump(config, sort_keys=False), encoding="utf-8")
    print(output)


if __name__ == "__main__":
    main()
