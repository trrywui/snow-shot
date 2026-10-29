#!/usr/bin/env python3
"""Stage a native worker and notices. Does not sign or publish a release."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
SUPPORTED_TARGETS = (
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
)


def publish(staging, destination):
    """Only completed bundles replace the last successful package; roll back a failed rename."""
    backup = destination.with_name(f".{destination.name}.previous-{uuid.uuid4().hex}")
    had_previous = destination.exists()
    if had_previous:
        destination.replace(backup)
    try:
        staging.replace(destination)
    except BaseException:
        if had_previous:
            try:
                backup.replace(destination)
            except OSError as error:
                raise RuntimeError(f"Package publication and rollback failed; previous bundle remains at {backup}") from error
        raise
    if had_previous:
        try:
            shutil.rmtree(backup)
        except OSError as error:
            raise RuntimeError(f"New package published at {destination}, but previous bundle cleanup failed at {backup}") from error


def main(arguments=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", default=None)
    args = parser.parse_args(arguments)
    host = next(line.split(": ", 1)[1] for line in subprocess.check_output(["rustc", "-vV"], cwd=ROOT, text=True).splitlines() if line.startswith("host:"))
    target = args.target or host
    if target not in SUPPORTED_TARGETS:
        raise RuntimeError(f"Unsupported package target: {target}")
    if target != host:
        raise RuntimeError("Package on the native target after its platform validation; cross-checking Rust is insufficient")
    cargo_target = (ROOT / "target/cargo").resolve()
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "snow-image-worker",
                    "--bin", "snow-image-worker", "--target", target, "--target-dir", str(cargo_target)],
                   cwd=ROOT, check=True)
    destination = ROOT / "target/dist" / target
    destination.parent.mkdir(parents=True, exist_ok=True)
    name = "snow-image-worker.exe" if "windows" in target else "snow-image-worker"
    with tempfile.TemporaryDirectory(prefix=f".{target}.staging-", dir=destination.parent) as temporary:
        staging = Path(temporary)
        shutil.copy2(cargo_target / target / "release" / name, staging / name)
        for file in ("LICENSE", "THIRD_PARTY_NOTICES.md", "Cargo.lock"):
            shutil.copy2(ROOT / file, staging / file)
        shutil.copy2(ROOT / "native/sources.json", staging / "native-sources.json")
        subprocess.run([sys.executable, str(ROOT / "scripts/licenses.py"), "--output", str(staging / "third-party")], cwd=ROOT, check=True)
        (staging / "manifest.json").write_text(json.dumps({"schema": 1, "ipc_version": 1, "target": target,
            "worker_sha256": hashlib.sha256((staging / name).read_bytes()).hexdigest(),
            "cargo_lock_sha256": hashlib.sha256((staging / "Cargo.lock").read_bytes()).hexdigest(),
            "native_sources_sha256": hashlib.sha256((staging / "native-sources.json").read_bytes()).hexdigest()}, indent=2) + "\n")
        publish(staging, destination)
    print(f"Staged unsigned worker: {destination}")


if __name__ == "__main__":
    main()
