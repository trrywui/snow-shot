#!/usr/bin/env python3
"""Focused failure and continuity checks; never modifies a real keychain."""
import hashlib
import importlib.util
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('signing', ROOT / 'scripts/macos14-signing.py')
signing = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(signing)


class MaintainedSigning(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.certificate = b'public certificate fixture'
        self.fingerprint = hashlib.sha1(self.certificate).hexdigest().upper()
        self.requirement = (f'identifier "{signing.IDENTIFIER}" and '
                            f'certificate leaf = H"{self.fingerprint.lower()}"')
        self.policy = {'schemaVersion': 1, 'bundleIdentifier': signing.IDENTIFIER,
                       'certificateSha1': self.fingerprint,
                       'designatedRequirement': self.requirement}
        self.path = self.directory / 'policy.json'
        self.path.write_text(json.dumps(self.policy))
        self.app = self.directory / 'Snow Shot.app'
        (self.app / 'Contents').mkdir(parents=True)
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': signing.IDENTIFIER, 'CFBundleVersion': '6'}))
        self.calls = []

    def fake_run(self, *arguments, **kwargs):
        self.calls.append(arguments)
        if arguments[0] == '/usr/bin/security':
            return subprocess.CompletedProcess(arguments, 0,
                                               f'  1) {self.fingerprint} "Maintained"\n', '')
        if '-r-' in arguments:
            return subprocess.CompletedProcess(arguments, 0, '',
                                               '# designated => ' + self.requirement + '\n')
        for argument in arguments:
            if argument.startswith('--extract-certificates='):
                Path(argument.split('=', 1)[1] + '0').write_bytes(self.certificate)
        return subprocess.CompletedProcess(arguments, 0, '', '')

    def test_policy_rejects_changed_identifier_fingerprint_and_code_hash_requirement(self):
        for field, value in [('schemaVersion', 2), ('schemaVersion', True),
                             ('bundleIdentifier', 'com.snowshot.snow_shot'),
                             ('certificateSha1', '-'), ('certificateSha1', 123),
                             ('designatedRequirement', 'cdhash H"abc"'),
                             ('designatedRequirement', self.requirement + ' or true')]:
            with self.subTest(field=field, value=value):
                self.path.write_text(json.dumps(dict(self.policy, **{field: value})))
                with self.assertRaises(ValueError):
                    signing.read_policy(self.path)

    def test_missing_policy_never_provisions_an_identity(self):
        with patch.object(signing, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'restore it from Git'):
                signing.read_policy(self.directory / 'missing.json')
            run.assert_not_called()

    def test_identity_lookup_is_read_only_and_uses_the_pinned_fingerprint(self):
        with patch.object(signing, 'run', side_effect=self.fake_run):
            self.assertEqual(signing.identity(signing.read_policy(self.path)), self.fingerprint)
        self.assertEqual(self.calls, [('/usr/bin/security', 'find-identity', '-v', '-p', 'codesigning')])

    def test_missing_or_different_private_key_fails_without_import_or_fallback(self):
        for output in ['0 valid identities found\n', '  1) ' + 'F' * 40 + ' "Maintained"\n']:
            with patch.object(signing, 'run', return_value=subprocess.CompletedProcess(
                    [], 0, output, '')) as run:
                with self.assertRaisesRegex(ValueError, 'Restore the original identity'):
                    signing.identity(self.policy)
                self.assertEqual(run.call_count, 1)

    def test_initialization_cannot_replace_an_established_or_partial_identity(self):
        with patch.object(signing, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'already exists'):
                signing.provision(self.path)
            self.path.unlink()
            self.path.with_suffix('.initializing').touch()
            with self.assertRaises(FileExistsError):
                signing.provision(self.path)
            run.assert_not_called()

    def test_verification_checks_real_requirement_certificate_and_deep_seal(self):
        with patch.object(signing, 'run', side_effect=self.fake_run):
            result = signing.verify(self.app, self.policy)
        self.assertEqual(result['certificateSha1'], self.fingerprint)
        self.assertEqual(self.calls[0], ('/usr/bin/codesign', '--verify', '--deep', '--strict',
                                        '-R', '=' + self.requirement, str(self.app)))
        self.assertFalse(any(call[0] == '/usr/bin/security' for call in self.calls))

    def test_wrong_certificate_is_rejected_even_if_a_requirement_command_succeeds(self):
        self.policy['certificateSha1'] = '0' * 40
        with patch.object(signing, 'run', side_effect=self.fake_run):
            with self.assertRaisesRegex(ValueError, 'pinned certificate'):
                signing.verify(self.app, self.policy)

    def test_changed_designated_requirement_is_rejected(self):
        self.policy['designatedRequirement'] += ' and true'
        with patch.object(signing, 'run', side_effect=self.fake_run):
            with self.assertRaisesRegex(ValueError, 'changed its designated requirement'):
                signing.verify(self.app, self.policy)

    def test_wrong_bundle_identifier_stops_before_signature_validation(self):
        (self.app / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': 'com.snowshot.snow_shot'}))
        with patch.object(signing, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'bundle identifier'):
                signing.verify(self.app, self.policy)
            run.assert_not_called()

    def test_command_failure_does_not_print_secret_arguments(self):
        with patch.object(subprocess, 'run', return_value=subprocess.CompletedProcess(
                [], 1, '', 'import denied')):
            with self.assertRaises(ValueError) as error:
                signing.run('/usr/bin/security', 'import', '-P', 'SECRET')
        self.assertNotIn('SECRET', str(error.exception))

    def test_command_timeout_does_not_print_secret_arguments(self):
        with patch.object(subprocess, 'run', side_effect=subprocess.TimeoutExpired(
                ['/usr/bin/security', 'import', '-P', 'SECRET'], 60)):
            with self.assertRaises(ValueError) as error:
                signing.run('/usr/bin/security', 'import', '-P', 'SECRET')
        self.assertNotIn('SECRET', str(error.exception))

    def test_recovery_never_provisions_without_an_interrupted_initialization(self):
        with patch.object(signing, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'interrupted first initialization'):
                signing.finish_provision(self.directory / 'absent-policy.json')
            with self.assertRaisesRegex(ValueError, 'interrupted first initialization'):
                signing.finish_provision(self.path)
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
