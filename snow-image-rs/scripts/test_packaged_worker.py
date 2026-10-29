#!/usr/bin/env python3
"""Opt-in native package smoke test; builds the client example, never the package."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import zlib


ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT = 64, 47
SIGNATURE = b"\x89PNG\r\n\x1a\n"
EXPECTED = bytes(value for y in range(HEIGHT) for x in range(WIDTH)
                 for value in ((x * 7 + y * 3) % 256, (x * 11 + y * 17) % 256,
                               (x * 13 + y * 5) % 256, 255))
SUPPORTED_TARGETS = {
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "aarch64-pc-windows-msvc", "x86_64-pc-windows-msvc",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def verify_package(package, host):
    manifest = json.loads((package / "manifest.json").read_text(encoding="utf-8"))
    require(manifest.get("schema") == 1 and manifest.get("ipc_version") == 1,
            "Package manifest schema or IPC version is unsupported")
    require(manifest.get("target") == host,
            f"Package target {manifest.get('target')!r} differs from native host {host!r}")
    worker = package / ("snow-image-worker.exe" if "windows" in host else "snow-image-worker")
    for path, key in ((worker, "worker_sha256"),
                      (package / "Cargo.lock", "cargo_lock_sha256"),
                      (package / "native-sources.json", "native_sources_sha256")):
        require(digest(path) == manifest.get(key), f"Package hash mismatch: {path.name}")
    for name in ("LICENSE", "THIRD_PARTY_NOTICES.md"):
        require((package / name).is_file(), f"Package is missing {name}")
    notice_root = (package / "third-party").resolve()
    notices = json.loads((notice_root / "manifest.json").read_text(encoding="utf-8"))
    require(isinstance(notices, list) and notices, "Package notice manifest is empty or invalid")
    paths = set()
    for item in notices:
        relative = Path(item["file"])
        path = (notice_root / relative).resolve()
        require(not relative.is_absolute() and notice_root in path.parents,
                f"Notice path escapes the package: {relative}")
        require(path not in paths and path.is_file(), f"Missing or duplicate notice: {relative}")
        paths.add(path)
    return worker, len(paths)


def chunk(kind, payload):
    return (struct.pack(">I", len(payload)) + kind + payload
            + struct.pack(">I", zlib.crc32(kind + payload)))


def write_fixture(path):
    stride = WIDTH * 4
    rows = b"".join(b"\0" + EXPECTED[y * stride:(y + 1) * stride] for y in range(HEIGHT))
    path.write_bytes(SIGNATURE
                     + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0))
                     + chunk(b"sRGB", b"\0") + chunk(b"IDAT", zlib.compress(rows))
                     + chunk(b"IEND", b""))


def verify_png(path):
    require(path.stat().st_size <= 1024 * 1024, "Fixture output unexpectedly exceeds 1 MiB")
    data = path.read_bytes()
    require(data.startswith(SIGNATURE), "Export is not PNG")
    position, compressed, header, ended = 8, bytearray(), None, False
    while position < len(data):
        require(position + 12 <= len(data), "Truncated PNG chunk header")
        size = struct.unpack_from(">I", data, position)[0]
        end = position + size + 12
        require(end <= len(data), "Truncated PNG chunk payload")
        kind = data[position + 4:position + 8]
        payload = data[position + 8:end - 4]
        require(zlib.crc32(kind + payload) == struct.unpack_from(">I", data, end - 4)[0],
                f"PNG CRC mismatch for {kind!r}")
        if kind == b"IHDR":
            require(position == 8 and size == 13 and header is None, "Invalid PNG IHDR")
            header = struct.unpack(">IIBBBBB", payload)
        elif kind == b"IDAT":
            require(header is not None, "PNG data precedes its header")
            compressed.extend(payload)
        elif kind == b"IEND":
            require(size == 0 and end == len(data), "Invalid PNG end or trailing bytes")
            ended = True
        position = end
    require(ended and header == (WIDTH, HEIGHT, 8, 6, 0, 0, 0),
            "PNG dimensions, format, or end marker changed")
    stride = WIDTH * 4
    expected_length = HEIGHT * (stride + 1)
    decoder = zlib.decompressobj()
    raw = decoder.decompress(compressed, expected_length + 1)
    require(len(raw) == expected_length and decoder.eof
            and not decoder.unused_data and not decoder.unconsumed_tail,
            "PNG decompressed length or stream boundary is invalid")
    decoded, previous = bytearray(), bytes(stride)
    for y in range(HEIGHT):
        kind = raw[y * (stride + 1)]
        row = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        require(kind in range(5), f"Invalid PNG row filter: {kind}")
        for i in range(stride):
            left = row[i - 4] if i >= 4 else 0
            above = previous[i]
            corner = previous[i - 4] if i >= 4 else 0
            if kind == 1:
                predictor = left
            elif kind == 2:
                predictor = above
            elif kind == 3:
                predictor = (left + above) // 2
            elif kind == 4:
                prediction = left + above - corner
                distances = (abs(prediction - left), abs(prediction - above), abs(prediction - corner))
                predictor = (left, above, corner)[distances.index(min(distances))]
            else:
                predictor = 0
            row[i] = (row[i] + predictor) % 256
        decoded.extend(row)
        previous = row
    require(decoded == EXPECTED, "ScreenshotSdr export changed fixture RGBA samples")
    return len(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, help="Package directory; defaults to target/dist/<native-host>")
    args = parser.parse_args()
    compiler = subprocess.run(["rustc", "-vV"], cwd=ROOT, check=True, capture_output=True,
                              encoding="utf-8", timeout=15)
    host = next(line.split(": ", 1)[1] for line in compiler.stdout.splitlines() if line.startswith("host:"))
    require(host in SUPPORTED_TARGETS, f"Unsupported native package host: {host}")
    package = (args.package or ROOT / "target/dist" / host).resolve()
    worker, notice_count = verify_package(package, host)
    target_dir = (ROOT / "target/cargo").resolve()
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "snow-image-rs",
                    "--example", "export", "--target", host, "--target-dir", str(target_dir)],
                   cwd=ROOT, check=True, timeout=600)
    example = target_dir / host / "release/examples" / ("export.exe" if "windows" in host else "export")
    with tempfile.TemporaryDirectory(prefix="snow-image-package-smoke-") as temporary:
        source, output = Path(temporary) / "input.png", Path(temporary) / "output.png"
        write_fixture(source)
        result = subprocess.run([str(example), str(worker), str(source), str(output)],
                                cwd=ROOT, check=True, capture_output=True, encoding="utf-8", timeout=60)
        size = verify_png(output)
        receipt = f"{WIDTH}×{HEIGHT}, 8 bits, {size} bytes, Exact"
        require(result.stdout.strip() == receipt, f"Unexpected export receipt: {result.stdout!r}")
    print(f"Verified {package}: three hashes, {notice_count} notice files, PNG CRCs, "
          f"all {len(EXPECTED)} RGBA samples, and Exact receipt ({size} bytes)")


if __name__ == "__main__":
    main()
