#!/usr/bin/env python3
"""Opt-in real APFS exhaustion check for a disposable worker's mapped JPEG output.

This mounts only a newly created private 24 MiB image. It never fills a host
filesystem, uses sudo, or changes existing files. Run outside benchmark windows.
"""

import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import struct
import subprocess
import sys
import tempfile


MIB = 1024 * 1024
IMAGE_BYTES = 24 * MIB
MAX_INPUT_BYTES = 4 * MIB
MAX_OUTPUT_BYTES = 8 * MIB


def command(arguments):
    return subprocess.run(arguments, check=True, capture_output=True, timeout=30)


def policy():
    return {
        "max_input_bytes": MAX_INPUT_BYTES,
        "max_metadata_bytes": MIB,
        "max_animation_frames": 32,
        "working_bytes_per_job": 128 * MIB,
        "total_memory_bytes": 256 * MIB,
        "max_raster_bytes": MAX_OUTPUT_BYTES,
        "max_prepared_bytes": 0,
        "preview_max_dimension": 2048,
        "max_artifact_bytes": MAX_OUTPUT_BYTES,
        "max_temp_bytes": MAX_OUTPUT_BYTES,
        "max_queued_jobs": 1,
        "max_codec_operations": 1,
        "max_codec_threads": 1,
        "worker_address_space_bytes": 1024**4,
        "worker_memory_bytes": 256 * MIB,
    }


def invoke_worker(worker, encoded, backing):
    request = {
        "Run": {
            "version": 1,
            "generation": 1,
            "id": 1,
            "policy": policy(),
            "operation": {
                "Decode": {
                    "apply_orientation": True,
                    "preserve_metadata": False,
                    "preferred_frame": None,
                }
            },
            "input_len": len(encoded),
            "payload_storage": "Pipe",
            "metadata": {"icc": 0, "exif": 0, "xmp": 0},
        }
    }
    control = json.dumps(request, separators=(",", ":")).encode()
    with backing.open("r+b", buffering=0) as output:
        process = subprocess.Popen(
            [str(worker), "--output-backing", "--address-space-limit", str(1024**4)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=output,
            start_new_session=True,
        )
        # EOF is a parent-death signal to this worker. communicate() closes its
        # own stdin handle, so retain one private parent endpoint until exit.
        keepalive = os.dup(process.stdin.fileno())
        try:
            response, _ = process.communicate(
                struct.pack("<I", len(control)) + control + encoded, timeout=10
            )
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)
            raise
        finally:
            os.close(keepalive)
    if response:
        if len(response) < 4:
            raise AssertionError("truncated worker response prefix")
        size = struct.unpack("<I", response[:4])[0]
        if size > MIB or len(response) < size + 4:
            raise AssertionError("invalid worker response size")
        reply = json.loads(response[4 : size + 4])["reply"]
    else:
        reply = None
    return process.returncode, reply


def fingerprint(path):
    digest = hashlib.sha256()
    with path.open("rb", buffering=0) as stream:
        while block := stream.read(64 * 1024):
            digest.update(block)
    return digest.hexdigest()


def exhaust(mount):
    written = 0
    block = os.urandom(64 * 1024)
    with (mount / "filler").open("wb", buffering=0) as filler:
        while written < IMAGE_BYTES:
            try:
                written += filler.write(block)
            except OSError as error:
                if error.errno != errno.ENOSPC:
                    raise
                break
        else:
            raise AssertionError("bounded filler did not reach ENOSPC")
        try:
            os.fsync(filler.fileno())
        except OSError as error:
            if error.errno != errno.ENOSPC:
                raise
    return written


