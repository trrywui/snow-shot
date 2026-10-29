#!/usr/bin/env python3
"""Provision once and verify the maintained edition's persistent signing identity."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import secrets
import shutil
import ssl
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / 'snow_shot/packaging/macos/maintained-signing.json'
IDENTIFIER = 'io.github.trrywui.snowshot'
CERTIFICATE_NAME = 'Snow Shot Maintained (trrywui)'


def run(*arguments, check=True, env=None):
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, env=env, timeout=60)
    except subprocess.TimeoutExpired as error:
        raise ValueError(f'{Path(arguments[0]).name} timed out') from error
    if check and result.returncode:
        # Arguments can contain the temporary PKCS#12 import password.
        raise ValueError(f'{Path(arguments[0]).name} failed: {result.stderr.strip()}')
    return result


def read_policy(path=POLICY):
    try:
        policy = json.loads(path.read_text())
    except FileNotFoundError as error:
        raise ValueError('Signing policy is missing; restore it from Git. '
                         'Only the first provisioning uses the explicit init command.') from error
    if not isinstance(policy, dict):
        raise ValueError('Invalid maintained signing policy')
    fingerprint = policy.get('certificateSha1', '')
    requirement = policy.get('designatedRequirement', '')
    if (type(policy.get('schemaVersion')) is not int or policy['schemaVersion'] != 1
            or policy.get('bundleIdentifier') != IDENTIFIER
            or not isinstance(fingerprint, str)
            or not re.fullmatch(r'[0-9A-F]{40}', fingerprint)
            or requirement != (f'identifier "{IDENTIFIER}" and '
                               f'certificate leaf = H"{fingerprint.lower()}"')):
        raise ValueError('Invalid maintained signing policy; restore the pinned policy from Git')
    return policy


def identity(policy):
    fingerprint = policy['certificateSha1']
    available = run('/usr/bin/security', 'find-identity', '-v', '-p', 'codesigning').stdout
    if not re.search(r'^\s*\d+\)\s+' + fingerprint + r'\s+"', available, re.MULTILINE):
        raise ValueError('The pinned signing certificate/private key is unavailable. '
                         'Restore the original identity in Keychain Access; '
                         'a new certificate or ad-hoc fallback would lose permission continuity.')
    return fingerprint


def requirement(app):
    result = run('/usr/bin/codesign', '-d', '-r-', str(app))
    match = re.search(r'^(?:# )?designated => (.+)$', result.stdout + result.stderr, re.MULTILINE)
    if not match:
        raise ValueError('The signed application has no designated requirement')
    return match[1]


def verify(app, policy):
    metadata = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if metadata.get('CFBundleIdentifier') != policy['bundleIdentifier']:
        raise ValueError('The application bundle identifier does not match the signing policy')
    run('/usr/bin/codesign', '--verify', '--deep', '--strict', '-R',
        '=' + policy['designatedRequirement'], str(app))
    actual = requirement(app)
    if actual != policy['designatedRequirement']:
        raise ValueError('The application changed its designated requirement')
    with tempfile.TemporaryDirectory(prefix='snow-shot-public-certificate-') as temporary:
        prefix = Path(temporary) / 'certificate'
        run('/usr/bin/codesign', '-d', '--extract-certificates=' + str(prefix), str(app))
        certificate = Path(str(prefix) + '0')
        if (not certificate.is_file()
                or hashlib.sha1(certificate.read_bytes()).hexdigest().upper()
                != policy['certificateSha1']):
            raise ValueError('The application is not signed by the pinned certificate')
    return {'bundleIdentifier': metadata['CFBundleIdentifier'],
            'version': metadata.get('CFBundleShortVersionString'),
            'buildNumber': metadata.get('CFBundleVersion'),
            'certificateSha1': policy['certificateSha1'], 'designatedRequirement': actual}


def provision(path=POLICY):
    if path.exists() or path.is_symlink():
        raise ValueError('The signing policy already exists. Restore its original private key; '
                         'initialization never replaces an established identity.')
    marker = path.with_suffix('.initializing')
    # A failed first provisioning must never silently mint a second identity.
    with marker.open('x') as output:
        output.write('Provisioning started; recover any imported identity before retrying.\n')
    existing = run('/usr/bin/security', 'find-certificate', '-c', CERTIFICATE_NAME, check=False)
    if existing.returncode == 0:
        raise ValueError('An unpinned maintained certificate already exists; recover its policy')
    if existing.returncode != 44:
        raise ValueError('Could not establish that the maintained certificate is absent')
    keychain = Path.home() / 'Library/Keychains/login.keychain-db'
    if not keychain.is_file():
        raise ValueError('The login keychain is unavailable')
    with tempfile.TemporaryDirectory(prefix='snow-shot-signing-') as temporary:
        directory = Path(temporary)
        private_key = directory / 'private.pem'
        certificate = directory / 'certificate.pem'
        bundle = directory / 'identity.p12'
        config = directory / 'certificate.cnf'
        config.write_text(f'''[req]
prompt = no
distinguished_name = subject
x509_extensions = signing
[subject]
CN = {CERTIFICATE_NAME}
[signing]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = codeSigning
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
''')
        run('openssl', 'req', '-new', '-newkey', 'rsa:2048', '-nodes', '-x509', '-days', '3650',
            '-config', str(config), '-keyout', str(private_key), '-out', str(certificate))
        fingerprint_output = run('openssl', 'x509', '-in', str(certificate), '-noout',
                                 '-fingerprint', '-sha1').stdout
        fingerprint = fingerprint_output.strip().split('=')[-1].replace(':', '').upper()
        if not re.fullmatch(r'[0-9A-F]{40}', fingerprint):
            raise ValueError('Could not determine the generated certificate fingerprint')
        password = secrets.token_hex(32)
        # Use the PKCS#12 algorithms supported by Apple's importer.
        run('openssl', 'pkcs12', '-export', '-inkey', str(private_key), '-in', str(certificate),
            '-out', str(bundle), '-keypbe', 'PBE-SHA1-3DES', '-certpbe', 'PBE-SHA1-3DES',
            '-macalg', 'sha1', '-passout', 'env:SNOW_SIGNING_IMPORT_PASSWORD',
            env=dict(os.environ, SNOW_SIGNING_IMPORT_PASSWORD=password))
        run('/usr/bin/security', 'import', str(bundle), '-k', str(keychain), '-P', password,
            '-T', '/usr/bin/codesign')
        # User trust only, limited to code signing; no system trust or broad key ACL.
        run('/usr/bin/security', 'add-trusted-cert', '-r', 'trustRoot', '-p', 'codeSign',
            '-k', str(keychain), str(certificate))
    return finish_provision(path, fingerprint)


def finish_provision(path=POLICY, expected_fingerprint=None):
    if path.exists() or not path.with_suffix('.initializing').is_file():
        raise ValueError('Recovery is only for an interrupted first initialization')
    keychain = Path.home() / 'Library/Keychains/login.keychain-db'
    public = run('/usr/bin/security', 'find-certificate', '-a', '-c', CERTIFICATE_NAME,
                 '-p', str(keychain)).stdout
    certificates = re.findall(r'-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----',
                              public, re.DOTALL)
    if len(certificates) != 1:
        raise ValueError('Expected exactly one maintained certificate; recover it explicitly')
    fingerprint = hashlib.sha1(ssl.PEM_cert_to_DER_cert(certificates[0])).hexdigest().upper()
    if expected_fingerprint and fingerprint != expected_fingerprint:
        raise ValueError('The imported certificate does not match the generated certificate')
    with tempfile.TemporaryDirectory(prefix='snow-shot-signing-probe-') as temporary:
        directory = Path(temporary)
        probe = directory / 'Snow Shot.app'
        executable = probe / 'Contents/MacOS/probe'
        executable.parent.mkdir(parents=True)
        # copyfile avoids inheriting protected flags from Apple's system executable.
        shutil.copyfile('/usr/bin/true', executable)
        executable.chmod(0o755)
        (probe / 'Contents/Info.plist').write_bytes(plistlib.dumps({
            'CFBundleIdentifier': IDENTIFIER, 'CFBundleExecutable': 'probe',
            'CFBundlePackageType': 'APPL'}))
        run('/usr/bin/codesign', '--force', '--sign', fingerprint,
            '--identifier', IDENTIFIER, str(probe))
        policy = {'schemaVersion': 1, 'bundleIdentifier': IDENTIFIER,
                  'certificateName': CERTIFICATE_NAME, 'certificateSha1': fingerprint,
                  'designatedRequirement': requirement(probe)}
        candidate = directory / 'policy.json'
        candidate.write_text(json.dumps(policy, indent=2) + '\n')
        policy = read_policy(candidate)
        identity(policy)
        verify(probe, policy)
        with path.open('x') as output:
            output.write(json.dumps(policy, indent=2) + '\n')
    path.with_suffix('.initializing').unlink()
    return policy


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['init', 'recover', 'identity', 'verify'])
    parser.add_argument('--app', type=Path)
    args = parser.parse_args()
    if args.command == 'init':
        print(json.dumps(provision(), indent=2))
    elif args.command == 'recover':
        print(json.dumps(finish_provision(), indent=2))
    elif args.command == 'identity':
        print(identity(read_policy()))
    else:
        if not args.app:
            parser.error('verify requires --app')
        print(json.dumps(verify(args.app, read_policy()), indent=2))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        print(f'Maintained signing: {error}', file=sys.stderr)
        sys.exit(1)
