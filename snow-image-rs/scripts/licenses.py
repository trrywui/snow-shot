#!/usr/bin/env python3
"""Collect actual resolved source notices; never substitute a generic MIT text."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
LICENSE = re.compile(r"^(?:LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE|UNLICENSE|PATENTS|AUTHORS)(?:[._-].*)?$", re.I)
# These bundled components participate in AOM's encoder and native x64 assembly.
# Keep their source-relative paths: all three files are named LICENSE.
NATIVE_COMPONENT_NOTICES = {
    "aom": (
        "third_party/fastfeat/LICENSE",
        "third_party/vector/LICENSE",
        "third_party/x86inc/LICENSE",
    ),
}


def native_notices(name, directory):
    if not directory.is_dir():
        raise RuntimeError(f"Prepare verified native sources first: {directory}")
    notices = sorted(path for path in directory.iterdir() if path.is_file() and LICENSE.match(path.name))
    required = list(NATIVE_COMPONENT_NOTICES.get(name, ()))
    if name == "jpeg":
        required += ["README.ijg", "LICENSE.md"]
    for relative in required:
        path = directory / relative
        if not path.is_file():
            raise RuntimeError(f"Missing required {name} component notice: {path}")
        if path not in notices:
            notices.append(path)
    return notices


def cargo(*args):
    return subprocess.check_output(["cargo", *args], cwd=ROOT, text=True)


def collect(destination):
    destination.mkdir(parents=True, exist_ok=True)
    packages = {}
    selected = set()
    for target in ("aarch64-apple-darwin", "x86_64-apple-darwin", "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc"):
        metadata = json.loads(cargo("metadata", "--locked", "--offline", "--filter-platform", target, "--format-version", "1"))
        packages.update({package["id"]: package for package in metadata["packages"]})
        tree = cargo("tree", "--locked", "--offline", "-p", "snow-image-worker", "-p", "snow-image-rs", "--target", target,
                     "--edges", "normal,build", "--prefix", "none", "--format", "{p}")
        selected.update(tuple(line.split()[:2]) for line in tree.splitlines())
    records = []

    def copy(category, package, declared, sources, source_root=None):
        if not sources:
            raise RuntimeError(f"No distributable notice for {package}")
        directory = destination / category / package
        directory.mkdir(parents=True, exist_ok=True)
        for source in sources:
            relative = source.relative_to(source_root) if source_root else Path(source.name)
            target = directory / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            records.append(dict(category=category, package=package, license=declared, file=target.relative_to(destination).as_posix()))

    for package in packages.values():
        if not package["source"] or (package["name"], "v" + package["version"]) not in selected:
            continue
        directory = Path(package["manifest_path"]).parent
        notices = sorted(path for path in directory.iterdir() if path.is_file() and LICENSE.match(path.name))
        if package.get("license_file"):
            path = directory / package["license_file"]
            if path.is_file() and path not in notices:
                notices.append(path)
        if not notices:
            fallback = ROOT.parent / "licenses/cargo" / f"{package['name']}-{package['version']}"
            if fallback.is_dir():
                notices = sorted(path for path in fallback.iterdir() if path.is_file())
        copy("rust", f"{package['name']}-{package['version']}", package["license"], notices)
    manifest = json.loads((ROOT / "native/sources.json").read_text())
    compatibility = ROOT / "target/license-prefix/share"
    for name, source in manifest["sources"].items():
        directory = ROOT / "target/native/sources" / name
        notices = native_notices(name, directory)
        copy("native", f"{name}-{source['version']}", "See upstream source notices", notices, directory)
        compat = compatibility / name
        compat.mkdir(parents=True, exist_ok=True)
        (compat / "copyright").write_text("\n\n".join(path.read_text(errors="replace") for path in notices))
    records.sort(key=lambda record: (record["category"], record["package"], record["file"]))
    (destination / "manifest.json").write_text(json.dumps(records, indent=2) + "\n")
    (destination / "INDEX.md").write_text("# Snow Image Rust dependency notices\n\n" +
        "These are the resolved runtime/build dependencies for all four supported targets.\n\n" +
        "| Source | Package | Declared license | Text |\n| --- | --- | --- | --- |\n" +
        "".join(f"| {r['category']} | {r['package']} | {r['license']} | [{Path(r['file']).name}]({r['file']}) |\n" for r in records))
    print(f"Collected {len(records)} notices in {destination}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "target/third-party")
    parser.add_argument("--repository-check", action="store_true", help="Also run the repository PowerShell collector against a prepared notice prefix")
    args = parser.parse_args()
    collect(args.output)
    if args.repository_check:
        powershell = shutil.which("pwsh")
        if not powershell:
            candidates = sorted((ROOT.parent / ".tools/powershell").glob("*/pwsh"))
            powershell = str(candidates[-1]) if candidates else None
        if not powershell:
            raise RuntimeError("PowerShell is needed for --repository-check")
        subprocess.run([powershell, "-NoProfile", "-File", str(ROOT.parent / "scripts/collect-third-party-licenses.ps1"),
                        "-Destination", str(ROOT / "target/repository-license-check/third-party"),
                        "-AllowedRoot", str(ROOT / "target"), "-VcpkgPrefix", str(ROOT / "target/license-prefix"),
                        "-StandaloneMedia", "-CargoManifest", str(ROOT / "crates/snow-image-worker/Cargo.toml"),
                        "-AntDesignNotice", str(ROOT.parent / "ant_design_qt/THIRD_PARTY_NOTICES.md"),
                        "-FallbackLicenseDirectory", str(ROOT.parent / "licenses"),
                        "-CargoTarget", "aarch64-apple-darwin"], cwd=ROOT, check=True)


if __name__ == "__main__":
    main()
