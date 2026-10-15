#!/usr/bin/env python3
"""Check that release sources include locked Git crates without local Git metadata."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('collect-maintained-sources.py')
spec = importlib.util.spec_from_file_location('maintained_sources', SCRIPT)
collector = importlib.util.module_from_spec(spec)
spec.loader.exec_module(collector)


class CargoGitSources(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        self.root = self.repo / '.tools/macos14-personal'
        self.sources = self.root / 'release-sources'
        self.sources.mkdir(parents=True)
        for name, value in [('REPO', self.repo), ('ROOT', self.root), ('SOURCES', self.sources)]:
            replacement = patch.object(collector, name, value)
            replacement.start()
            self.addCleanup(replacement.stop)
        self.checkout = self.root / 'cargo/git/checkouts/example-cache/locked'
        (self.checkout / 'src').mkdir(parents=True)
        (self.checkout / 'Cargo.toml').write_text('[package]\nname="example"\nversion="1.0.0"\n')
        (self.checkout / 'src/lib.rs').write_text('pub const VALUE: u8 = 7;\n')
        subprocess.run(['git', 'init', '--quiet', str(self.checkout)], check=True)
        subprocess.run(['git', '-C', str(self.checkout), 'add', '.'], check=True)
        subprocess.run(['git', '-C', str(self.checkout), '-c', 'user.name=Source Fixture',
                        '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet',
                        '-m', 'fixture'], check=True)
        self.revision = subprocess.check_output(
            ['git', '-C', str(self.checkout), 'rev-parse', 'HEAD'], text=True).strip()
        self.source = 'git+https://example.invalid/crate.git?rev=' + self.revision + '#' + self.revision
        for name in ('snow-crates', 'snow_rust_ffi'):
            directory = self.repo / name
            directory.mkdir()
            directory.joinpath('Cargo.lock').write_text(
                'version = 4\n[[package]]\nname="example"\nversion="1.0.0"\n'
                'source=' + json.dumps(self.source) + '\n')

    def test_shared_git_source_is_deduplicated_and_pinned(self):
        records = collector.cargo_git_sources()
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0]['revision'], self.revision)
        self.assertEqual(records[0]['packages'], ['example-1.0.0'])

    def test_missing_locked_checkout_stops_collection(self):
        path = self.repo / 'snow-crates/Cargo.lock'
        path.write_text(path.read_text().replace(self.revision, '0' * 40))
        with self.assertRaisesRegex(ValueError, 'Missing locked Cargo Git source'):
            collector.cargo_git_sources()

    def test_archive_contains_actual_git_sources_and_excludes_git_database(self):
        # Model a locally patched dependency: archive the same working tree used to build.
        (self.checkout / 'src/lib.rs').write_text('pub const VALUE: u8 = 9;\n')
        (self.sources / 'manifest.json').write_text(json.dumps(
            {'native': [], 'qt': {'QtVersion': '6.12.0'}}))
        for name in ('vcpkg-registry.tar.gz', 'vcpkg-local.patch'):
            (self.sources / name).write_bytes(b'fixture')
        (self.root / 'downloads').mkdir()
        (self.root / 'downloads/qt-everywhere-src-6.12.0.tar.xz').write_bytes(b'qt fixture')
        (self.root / 'cargo/registry/src').mkdir(parents=True)
        output = self.root / 'source.tar.gz'
        collector.archive(output)
        with tarfile.open(output) as archive:
            names = archive.getnames()
            self.assertFalse(any('.git' in Path(name).parts for name in names))
            source = next(name for name in names if name.startswith('cargo-git/')
                          and name.endswith('/src/lib.rs'))
            self.assertEqual(archive.extractfile(source).read(), b'pub const VALUE: u8 = 9;\n')
            provenance = json.load(archive.extractfile('cargo-git-sources.json'))
            self.assertEqual(provenance[0]['source'], self.source)
            self.assertEqual(provenance[0]['revision'], self.revision)


if __name__ == '__main__':
    unittest.main()
