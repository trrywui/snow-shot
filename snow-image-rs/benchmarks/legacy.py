#!/usr/bin/env python3
"""Build/run a standalone legacy benchmark using its existing Release Ninja flags."""
import argparse
import datetime
import json
import pathlib
import platform
import shlex
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]


def build(directory):
    directory = directory.resolve()
    commands = subprocess.check_output(
        ["ninja", "-C", str(directory), "-t", "commands", "snow_image/tools/snow_image_benchmark"],
        text=True,
    ).splitlines()
    compile_command = next(line for line in commands if " -c " in line and line.endswith("/codec_benchmark.cpp"))
    link_command = next(line for line in commands if " -o snow_image/tools/snow_image_benchmark " in line)
    destination = ROOT / "target/legacy"
    destination.mkdir(parents=True, exist_ok=True)
    obj = destination / "legacy.o"
    executable = destination / "legacy"
    args = shlex.split(compile_command)
    args[args.index("-c") + 1] = str(ROOT / "benchmarks/legacy.cpp")
    for flag in ("-o", "-MT", "-MF"):
        args[args.index(flag) + 1] = str(obj if flag != "-MF" else obj.with_suffix(".d"))
    subprocess.run(args, cwd=directory, check=True)
    # Ninja's link rule has ': && compiler ... && :' wrappers. Parse tokens;
    # no project command is executed through a shell.
    args = shlex.split(link_command)
    args = args[args.index("&&") + 1 :]
    args = args[: args.index("&&")]
    args[args.index("-o") + 1] = str(executable)
    args = [str(obj) if argument.endswith("/codec_benchmark.cpp.o") else argument for argument in args]
    subprocess.run(args, cwd=directory, check=True)
    return executable


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=pathlib.Path, default=ROOT.parent / "build/snow-shot-macos-arm64-performance")
    parser.add_argument("--corpus", type=pathlib.Path, default=ROOT / "target/corpus")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "benchmarks/legacy-macos-arm64-balanced.json")
    parser.add_argument("--iterations", type=int, default=5)
    parser.add_argument("--size", choices=("all", "1080", "4k", "8k", "extended"), default="all")
    arguments = parser.parse_args()
    executable = build(arguments.build)
    report = {
        "schema": 1,
        "platform": f"{platform.machine()}-{platform.system()}",
        "recorded_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "iterations": arguments.iterations,
        "settings": "v2; generated-ui v1; retained RGBA8 source; native RasterSource encode to file; PNG6 adaptive RGBA opaque+alpha+tall; JPEG95/444/sequential; exact fit2048 n/8 reduced JPEG decode then linear premultiplied Lanczos3 max8threads; reduced-only preview separately; tall256x131072 streams a precomputed1800-row tile; four shared-input GIF sessions reset then32 advances+owned snapshots; one warmup; PSNR includes RGBA; no worker isolation",
        "build_directory": str(arguments.build.resolve()),
        "records": [],
    }
    process = subprocess.Popen(
        [str(executable), str(arguments.corpus.resolve()), str(ROOT / "target/legacy"), str(arguments.iterations), arguments.size],
        stdout=subprocess.PIPE,
        text=True,
    )
    for line in process.stdout:
        record = json.loads(line)
        report["records"].append(record)
        print(f'{record["name"]}: {record["median_ms"]:.3f} ms; {record["output_bytes"]} bytes', flush=True)
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(json.dumps(report, indent=2) + "\n")
    if process.wait() != 0:
        raise SystemExit("Legacy benchmark failed; partial report retained")


if __name__ == "__main__":
    main()
