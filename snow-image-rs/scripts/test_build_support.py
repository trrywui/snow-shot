#!/usr/bin/env python3
"""Small offline regressions for native build configuration and package staging."""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


native = load("snow_native_build", ROOT / "native/build.py")
package = load("snow_package", ROOT / "scripts/package.py")
licenses = load("snow_licenses", ROOT / "scripts/licenses.py")


class NativeConfiguration(unittest.TestCase):
    def test_release_host_tools_preserve_loadable_proc_macros(self):
        # Python 3.9 is supported, so inspect this small contract without tomllib.
        manifest = (ROOT / "Cargo.toml").read_text()
        section = manifest.split("[profile.release.build-override]\n", 1)[1].split("\n[", 1)[0]
        self.assertIn('strip = "none"', section.splitlines())
        release = manifest.split("[profile.release]\n", 1)[1].split("\n[", 1)[0]
        self.assertNotIn('strip = "none"', release.splitlines())
        self.assertIn('lto = "thin"', release.splitlines())

    def test_windows_jpeg_and_dav1d_use_matching_dynamic_crt(self):
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            commands = []
            with mock.patch.object(native, "CACHE", cache), \
                    mock.patch.object(native, "host_target", return_value="aarch64-pc-windows-msvc"), \
                    mock.patch.object(native.platform, "system", return_value="Windows"), \
                    mock.patch.object(native, "checked_archive", side_effect=lambda name, fetch: cache / name), \
                    mock.patch.object(native, "extract", side_effect=lambda name, archive: cache / "sources" / name), \
                    mock.patch.object(native, "run", side_effect=lambda command, env: commands.append(list(map(str, command)))), \
                    contextlib.redirect_stdout(io.StringIO()):
                native.build("aarch64-pc-windows-msvc", 2)
            jpeg = next(command for command in commands if command[:2] == ["cmake", "-S"] and Path(command[2]).name == "jpeg")
            self.assertIn("-DWITH_CRT_DLL=ON", jpeg)
            self.assertIn("-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL", jpeg)
            self.assertIn("-DCMAKE_C_COMPILER_TARGET=aarch64-pc-windows-msvc", jpeg)
            dav1d = next(command for command in commands if "setup" in command)
            self.assertIn("-Db_vscrt=md", dav1d)


