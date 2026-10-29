#!/usr/bin/env python3
"""Compare the matched legacy/new-engine reports without excluding regressions."""
import argparse
import json
import math
from pathlib import Path


def platform_key(value):
    architecture, system = value.rsplit("-", 1)
    return (
        {"arm64": "aarch64", "AMD64": "x86_64"}.get(architecture, architecture),
        {"Darwin": "macos", "Windows": "windows"}.get(system, system),
    )


def compare(legacy, candidate):
    if platform_key(legacy["platform"]) != platform_key(candidate["platform"]):
        raise ValueError("Native platforms differ")
    if not legacy["settings"].startswith("v2;") or not candidate["settings"].startswith("v4;"):
        raise ValueError("This comparison requires legacy v2 and engine v4 matched settings")
    if min(legacy["iterations"], candidate["iterations"]) < 5:
        raise ValueError("Use at least five measured iterations after warmup")
    old = {record["name"]: record for record in legacy["records"]}
    new = {record["name"]: record for record in candidate["records"]}
    required = [
        f"{operation}-{size}"
        for size in ("1920x1080", "3840x2160", "7680x4320")
        for operation in ("png", "png-alpha", "jpeg", "preview")
    ]
    report = {"schema": 1, "threshold_percent": 10, "records": [], "latency_gate_passed": True}
    for name in required + ["tall-png-256x131072", "four-animations-32-frames"]:
        before, after = old[name], new[name]
        if (before["width"], before["height"]) != (after["width"], after["height"]):
            raise ValueError(f"{name}: input dimensions differ")
        if after["post_job_accounted_bytes"] or after["post_job_disk_bytes"]:
            raise ValueError(f"{name}: retained job reservations")
        if name.startswith(("png", "tall-png")):
            if before["psnr_db"] is not None or after["psnr_db"] is not None:
                raise ValueError(f"{name}: lossless sample verification failed")
        if name.startswith("jpeg"):
            if before["output_bytes"] != after["output_bytes"] or not math.isclose(
                before["psnr_db"], after["psnr_db"], abs_tol=0.0001
            ):
                raise ValueError(f"{name}: JPEG size/quality no longer matches")
        if name.startswith("preview"):
            if before["output_bytes"] != after["output_bytes"] or not after["preview_reference"]:
                raise ValueError(f"{name}: preview dimensions/quality comparison missing")
        change = (after["median_ms"] / before["median_ms"] - 1) * 100
        passes = change <= 10
        included = name in required
        report["records"].append({
            "name": name,
            "legacy_median_ms": before["median_ms"],
            "engine_median_ms": after["median_ms"],
            "change_percent": change,
            "legacy_bytes": before["output_bytes"],
            "engine_bytes": after["output_bytes"],
            "common_path_gate": included,
            "latency_passed": passes,
            "preview_reference": after.get("preview_reference"),
        })
        if included and not passes:
            report["latency_gate_passed"] = False
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("legacy", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = compare(json.loads(args.legacy.read_text()), json.loads(args.candidate.read_text()))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    for record in report["records"]:
        print(f'{record["name"]}: {record["change_percent"]:+.1f}%'
              + (" (common path)" if record["common_path_gate"] else " (additional workload)"))
    if not report["latency_gate_passed"]:
        raise SystemExit("Common-path median regression exceeds 10%; report retained")


if __name__ == "__main__":
    main()
