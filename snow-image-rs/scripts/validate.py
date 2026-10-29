#!/usr/bin/env python3
"""Scoped native-machine validation. Never runs the containing repository suite."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--prepare", action="store_true", help="Explicitly fetch verified native sources and build them first")
    parser.add_argument("--performance", action="store_true", help="Run the related release benchmark matrix")
    parser.add_argument("--output", type=Path, default=ROOT / "target/validation/native.json")
    args = parser.parse_args()
    compiler = subprocess.check_output(["rustc", "-vV"], text=True)
    host = next(line.split(": ", 1)[1] for line in compiler.splitlines() if line.startswith("host:"))
    if host not in ("aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc"):
        raise RuntimeError("Native validation must run on a supported native target")
    report = {"schema": 1, "target": host, "compiler": compiler, "checks": []}
    env = os.environ.copy()
    env["SNOW_IMAGE_WORKER"] = str(ROOT / "target/cargo/debug" / ("snow-image-worker.exe" if os.name == "nt" else "snow-image-worker"))

    def run(command):
        print(" ".join(command), flush=True)
        start = time.monotonic()
        result = subprocess.run(command, cwd=ROOT, env=env)
        report["checks"].append({"command": command, "exit_code": result.returncode, "seconds": time.monotonic() - start})
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
        result.check_returncode()

    if args.prepare:
        run([sys.executable, "native/build.py", "fetch"])
        run([sys.executable, "native/build.py", "build"])
    run([sys.executable, "native/build.py", "verify"])
    run([sys.executable, "scripts/test_build_support.py"])
    run(["cargo", "fmt", "--all", "--check"])
    run(["cargo", "build", "--locked", "-p", "snow-image-worker"])
    for package in ("snow-image-core", "snow-image-protocol", "snow-image-transport", "snow-image-native-sys", "snow-image-codecs", "snow-image-worker", "snow-image-rs"):
        run(["cargo", "test", "--locked", "-p", package] + (["--features", "native-backends"] if package == "snow-image-codecs" else []))
    for test in ("service", "faults"):
        run(["cargo", "test", "--locked", "-p", "snow-image-rs", "--test", test, "--", "--ignored", "--test-threads=1"])
    run(["cargo", "clippy", "--locked", "-p", "snow-image-rs", "-p", "snow-image-worker", "-p", "snow-image-core", "-p", "snow-image-codecs", "-p", "snow-image-native-sys", "-p", "snow-image-protocol", "-p", "snow-image-transport", "-p", "xtask", "--all-targets", "--", "-D", "warnings"])
    run([sys.executable, "scripts/licenses.py"])
    if args.performance:
        run(["cargo", "build", "--locked", "--profile", "performance", "-p", "snow-image-worker", "-p", "xtask"])
        tool = ROOT / "target/cargo/performance" / ("xtask.exe" if os.name == "nt" else "xtask")
        run([str(tool), "bench", "--output", str(ROOT / "target/benchmarks" / f"{host}.json")])
    print(f"Validation report: {args.output}")


if __name__ == "__main__":
    main()
