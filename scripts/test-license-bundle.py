"""Install the Windows notice document without building Qt or application binaries."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[1]
PROJECTS = ('snow_image', 'ant_design_qt', 'snow-crates',
            'snow_draw_engine_qt', 'snow_rust_ffi')


class LicenseBundle(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='snow license bundle ')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'sources/snow_shot'
        self.third_party = self.root / 'collected third-party'
        self.build = self.root / 'build'
        self.stage = self.root / 'installed notices'
        self.texts = {}
        for name in ('LICENSE', 'COPYRIGHT', 'THIRD_PARTY_NOTICES.md'):
            self.notice(self.source / name, name, f'Original project {name}\n')
        for project in PROJECTS:
            for name in ('LICENSE', 'COPYRIGHT'):
                label = f'components/{project.replace("_", "-")}/{name}'
                self.notice(self.source.parent / project / name, label,
                            f'Original {project} {name}\n')
        dependencies = {
            'cargo/example-1.2.3/LICENSE': 'Copyright © Example; literal ${value} and @value@\n',
            'cargo/example-1.2.3/NOTICE': 'Original attribution\nAll rights reserved.\n',
            'qt/qtbase/LICENSE': 'Original Qt terms\n',
            'source/font/OFL.txt': 'Original font notice\n',
            'vcpkg/native/copyright.txt': 'Original native terms\n',
            'project-notices/ant-design-icons/THIRD_PARTY_NOTICES.md': 'Original icons notice\n',
        }
        metadata = {'Notices': [{'Notice': name} for name in dependencies]}
        dependencies['manifest.json'] = json.dumps(metadata) + '\n'
        dependencies['INDEX.md'] = 'Original component and license mapping\n'
        for name, text in dependencies.items():
            self.notice(self.third_party / name, f'third-party/{name}', text)
        (self.root / 'CMakeLists.txt').write_text(f'''
cmake_minimum_required(VERSION 4.2)
project(LicenseBundle NONE)
include("{ROOT.as_posix()}/cmake/SnowShotLicenseBundle.cmake")
foreach(edition IN ITEMS SnowShot SnowShotMini)
    snow_install_license_bundle(
        SOURCE_DIRECTORY "{self.source.as_posix()}"
        THIRD_PARTY_DIRECTORY "{self.third_party.as_posix()}"
        DESTINATION "share/${{edition}}/licenses"
        COMPONENT "${{edition}}" PRODUCT "${{edition}}")
endforeach()
''', encoding='utf-8')
        self.run_cmake('-S', str(self.root), '-B', str(self.build))

    def notice(self, path, label, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(text.encode('utf-8'))
        self.texts[label] = text.encode('utf-8')

    def run_cmake(self, *arguments, succeeds=True):
        result = subprocess.run(['cmake', *arguments], capture_output=True,
                                text=True, encoding='utf-8', errors='replace')
        diagnostics = result.stdout + result.stderr
        if succeeds:
            self.assertEqual(result.returncode, 0, diagnostics)
        else:
            self.assertNotEqual(result.returncode, 0, diagnostics)
        return diagnostics

    def install(self, edition='SnowShot', succeeds=True):
        return self.run_cmake('--install', str(self.build), '--component', edition,
                              '--prefix', str(self.stage), succeeds=succeeds)

    def test_each_edition_installs_one_file_with_every_original_notice(self):
        for edition in ('SnowShot', 'SnowShotMini'):
            with self.subTest(edition=edition):
                self.install(edition)
                destination = self.stage / f'share/{edition}/licenses'
                self.assertEqual([p.name for p in destination.rglob('*') if p.is_file()],
                                 ['LICENSES.txt'])
                output = (destination / 'LICENSES.txt').read_bytes()
                self.assertTrue(output.startswith(f'{edition} - Licenses and Notices\n'.encode()))
                for label, original in self.texts.items():
                    marker = f'FILE: {label}\n'.encode()
                    self.assertEqual(output.count(marker), 1, label)
                    section = output.split(marker, 1)[1].split(b'\n\n', 1)[1]
                    self.assertTrue(section.startswith(original), label)
                manifest = (self.build / f'install_manifest_{edition}.txt').read_text()
                self.assertEqual(manifest.strip().replace('\\', '/'),
                                 (destination / 'LICENSES.txt').as_posix())
                self.install(edition)
                self.assertEqual((destination / 'LICENSES.txt').read_bytes(), output)

    def test_reinstall_refreshes_changed_and_retired_dependency_notices(self):
        self.install()
        retired = self.third_party / 'qt/qtbase/LICENSE'
        retired.unlink()
        manifest = self.third_party / 'manifest.json'
        metadata = json.loads(manifest.read_text())
        metadata['Notices'] = [entry for entry in metadata['Notices']
                               if entry['Notice'] != 'qt/qtbase/LICENSE']
        manifest.write_text(json.dumps(metadata), encoding='utf-8')
        (self.third_party / 'cargo/example-1.2.3/NOTICE').write_text(
            'Updated attribution\n', encoding='utf-8')
        self.install()
        output = (self.stage / 'share/SnowShot/licenses/LICENSES.txt').read_text(encoding='utf-8')
        self.assertIn('Updated attribution\n', output)
        self.assertNotIn('Original attribution\n', output)
        self.assertNotIn('FILE: third-party/qt/qtbase/LICENSE\n', output)

    def test_missing_collected_notice_blocks_installation(self):
        (self.third_party / 'cargo/example-1.2.3/NOTICE').unlink()
        diagnostics = self.install(succeeds=False)
        self.assertIn('Missing collected license notice', diagnostics)
        self.assertFalse(self.stage.exists())

    def test_missing_project_license_blocks_installation(self):
        (self.source / 'LICENSE').unlink()
        diagnostics = self.install(succeeds=False)
        self.assertIn('Missing license notice', diagnostics)
        self.assertFalse(self.stage.exists())

    def test_missing_collection_manifest_blocks_installation(self):
        (self.third_party / 'manifest.json').unlink()
        diagnostics = self.install(succeeds=False)
        self.assertIn('Missing collected license bundle entry', diagnostics)
        self.assertFalse(self.stage.exists())

    def test_cpack_archive_contains_only_the_selected_editions_document(self):
        configuration = self.root / 'CMakeLists.txt'
        with configuration.open('a', encoding='utf-8') as stream:
            stream.write('''
set(CPACK_PACKAGE_NAME snow-notices)
set(CPACK_PACKAGE_VERSION 1.0.0)
include(CPack)
''')
        self.run_cmake('-S', str(self.root), '-B', str(self.build))
        for edition in ('SnowShot', 'SnowShotMini'):
            with self.subTest(edition=edition):
                destination = self.root / f'archive-{edition}'
                result = subprocess.run(
                    ['cpack', '--config', str(self.build / 'CPackConfig.cmake'), '-G', 'ZIP',
                     '-B', str(destination), '-D',
                     f'CPACK_INSTALL_CMAKE_PROJECTS={self.build};LicenseBundle;{edition};/'],
                    capture_output=True, text=True, encoding='utf-8', errors='replace')
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                archives = list(destination.glob('*.zip'))
                self.assertEqual(len(archives), 1)
                with zipfile.ZipFile(archives[0]) as archive:
                    files = [name for name in archive.namelist() if not name.endswith('/')]
                    self.assertEqual(len(files), 1)
                    self.assertTrue(files[0].endswith(f'/share/{edition}/licenses/LICENSES.txt'))
                    self.assertIn(b'Original attribution\n', archive.read(files[0]))


if __name__ == '__main__':
    unittest.main()