def run(worker, jpeg):
    if sys.platform != "darwin":
        raise RuntimeError("this opt-in filesystem test requires macOS")
    if jpeg.stat().st_size > MAX_INPUT_BYTES:
        raise ValueError("JPEG fixture exceeds the 4 MiB input limit")
    encoded = jpeg.read_bytes()
    root = Path(tempfile.mkdtemp(prefix="snow-image-enospc-"))
    image = root / "private.dmg"
    mount = root / "mount"
    device = None
    detached = False
    attach_attempted = False
    try:
        # Establish the exact reference without retaining any extra physical
        # backing while the 24 MiB image is mounted. The worker output policy
        # rejects images larger than 8 MiB, so this stage is bounded too.
        reference = root / "reference"
        reference.touch()
        status, reply = invoke_worker(worker, encoded, reference)
        if status != 0 or not isinstance(reply, dict) or "BackedImage" not in reply:
            raise AssertionError(f"reference decode failed: {status}, {reply}")
        reference_bytes = reference.stat().st_size
        if not 0 < reference_bytes <= MAX_OUTPUT_BYTES:
            raise AssertionError("reference dimensions exceed the test output limit")
        reference_hash = fingerprint(reference)
        reference.unlink()

        mount.mkdir()
        command([
            "/usr/bin/hdiutil", "create", "-size", "24m", "-fs", "APFS",
            "-layout", "NONE", "-type", "UDIF", "-nospotlight",
            "-volname", "SnowImageENOSPC", str(image),
        ])
        if image.stat().st_blocks * 512 > 32 * MIB:
            raise AssertionError("private image exceeded its physical storage cap")
        attach_attempted = True
        attached = command([
            "/usr/bin/hdiutil", "attach", "-noautoopen", "-mountpoint", str(mount),
            "-plist", str(image),
        ])
        entities = plistlib.loads(attached.stdout)["system-entities"]
        device = next(entry["dev-entry"] for entry in entities if "dev-entry" in entry)
        if not any(Path(entry.get("mount-point", "/")) == mount.resolve() for entry in entities):
            raise AssertionError("private filesystem mounted at an unexpected path")
        output = mount / "output"
        output.touch()
        filler_bytes = exhaust(mount)
        status, reply = invoke_worker(worker, encoded, output)
        report = {
            "image_bytes": IMAGE_BYTES,
            "filler_bytes_before_enospc": filler_bytes,
            "reference_bytes": reference_bytes,
            "worker_exit": status,
            "reply": reply,
            "output_bytes": output.stat().st_size,
        }
        # The child has exited before any host read or mapping. Capture evidence
        # even on a buggy successful reply: dirty cache pages can still look right
        # while an explicit filesystem synchronization reports ENOSPC.
        if isinstance(reply, dict) and "BackedImage" in reply:
            try:
                report["pixels_match_after_child_exit"] = fingerprint(output) == reference_hash
            except OSError as error:
                report["read_errno"] = error.errno
            with output.open("r+b", buffering=0) as stream:
                try:
                    os.fsync(stream.fileno())
                    report["post_exit_fsync_errno"] = 0
                except OSError as error:
                    report["post_exit_fsync_errno"] = error.errno
        report["image_physical_bytes"] = image.stat().st_blocks * 512
        print(json.dumps(report, indent=2), flush=True)
        if report["image_physical_bytes"] > 32 * MIB:
            raise AssertionError("private image exceeded its physical storage cap")
        if isinstance(reply, dict) and "BackedImage" in reply:
            raise AssertionError("worker published successful pixels on an exhausted filesystem")
        if status == 0 and (not isinstance(reply, dict) or "Failure" not in reply):
            raise AssertionError("worker neither rejected output nor reported process failure")
    finally:
        # A timeout or malformed attach response is an unknown outcome. Recover
        # only our exact image's device identity before removing any paths.
        if attach_attempted and device is None:
            try:
                state = plistlib.loads(command(["/usr/bin/hdiutil", "info", "-plist"]).stdout)
                for entry in state.get("images", []):
                    if Path(entry.get("image-path", "/")).resolve() == image.resolve():
                        device = next(
                            entity["dev-entry"]
                            for entity in entry["system-entities"]
                            if "dev-entry" in entity
                        )
                        break
            except BaseException:
                print(f"Could not establish attachment cleanup; private image retained at {root}", file=sys.stderr)
                raise
        if device is None and mount.is_mount():
            raise RuntimeError(f"Private mount still present; preserve {root} for cleanup")
        if device is not None:
            try:
                command(["/usr/bin/hdiutil", "detach", device])
                detached = True
            except BaseException:
                print(f"Cleanup requires detaching {device}; private image retained at {root}", file=sys.stderr)
                raise
        if device is None or detached:
            shutil.rmtree(root)
            print("Private filesystem detached and all test files removed.", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", required=True, type=Path)
    parser.add_argument("--jpeg", required=True, type=Path,
                        help="opaque sRGB JPEG with decoded RGBA size at most 8 MiB")
    arguments = parser.parse_args()
    run(arguments.worker.resolve(strict=True), arguments.jpeg.resolve(strict=True))


if __name__ == "__main__":
    main()
