"""Focused fixtures for the storage architecture boundary checker."""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / 'scripts/check_storage_boundary.cmake'
CMAKE = shutil.which('cmake')


@unittest.skipUnless(CMAKE, 'CMake is required')
class StorageBoundaryTests(unittest.TestCase):
    def check_sources(self, sources):
        with tempfile.TemporaryDirectory(prefix='snow-shot-storage-boundary-') as directory:
            root = Path(directory)
            for name, contents in sources.items():
                source = root / name
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text(contents, encoding='utf-8')
            return subprocess.run(
                [CMAKE, '-DSNOW_SHOT_SOURCE_DIR=' + root.as_posix(), '-P', str(CHECKER)],
                capture_output=True, text=True, encoding='utf-8', timeout=30)

    def test_storage_owns_persistence_file_names(self):
        result = self.check_sources({
            'src/storage/repository.cpp':
                'config.json __data_directory __mini_data_directory '
                'canvas_history.json display_0.png',
        })
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_storage_cannot_depend_on_presentation(self):
        for source in ('src/storage/repository.cpp', 'include/snow_shot/storage/repository.h'):
            with self.subTest(source=source):
                result = self.check_sources({
                    source: '#include "snow_shot/presentation/presenter.h"',
                })
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('storage includes presentation', result.stderr)

    def test_persistence_file_names_remain_private_to_storage(self):
        for name in ('config.json', '__data_directory', '__mini_data_directory',
                     'canvas_history.json', 'display_0.png'):
            with self.subTest(name=name):
                result = self.check_sources({'src/presentation/presenter.cpp': name})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("'" + name + "' appears", result.stderr)

    def test_edition_metadata_can_define_both_installation_markers(self):
        result = self.check_sources({
            'include/snow_shot/app/edition.h': '__data_directory __mini_data_directory',
        })
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_marker_exception_does_not_expose_settings_internals(self):
        for token in ('QSettings', 'config.json', 'canvas_history.json', 'display_0.png'):
            with self.subTest(token=token):
                result = self.check_sources({'include/snow_shot/app/edition.h': token})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("'" + token + "' appears", result.stderr)

    def test_other_app_headers_cannot_define_installation_markers(self):
        result = self.check_sources({'include/snow_shot/app/controller.h': '__data_directory'})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("'__data_directory' appears", result.stderr)

    def test_updater_keeps_its_installation_registry_and_marker_access(self):
        result = self.check_sources({
            'src/update/updatetransaction.cpp':
                'QSettings __data_directory __mini_data_directory',
        })
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
