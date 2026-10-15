#!/usr/bin/env python3
"""Focused checks for the removable personal build workflow."""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class PersonalWorkflow(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name) / 'repo'
        (self.repo / 'scripts').mkdir(parents=True)
        self.script = self.repo / 'scripts/macos14-personal.sh'
        shutil.copy2(ROOT / 'scripts/macos14-personal.sh', self.script)
        metadata = Path('snow_shot/packaging/macos/maintained-release.json')
        (self.repo / metadata).parent.mkdir(parents=True)
        shutil.copy2(ROOT / metadata, self.repo / metadata)
        self.personal = self.repo / '.tools/macos14-personal'
        self.personal.mkdir(parents=True)

    def command(self, *args, env=None):
        return subprocess.run(['bash', str(self.script), *args], capture_output=True,
                              text=True, env=env, timeout=30)

    def mark(self):
        (self.personal / '.personal-build-root').touch()

    def test_clean_keeps_artifacts_and_user_data(self):
        self.mark()
        for name in ('build', 'qt-build', 'rust-tests', 'tmp', 'artifacts', 'user-data', 'host'):
            (self.personal / name).mkdir()
            (self.personal / name / 'keep-or-remove').write_text(name)
        result = self.command('clean')
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ('build', 'qt-build', 'rust-tests', 'tmp'):
            self.assertFalse((self.personal / name).exists())
        for name in ('artifacts', 'user-data', 'host'):
            self.assertTrue((self.personal / name / 'keep-or-remove').is_file())
        self.assertEqual(self.command('clean', '--all').returncode, 0)
        self.assertFalse(self.personal.exists())

    def test_cleanup_refuses_unmarked_roots_and_symlinks(self):
        self.assertNotEqual(self.command('clean', '--all').returncode, 0)
        self.mark()
        outside = Path(self.temporary.name) / 'outside'
        outside.mkdir()
        (outside / 'valuable').write_text('preserve')
        (self.personal / 'build').symlink_to(outside, target_is_directory=True)
        self.assertNotEqual(self.command('clean').returncode, 0)
        self.assertTrue((outside / 'valuable').is_file())
        (self.personal / 'build').unlink()
        self.personal.rename(self.personal.with_name('linked-target'))
        self.personal.symlink_to(self.personal.with_name('linked-target'), target_is_directory=True)
        self.assertNotEqual(self.command('clean', '--all').returncode, 0)

    def test_configure_uses_isolated_dependencies_and_ad_hoc_signing(self):
        tools = self.repo / 'fake-tools'
        tools.mkdir()
        for name, body in {
            'uname': 'if [ "$1" = -s ]; then echo Darwin; else echo x86_64; fi',
            'xcode-select': 'echo /fake/Xcode',
            'cmake': 'printf "%s\\n" "$@" > "$SNOW_CONFIGURE_LOG"\n'
                     'printf "%s\\n" "$CARGO_HOME" "$RUSTUP_HOME" "$RUSTUP_TOOLCHAIN" "$TMPDIR" '
                     '> "$SNOW_ENVIRONMENT_LOG"',
        }.items():
            executable = tools / name
            executable.write_text('#!/bin/bash\n' + body + '\n')
            executable.chmod(0o755)
        log = self.repo / 'configure-arguments'
        environment_log = self.repo / 'child-environment'
        env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                   SNOW_CONFIGURE_LOG=str(log), SNOW_ENVIRONMENT_LOG=str(environment_log))
        result = self.command('configure', env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        args = log.read_text().splitlines()
        for expected in ('-DCMAKE_OSX_DEPLOYMENT_TARGET=14.4',
                         '-DSNOW_APPS_MACOS14_PERSONAL=ON', '-DSNOW_MACOS_CODESIGN_IDENTITY=-',
                         '-DSNOW_SHOT_ENABLE_MCP=OFF', '-DSNOW_APPS_BUILD_SNOW_SHOT_MINI=OFF',
                         '-DSNOW_APPS_BUILD_TESTS=OFF', '-DSNOW_APPS_QT_STATIC=ON'):
            self.assertIn(expected, args)
        tests_result = self.command('configure-tests', env=env)
        self.assertEqual(tests_result.returncode, 0, tests_result.stderr)
        test_args = log.read_text().splitlines()
        self.assertIn('-DSNOW_APPS_BUILD_TESTS=ON', test_args)
        self.assertIn('-DSNOW_SHOT_BUILD_TESTS=ON', test_args)
        self.assertIn('-DSNOW_APPS_BUILD_BENCHMARKS=OFF', test_args)
        self.assertIn(f'-DVCPKG_INSTALLED_DIR={self.personal}/installed', args)
        self.assertIn(f'-DSNOW_RUST_CARGO_TARGET_DIR={self.personal}/build/cargo', args)
        self.assertIn(f'-DSNOW_SHOT_QR_MODELS_CACHE={self.personal}/cache/qr-models-wechat', args)
        expected_environment = [str(self.personal / 'cargo'), str(self.personal / 'rustup'),
                                '1.97.1', str(self.personal / 'tmp')]
        self.assertEqual(environment_log.read_text().splitlines(), expected_environment)
        self.assertEqual(self.command('icon', env=env).returncode, 0)
        self.assertEqual(environment_log.read_text().splitlines(), expected_environment)

    def test_invalid_release_metadata_is_rejected_before_building(self):
        metadata = self.repo / 'snow_shot/packaging/macos/maintained-release.json'
        original = json.loads(metadata.read_text())
        for key, value in [('version', '../outside'), ('upstreamVersion', '../outside'),
                           ('version', '01.0.0'), ('buildNumber', 0), ('buildNumber', True)]:
            with self.subTest(key=key, value=value):
                metadata.write_text(json.dumps(dict(original, **{key: value})))
                result = self.command('configure')
                self.assertNotEqual(result.returncode, 0)
        metadata.write_text(json.dumps(original))


if __name__ == '__main__':
    unittest.main()
