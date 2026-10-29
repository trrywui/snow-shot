"""Exercise workspace test selection using a small CTest project without Qt."""

import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
CMAKE = shutil.which('cmake')
CTEST = shutil.which('ctest')


@unittest.skipUnless(CMAKE and CTEST, 'CMake and CTest are required')
class TestRunnerPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        directory = tempfile.TemporaryDirectory(prefix='snow test runner ')
        cls.addClassCleanup(directory.cleanup)
        cls.source = Path(directory.name)
        cls.build = cls.source / 'build'
        workspace = (ROOT / 'CMakeLists.txt').read_text(encoding='utf-8')
        target = re.search(r'if\(NOT TARGET snow-tests\)\n.*?\nendif\(\)', workspace, re.S)
        if target is None:
            raise AssertionError('workspace must define the snow-tests runner')
        exclusion = re.search(r'set\(_snow_deterministic_test_exclude "([^"]+)"\)',
                              target.group())
        if exclusion is None:
            raise AssertionError('snow-tests must explicitly exclude non-default labels')
        cls.exclusion = exclusion.group(1)
        (cls.source / 'CMakeLists.txt').write_text('''
cmake_minimum_required(VERSION 3.21)
project(SnowTestRunnerPolicy NONE)
enable_testing()
add_custom_target(snow-all)
add_test(NAME portable-unit COMMAND "${CMAKE_COMMAND}" -E true)
set_tests_properties(portable-unit PROPERTIES LABELS "unit")
add_test(NAME windows-unit COMMAND "${CMAKE_COMMAND}" -E true)
set_tests_properties(windows-unit PROPERTIES LABELS "unit;windows")
add_test(NAME native-interactive COMMAND "${CMAKE_COMMAND}" -E false)
set_tests_properties(native-interactive PROPERTIES LABELS "unit;interactive")
add_test(NAME native-e2e COMMAND "${CMAKE_COMMAND}" -E false)
set_tests_properties(native-e2e PROPERTIES LABELS "e2e")
add_test(NAME measurement COMMAND "${CMAKE_COMMAND}" -E false)
set_tests_properties(measurement PROPERTIES LABELS "benchmark")
''' + target.group() + '\n', encoding='utf-8')
        configured = cls.run_command(CMAKE, '-S', str(cls.source), '-B', str(cls.build),
                                     '-DCMAKE_BUILD_TYPE=Debug')
        if configured.returncode != 0:
            raise AssertionError(configured.stdout + configured.stderr)

    @staticmethod
    def run_command(*arguments):
        return subprocess.run(arguments, capture_output=True, text=True,
                              encoding='utf-8', errors='replace', timeout=30)

    def selected_tests(self, exclusion):
        result = self.run_command(CTEST, '--test-dir', str(self.build), '-C', 'Debug',
                                  '--show-only=json-v1', '-LE', exclusion)
        self.assertEqual(result.returncode, 0, result.stderr)
        return {test['name'] for test in json.loads(result.stdout)['tests']}

    def test_runner_filter_is_valid_without_a_platform_shell(self):
        self.assertEqual(self.selected_tests(self.exclusion), {'portable-unit', 'windows-unit'})

    def test_generated_runner_excludes_native_and_benchmark_fixtures(self):
        result = self.run_command(CMAKE, '--build', str(self.build), '--config', 'Debug',
                                  '--target', 'snow-tests')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('100% tests passed', result.stdout)

    def test_default_presets_keep_the_same_noninteractive_scope(self):
        presets = json.loads((ROOT / 'CMakePresets.json').read_text(encoding='utf-8'))
        for preset in presets['testPresets']:
            exclusion = preset.get('filter', {}).get('exclude', {}).get('label')
            if exclusion is None:
                continue
            with self.subTest(preset=preset['name']):
                expected = {'portable-unit'}
                if 'macos' not in preset['name']:
                    expected.add('windows-unit')
                self.assertEqual(self.selected_tests(exclusion), expected)


