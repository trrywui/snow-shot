"""Focused failure fixtures for canonical MCP catalogs and recording constraints."""

import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import generate_mcp_capabilities as generator


class McpCapabilityGenerationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = generator.load_manifest()

    def fixture(self, root):
        source = root / generator.MANIFEST
        source.write_text(json.dumps(self.manifest), encoding='utf-8')
        for name, contents in generator.generated_outputs(root).items():
            output = root / name
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_text(contents, encoding='utf-8')
        return source

    def test_checked_in_outputs_are_current(self):
        generator.check_generated()

    def test_shared_reviewed_metadata_reaches_both_editions(self):
        manifest = copy.deepcopy(self.manifest)
        manifest['catalog']['surface_contracts']['recording_options']['adapter'] = 'Reviewed adapter'
        before = copy.deepcopy(manifest)
        catalogs = generator.catalogs(manifest)
        self.assertEqual(manifest, before, 'generation must not mutate reviewed inputs')
        for catalog in catalogs.values():
            contract = catalog['surface_contracts']['recording_options']
            self.assertEqual(contract['adapter'], 'Reviewed adapter')
            self.assertEqual(contract['members'], list(contract['validation']))
        full_tools = {tool for domain in catalogs['mcp-capabilities.json']['domains'].values()
                      for tool in domain['tools']}
        mini_tools = {tool for domain in catalogs['mcp-capabilities-mini.json']['domains'].values()
                      for tool in domain['tools']}
        self.assertEqual(full_tools - mini_tools, set(manifest['mini']['disabled_tools']))

    def test_modified_or_missing_generated_outputs_are_rejected(self):
        for name in ['mcp-capabilities.json', 'mcp-capabilities-mini.json',
                     generator.RECORDING_HEADER]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.fixture(root)
                generator.check_generated(root)
                output = root / name
                output.write_text('stale\n', encoding='utf-8')
                with self.assertRaisesRegex(ValueError, 'stale'):
                    generator.check_generated(root)
                output.unlink()
                with self.assertRaisesRegex(ValueError, 'stale'):
                    generator.check_generated(root)

    def test_changed_bounds_and_choices_update_header_and_both_catalogs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self.fixture(root)
            manifest = copy.deepcopy(self.manifest)
            validation = manifest['catalog']['surface_contracts']['recording_options']['validation']
            validation['mouse_highlight_radius']['minimum'] = 9
            validation['encoder']['enum'].append('h266')
            source.write_text(json.dumps(manifest), encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'stale'):
                generator.check_generated(root)
            generated = generator.generated_outputs(root)
            for name in ['mcp-capabilities.json', 'mcp-capabilities-mini.json']:
                actual = json.loads(generated[name])['surface_contracts']['recording_options']
                self.assertEqual(actual['validation']['mouse_highlight_radius']['minimum'], 9)
                self.assertIn('h266', actual['validation']['encoder']['enum'])
            self.assertIn('QStringLiteral("h266")', generated[generator.RECORDING_HEADER])
            self.assertIn('{QStringLiteral("minimum"), 9}', generated[generator.RECORDING_HEADER])

    def test_unknown_duplicate_or_malformed_mini_exclusions_are_rejected(self):
        for excluded in [['NotARealRenderer'], ['McpStatus', 'McpStatus'], 'McpStatus']:
            with self.subTest(excluded=excluded):
                manifest = copy.deepcopy(self.manifest)
                manifest['mini']['surface_contracts']['settings_custom_renderers'][
                    'exclude_members'] = excluded
                with self.assertRaises(ValueError):
                    generator.catalogs(manifest)
        for disabled in [['not_a_tool'], ['snow_shot_app_status', 'snow_shot_app_status']]:
            with self.subTest(disabled=disabled):
                manifest = copy.deepcopy(self.manifest)
                manifest['mini']['disabled_tools'] = disabled
                with self.assertRaises(ValueError):
                    generator.catalogs(manifest)

    def test_misspelled_policy_fields_cannot_be_ignored(self):
        for name in ['mini', 'surface']:
            with self.subTest(name=name):
                manifest = copy.deepcopy(self.manifest)
                target = manifest['mini'] if name == 'mini' else manifest['mini'][
                    'surface_contracts']['settings_custom_renderers']
                target['exclude_member'] = ['McpStatus']
                with self.assertRaisesRegex(ValueError, 'fields'):
                    generator.catalogs(manifest)

    def test_invalid_primitive_constraints_are_rejected(self):
        invalid = [
            {'type': 'unknown'},
            {'type': 'integer', 'minimum': 129, 'maximum': 128},
            {'type': 'integer', 'minimum': 8},
            {'type': 'integer', 'minimum': False, 'maximum': 128},
            {'type': 'integer'},
            {'type': 'integer', 'enum': [8, 8]},
            {'type': 'integer', 'enum': [8, True]},
            {'type': 'string', 'enum': []},
            {'type': 'string', 'enum': ['h264', 'h264']},
            {'type': 'integer', 'minimum': 8, 'maximum': 128, 'enum': [7]},
            {'type': 'integer', 'format': 'qt-color', 'enum': [8]},
            {'type': 'string', 'format': 'unknown'},
            {'type': 'string', 'minimun': 8},
        ]
        for constraint in invalid:
            with self.subTest(constraint=constraint):
                manifest = copy.deepcopy(self.manifest)
                manifest['catalog']['surface_contracts']['recording_options']['validation'][
                    'mouse_highlight_radius'] = constraint
                with self.assertRaises(ValueError):
                    generator.catalogs(manifest)

    def test_duplicate_manifest_declarations_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / generator.MANIFEST).write_text(
                '{"catalog": {}, "catalog": {}, "mini": {}}', encoding='utf-8')
            with self.assertRaisesRegex(ValueError, 'Duplicate'):
                generator.load_manifest(root)

    def test_recording_member_list_cannot_duplicate_constraint_keys(self):
        manifest = copy.deepcopy(self.manifest)
        manifest['catalog']['surface_contracts']['recording_options']['members'] = ['path']
        with self.assertRaisesRegex(ValueError, 'members derive'):
            generator.catalogs(manifest)


if __name__ == '__main__':
    unittest.main()
