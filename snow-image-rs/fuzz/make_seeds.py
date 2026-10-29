#!/usr/bin/env python3
"""Generate tiny original PNG/APNG/GIF/BMP and protocol seeds, without image libraries."""
import json
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parent / "corpus"

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

header = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 1, 8, 6, 0, 0, 0))
red = b"\0\xff\0\0\xff\x17\x2b\x3d\0"
green = b"\0\0\xff\0\xff\0\xff\0\xff"
png = header + chunk(b"IDAT", zlib.compress(red)) + chunk(b"IEND", b"")
def control(sequence):
    return chunk(b"fcTL", struct.pack(">IIIIIHHBB", sequence, 2, 1, 0, 0, 1, 100, 0, 0))
apng = header + chunk(b"acTL", struct.pack(">II", 2, 0)) + control(0)
apng += chunk(b"IDAT", zlib.compress(red)) + control(1)
apng += chunk(b"fdAT", struct.pack(">I", 2) + zlib.compress(green)) + chunk(b"IEND", b"")
gif = b"GIF89a\x01\0\x01\0\x80\0\0\xff\0\0\0\xff\0"
for compressed in [b"\x44\x01", b"\x4c\x01"]:
    gif += b"\x21\xf9\x04\0\x01\0\0\0\x2c\0\0\0\0\x01\0\x01\0\0\x02\x02" + compressed + b"\0"
gif += b"\x3b"
bmp = b"BM" + struct.pack("<IHHI", 58, 0, 0, 54)
bmp += struct.pack("<IiiHHIIiiII", 40, 1, 1, 1, 24, 0, 4, 0, 0, 0, 0) + b"\0\0\xff\0"
for target in ["inspect", "decode"]:
    directory = ROOT / target
    directory.mkdir(parents=True, exist_ok=True)
    for name, data in [("rgba.png", png), ("two.apng", apng), ("two.gif", gif), ("red.bmp", bmp)]:
        (directory / name).write_bytes(data)
(ROOT / "animation").mkdir(parents=True, exist_ok=True)
for name, data in [("two.apng", apng), ("two.gif", gif)]:
    (ROOT / "animation" / name).write_bytes(bytes([0, 0, 1, 0, 2, 3] + [0] * 10) + data)
(ROOT / "ipc").mkdir(parents=True, exist_ok=True)
for name, value in [("shutdown", "Shutdown"), ("cancel", {"Cancel": {"generation": 1, "id": 1}})]:
    payload = json.dumps(value).encode()
    (ROOT / "ipc" / name).write_bytes(struct.pack("<I", len(payload)) + payload)
(ROOT / "ipc" / "oversized").write_bytes(struct.pack("<I", 1024 * 1024 + 1))
shared = {
    "version": 1, "generation": 1, "id": 1,
    "reply": {"SharedImage": {
        "descriptor": {
            "width": 1, "height": 1, "row_stride": 4,
            "format": {"channels": "Rgba", "sample": "U8", "alpha": "Straight", "significant_bits": 8},
            "color": "Srgb", "orientation": "Normal",
        },
        "metadata": {"icc": 0, "exif": 0, "xmp": 0}, "bytes": 4,
    }},
}
payload = json.dumps(shared).encode()
(ROOT / "ipc" / "shared-image").write_bytes(struct.pack("<I", len(payload)) + payload)
policy = {
    "max_input_bytes": 1024 * 1024, "max_metadata_bytes": 64 * 1024,
    "max_animation_frames": 32, "working_bytes_per_job": 8 * 1024 * 1024,
    "total_memory_bytes": 8 * 1024 * 1024, "max_raster_bytes": 4 * 1024 * 1024,
    "max_prepared_bytes": 1024 * 1024, "preview_max_dimension": 2048,
    "max_artifact_bytes": 4 * 1024 * 1024, "max_temp_bytes": 16 * 1024 * 1024,
    "max_queued_jobs": 4, "max_codec_operations": 2, "max_codec_threads": 2,
    "worker_address_space_bytes": 1024 ** 4, "worker_memory_bytes": 256 * 1024 * 1024,
}
for storage in ["Pipe", "Backing", "InputHandle"]:
    command = {"Run": {
        "version": 1, "generation": 1, "id": 1, "policy": policy,
        "operation": {"Decode": {"apply_orientation": True, "preserve_metadata": True, "preferred_frame": None}},
        "input_len": len(png), "payload_storage": storage,
        "metadata": {"icc": 0, "exif": 0, "xmp": 0},
    }}
    payload = json.dumps(command).encode()
    (ROOT / "ipc" / f"decode-{storage.lower()}").write_bytes(struct.pack("<I", len(payload)) + payload)