@unittest.skipUnless(CMAKE and CTEST, 'CMake and CTest are required')
class ClipboardResourcePolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        directory = tempfile.TemporaryDirectory(prefix='snow clipboard resources ')
        cls.addClassCleanup(directory.cleanup)
        cls.source = Path(directory.name)
        workspace = (ROOT / 'snow_shot/CMakeLists.txt').read_text(encoding='utf-8')
        policy = re.search(r'if\(SNOW_SHOT_BUILD_TESTS\)\n\s*'
                           r'include\(SnowShotTestResources\)\nendif\(\)', workspace)
        if policy is None:
            raise AssertionError('Snow Shot must enable its shared test resource policy')
        # Derive required aliases from their real executable binding, independently of
        # the name families used by the policy. A new oddly named alias must be covered.
        hosts = ('clipboard-roundtrip', 'export-coordinator', 'pinned-window',
                 'screenshot-history-page', 'clipboard-placement')
        executable = r'snow-shot-(?:' + '|'.join(hosts) + r')-tests'
        registrations = list(re.finditer(
            r'\badd_test\(\s*NAME\s+(\S+)\s+COMMAND\s+(?:"?\$<TARGET_FILE:)?'
            + '(' + executable + r')(?:>"?)?(?=\s|\))', workspace))
        if {match.group(2) for match in registrations} != {
                f'snow-shot-{host}-tests' for host in hosts}:
            raise AssertionError('the fixture must exercise each shared clipboard host')
        cls.owners = set()
        for match in registrations:
            name = match.group(1).strip('"')
            variable = re.fullmatch(r'\$\{([^}]+)\}', name)
            if variable:
                setting = re.search(r'\bset\(' + re.escape(variable.group(1))
                                    + r'\s+"([^"]+)"\)', workspace)
                if setting is None:
                    raise AssertionError(f'cannot resolve test alias {name}')
                name = setting.group(1)
            cls.owners.add(re.sub(r'\$\{[^}]+\}', 'fixture', name))
        cls.late_owner = 'snow-shot-pinned-future-alias-tests'
        cls.owners.add(cls.late_owner)
        cls.existing_owner = 'snow-shot-export-coordinator-tests'
        cls.unrelated = {'snow-shot-about-page-tests', 'snow-shot-clipboard-preparation-tests',
                         'snow-shot-mcp-document-tests', 'snow-shot-export-artifact-tests',
                         'snow-shot-screen-recording-controller-tests'}
        module_directory = (ROOT / 'cmake').as_posix()
        fixture = '''cmake_minimum_required(VERSION 3.21)
project(SnowClipboardResourcePolicy NONE)
enable_testing()
set(WIN32 "${FIXTURE_WINDOWS}")
'''
        for name in sorted(cls.owners - {cls.late_owner} | cls.unrelated):
            fixture += f'add_test(NAME {name} COMMAND "${{CMAKE_COMMAND}}" -E true)\n'
        fixture += f'''set_tests_properties({cls.existing_owner} PROPERTIES
    RESOURCE_LOCK fixture-existing-resource)
set(SNOW_SHOT_BUILD_TESTS ON)
list(APPEND CMAKE_MODULE_PATH "{module_directory}")
{policy.group()}
add_test(NAME {cls.late_owner} COMMAND "${{CMAKE_COMMAND}}" -E true)
'''
        (cls.source / 'CMakeLists.txt').write_text(fixture, encoding='utf-8')
        cls.tests = {}
        for platform in ('ON', 'OFF'):
            build = cls.source / platform
            configured = TestRunnerPolicyTests.run_command(
                CMAKE, '-S', str(cls.source), '-B', str(build),
                '-DCMAKE_BUILD_TYPE=Debug', f'-DFIXTURE_WINDOWS={platform}')
            if configured.returncode != 0:
                raise AssertionError(configured.stdout + configured.stderr)
            inventory = TestRunnerPolicyTests.run_command(
                CTEST, '--test-dir', str(build), '-C', 'Debug', '--show-only=json-v1')
            if inventory.returncode != 0:
                raise AssertionError(inventory.stdout + inventory.stderr)
            cls.tests[platform] = {
                test['name']: {entry['name']: entry['value']
                               for entry in test.get('properties', [])}
                for test in json.loads(inventory.stdout)['tests']}

    def test_every_shared_host_alias_owns_the_windows_clipboard(self):
        for name in self.owners:
            with self.subTest(test=name):
                resources = self.tests['ON'][name].get('RESOURCE_LOCK', [])
                self.assertIn('snow-shot-system-clipboard', resources)

    def test_unrelated_hosts_keep_parallel_execution(self):
        for name in self.unrelated:
            with self.subTest(test=name):
                self.assertNotIn('RESOURCE_LOCK', self.tests['ON'][name])

    def test_clipboard_resource_appends_to_existing_ownership(self):
        self.assertEqual(set(self.tests['ON'][self.existing_owner]['RESOURCE_LOCK']),
                         {'fixture-existing-resource', 'snow-shot-system-clipboard'})

    def test_other_platforms_keep_their_existing_resources(self):
        for name, properties in self.tests['OFF'].items():
            with self.subTest(test=name):
                self.assertNotIn('snow-shot-system-clipboard',
                                 properties.get('RESOURCE_LOCK', []))
        self.assertEqual(self.tests['OFF'][self.existing_owner]['RESOURCE_LOCK'],
                         ['fixture-existing-resource'])


if __name__ == '__main__':
    unittest.main()