class PackageStaging(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        (self.root / "native").mkdir()
        (self.root / "native/sources.json").write_text('{"schema": 1, "sources": {}}\n')
        for name in ("LICENSE", "THIRD_PARTY_NOTICES.md", "Cargo.lock"):
            (self.root / name).write_text(f"fixture {name}\n")
        self.commands = []

    def runner(self, command, **kwargs):
        self.assertEqual(kwargs, {"cwd": self.root, "check": True})
        self.commands.append(command)
        if command[0] == "cargo":
            target = command[command.index("--target") + 1]
            target_dir = Path(command[command.index("--target-dir") + 1])
            self.assertTrue(target_dir.is_absolute())
            self.assertEqual(target_dir, self.root / "target/cargo")
            name = "snow-image-worker.exe" if "windows" in target else "snow-image-worker"
            artifact = target_dir / target / "release" / name
            artifact.parent.mkdir(parents=True, exist_ok=True)
            artifact.write_bytes(b"fresh validated target worker")
        else:
            notice_dir = Path(command[command.index("--output") + 1])
            notice_dir.mkdir()
            (notice_dir / "INDEX.md").write_text("fixture collected licenses")

    def invoke(self, host, arguments=None, runner=None):
        with mock.patch.object(package, "ROOT", self.root), \
                mock.patch.object(package.subprocess, "check_output", return_value=f"rustc fixture\nhost: {host}\n") as compiler, \
                mock.patch.object(package.subprocess, "run", side_effect=runner or self.runner), \
                mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": "wrong-directory", "CARGO_BUILD_TARGET": "wrong-target"}), \
                contextlib.redirect_stdout(io.StringIO()):
            package.main(arguments or [])
        compiler.assert_called_once_with(["rustc", "-vV"], cwd=self.root, text=True)

    def test_explicit_target_and_output_prevent_stale_binary_selection(self):
        for target in package.SUPPORTED_TARGETS:
            with self.subTest(target=target):
                name = "snow-image-worker.exe" if "windows" in target else "snow-image-worker"
                stale = self.root / "target/cargo/release" / name
                stale.parent.mkdir(parents=True, exist_ok=True)
                stale.write_bytes(b"stale worker from a different configuration")
                destination = self.root / "target/dist" / target
                destination.mkdir(parents=True)
                (destination / "obsolete-file").write_text("previous package")
                self.invoke(target)
                self.assertEqual((destination / name).read_bytes(), b"fresh validated target worker")
                self.assertFalse((destination / "obsolete-file").exists())
                manifest = json.loads((destination / "manifest.json").read_text())
                self.assertEqual(manifest["target"], target)
                self.assertEqual(manifest["worker_sha256"], hashlib.sha256((destination / name).read_bytes()).hexdigest())
                self.assertEqual(manifest["cargo_lock_sha256"], hashlib.sha256((destination / "Cargo.lock").read_bytes()).hexdigest())
                self.assertEqual(manifest["native_sources_sha256"], hashlib.sha256((destination / "native-sources.json").read_bytes()).hexdigest())
                self.assertTrue((destination / "third-party/INDEX.md").is_file())
                self.assertFalse(list(destination.parent.glob(f".{target}.*")))

    def test_unsupported_or_cross_target_fails_before_build(self):
        with self.assertRaisesRegex(RuntimeError, "Unsupported package target"):
            self.invoke("x86_64-unknown-linux-gnu")
        with self.assertRaisesRegex(RuntimeError, "Package on the native target"):
            self.invoke("aarch64-apple-darwin", ["--target", "x86_64-apple-darwin"])
        self.assertEqual(self.commands, [])

    def test_notice_failure_preserves_the_completed_package(self):
        target = "aarch64-apple-darwin"
        destination = self.root / "target/dist" / target
        destination.mkdir(parents=True)
        (destination / "manifest.json").write_bytes(b"previous completed manifest")
        (destination / "snow-image-worker").write_bytes(b"previous completed worker")

        def fail_notices(command, **kwargs):
            if command[0] == "cargo":
                return self.runner(command, **kwargs)
            raise subprocess.CalledProcessError(1, command)

        with self.assertRaises(subprocess.CalledProcessError):
            self.invoke(target, runner=fail_notices)
        self.assertEqual((destination / "manifest.json").read_bytes(), b"previous completed manifest")
        self.assertEqual((destination / "snow-image-worker").read_bytes(), b"previous completed worker")
        self.assertFalse(list(destination.parent.glob(f".{target}.*")))

    def test_failed_publish_rename_restores_previous_directory(self):
        staging = self.root / "staging"
        destination = self.root / "completed"
        staging.mkdir()
        destination.mkdir()
        (staging / "worker").write_bytes(b"new")
        (destination / "worker").write_bytes(b"old")
        replace = Path.replace

        def fail_new_directory(path, target):
            if path == staging:
                raise OSError("injected publication rename failure")
            return replace(path, target)

        with mock.patch.object(Path, "replace", fail_new_directory):
            with self.assertRaisesRegex(OSError, "injected publication rename failure"):
                package.publish(staging, destination)
        self.assertEqual((destination / "worker").read_bytes(), b"old")
        self.assertEqual((staging / "worker").read_bytes(), b"new")
        self.assertFalse(list(self.root.glob(".completed.previous-*")))


class ComponentNotices(unittest.TestCase):
    def test_native_component_basenames_remain_distinct(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "native").mkdir()
            manifest = {"sources": {"aom": {"version": "fixture"}}}
            (root / "native/sources.json").write_text(json.dumps(manifest))
            source = root / "target/native/sources/aom"
            paths = ["LICENSE", "PATENTS", *licenses.NATIVE_COMPONENT_NOTICES["aom"]]
            for relative in paths:
                path = source / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f"unique notice: {relative}")
            destination = root / "notices"

            def cargo(*args):
                return '{"packages": []}' if args[0] == "metadata" else ""

            with mock.patch.object(licenses, "ROOT", root), \
                    mock.patch.object(licenses, "cargo", side_effect=cargo), \
                    contextlib.redirect_stdout(io.StringIO()):
                licenses.collect(destination)
            records = json.loads((destination / "manifest.json").read_text())
            self.assertEqual(len(records), len(paths))
            self.assertEqual(len({record["file"] for record in records}), len(paths))
            for relative in paths:
                self.assertEqual((destination / "native/aom-fixture" / relative).read_text(), f"unique notice: {relative}")
            compatibility = (root / "target/license-prefix/share/aom/copyright").read_text()
            self.assertTrue(all(f"unique notice: {relative}" in compatibility for relative in paths))

    def test_missing_required_component_notice_is_an_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            (source / "LICENSE").write_text("top-level license is insufficient")
            with self.assertRaisesRegex(RuntimeError, "Missing required aom component notice"):
                licenses.native_notices("aom", source)


if __name__ == "__main__":
    unittest.main()
