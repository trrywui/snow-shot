#!/usr/bin/env python3
"""Collect the native/Rust/Qt source materials for a locally built maintained release."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import ssl
import subprocess
import tarfile
import urllib.request

REPO = Path(__file__).resolve().parents[1]
ROOT = REPO / '.tools/macos14-personal'
SOURCES = ROOT / 'release-sources'


def digest(path, algorithm):
    result = hashlib.new(algorithm)
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            result.update(block)
    return result.hexdigest()


def records():
    result = {}
    inventories = list((ROOT / 'installed').glob('*/share/*/vcpkg.spdx.json'))
    inventories += list((ROOT / 'qt-deps').glob('*/share/*/vcpkg.spdx.json'))
    if not inventories:
        raise ValueError('No built dependency inventories found')
    for path in inventories:
        port = path.parent.name
        if port.startswith('vcpkg-'):
            continue  # Standard build tools are not linked into the delivered application.
        data = json.loads(path.read_text())
        variables = {'VERSION': data['packages'][0].get('versionInfo', '').split('#')[0]}
        portfile = ROOT / 'vcpkg/ports' / port / 'portfile.cmake'
        if portfile.exists():
            variables.update(re.findall(r'set\((\w+)\s+"?([^\s"()]+)"?\)', portfile.read_text()))

        def expand(value):
            for _ in range(8):
                updated = re.sub(r'\$\{(\w+)\}', lambda m: variables.get(m[1], m[0]), value)
                if updated == value:
                    break
                value = updated
            if '${' in value:
                raise ValueError(f'Unresolved source location: {port}: {value}')
            return value

        for package in data['packages']:
            checksums = [c['checksumValue'] for c in package.get('checksums', [])
                         if c['algorithm'] == 'SHA512']
            if not checksums:
                continue
            name = package['name']
            if name.startswith('opencv-cache/') and '/tiny_dnn/' not in name:
                continue  # Other-platform/disabled optional data listed in the port template.
            location = expand(package['downloadLocation'])
            if location.startswith('git+'):
                repository, ref = location[4:].rsplit('@', 1)
                repository = repository.rstrip('/')
                if repository.startswith('https://github.com/'):
                    url = repository.replace('https://github.com/', 'https://codeload.github.com/')
                    url += '/tar.gz/' + ref
                elif repository.startswith('https://bitbucket.com/'):
                    url = repository.replace('bitbucket.com', 'bitbucket.org') + '/get/' + ref + '.tar.gz'
                else:
                    project = repository.rsplit('/', 1)[-1]
                    url = repository + '/-/archive/' + ref + '/' + project + '-' + ref + '.tar.gz'
            else:
                url = location
            if not url.startswith('https://'):
                raise ValueError('Source downloads require HTTPS')
            checksum = checksums[0]
            result[checksum] = {'port': port, 'name': name, 'url': url,
                                'sha512': checksum, 'file': checksum + '.source'}
    # Git-fetched sources do not carry archive checksums in the vcpkg inventories.
    # Their full revisions are pinned in the original ports bundled below.
    for name, repository, revision in (
        ('aom', 'https://aomedia.googlesource.com/aom', 'd772e334cc724105040382a977ebb10dfd393293'),
        ('crashpad', 'https://chromium.googlesource.com/crashpad/crashpad', '7e0af1d4d45b526f01677e74a56f4a951b70517d'),
        ('mini_chromium', 'https://chromium.googlesource.com/chromium/mini_chromium', 'dce72d97d1c2e9beb5e206c6a05a702269794ca3'),
    ):
        result[name] = {'port': name, 'name': name, 'revision': revision,
                        'url': repository + '/+archive/' + revision + '.tar.gz',
                        'file': name + '-' + revision + '.tar.gz'}
    return list(result.values())


def prepare():
    SOURCES.mkdir(parents=True, exist_ok=True)
    context = ssl.create_default_context()
    local = {}
    for directory in (ROOT / 'downloads', ROOT / 'vcpkg/downloads', ROOT / 'qt-vcpkg/downloads'):
        for path in directory.rglob('*'):
            if path.is_file() and not path.is_symlink() and not any(
                part in ('tools', 'temp', 'git-tmp') for part in path.relative_to(directory).parts
            ) and any(path.name.endswith(s) for s in ('.tar.gz', '.tar.xz', '.tar.bz2', '.tgz', '.zip', '.patch', '.gz')):
                local[digest(path, 'sha512')] = path

    def fetch(record):
        output = SOURCES / record['file']
        expected = record.get('sha512')
        if expected and expected in local and not output.exists():
            shutil.copy2(local[expected], output)
        if not output.exists():
            temporary = output.with_suffix(output.suffix + '.part')
            request = urllib.request.Request(record['url'], headers={'User-Agent': 'SnowShot-source-release'})
            with urllib.request.urlopen(request, timeout=90, context=context) as response:
                with temporary.open('wb') as destination:
                    shutil.copyfileobj(response, destination)
            if expected and digest(temporary, 'sha512') != expected:
                raise ValueError(f'Source checksum mismatch: {record["name"]}')
            temporary.replace(output)
        actual = digest(output, 'sha512')
        if expected and actual != expected:
            raise ValueError(f'Source checksum mismatch: {record["name"]}')
        record['sha512'] = actual
        print('Source ready:', record['name'], flush=True)
        return record

    with ThreadPoolExecutor(max_workers=2) as executor:
        collected = list(executor.map(fetch, records()))
    metadata = json.loads((ROOT / 'qt/share/snow-apps/static-qt-build.json').read_text())
    qt = ROOT / 'downloads' / f'qt-everywhere-src-{metadata["QtVersion"]}.tar.xz'
    if digest(qt, 'sha256') != metadata['SourceArchiveSha256']:
        raise ValueError('Qt source archive does not match the linked kit')
    (SOURCES / 'manifest.json').write_text(json.dumps(
        {'native': collected, 'qt': metadata}, ensure_ascii=False, indent=2) + '\n')
    registry = SOURCES / 'vcpkg-registry.tar.gz'
    subprocess.run(['git', '-C', str(ROOT / 'vcpkg'), 'archive', '--format=tar.gz',
                    '-o', str(registry), 'HEAD'], check=True)
    (SOURCES / 'vcpkg-local.patch').write_bytes(subprocess.check_output(
        ['git', '-C', str(ROOT / 'vcpkg'), 'diff', '--binary', 'HEAD']))



def cargo_git_sources():
    locked = {}
    for name in ('snow-crates', 'snow_rust_ffi'):
        # Cargo emits package scalar fields as quoted strings. Read only those fields;
        # keep this packaging helper usable with macOS's pre-tomllib system Python.
        text = (REPO / name / 'Cargo.lock').read_text()
        for block in re.split(r'(?m)^\[\[package\]\]\s*$', text)[1:]:
            package = dict(re.findall(
                r'(?m)^(name|version|source)\s*=\s*"([^"\r\n]*)"\s*$', block))
            source = package.get('source', '')
            if source.startswith('git+'):
                locked.setdefault(source, set()).add(package['name'] + '-' + package['version'])
    if not locked:
        return []
    checkouts = ROOT / 'cargo/git/checkouts'
    revisions = {}
    for candidate in sorted(checkouts.glob('*/*')):
        if not candidate.is_dir() or not (candidate / '.git').exists():
            continue
        revision = subprocess.check_output(
            ['git', '-C', str(candidate), 'rev-parse', 'HEAD'], text=True).strip()
        revisions.setdefault(revision, candidate.relative_to(checkouts).as_posix())
    records = []
    for source, packages in sorted(locked.items()):
        revision = source.rsplit('#', 1)[-1]
        if not re.fullmatch(r'[0-9a-f]{40}', revision) or revision not in revisions:
            raise ValueError(f'Missing locked Cargo Git source: {source}')
        records.append({'source': source, 'revision': revision, 'packages': sorted(packages),
                        'checkout': revisions[revision]})
    return records


def without_git_metadata(member):
    # Cargo Git sources need their working tree, never a local Git database/config.
    return None if '.git' in Path(member.name).parts else member


def archive(output):
    manifest = json.loads((SOURCES / 'manifest.json').read_text())
    git_sources = cargo_git_sources()
    if output.exists() or output.with_suffix(output.suffix + '.part').exists():
        raise ValueError('Refusing to overwrite a source release')
    temporary = output.with_suffix(output.suffix + '.part')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(temporary, 'w:gz', compresslevel=1) as bundle:
        for record in manifest['native']:
            path = SOURCES / record['file']
            if digest(path, 'sha512') != record['sha512']:
                raise ValueError('Source changed after preparation')
            bundle.add(path, arcname='native/' + record['file'])
        for name in ('manifest.json', 'vcpkg-registry.tar.gz', 'vcpkg-local.patch'):
            bundle.add(SOURCES / name, arcname=name)
        qt_name = f'qt-everywhere-src-{manifest["qt"]["QtVersion"]}.tar.xz'
        bundle.add(ROOT / 'downloads' / qt_name, arcname='qt/' + qt_name)
        for directory in (ROOT / 'cargo/registry/src').iterdir():
            if directory.is_dir():
                bundle.add(directory, arcname='cargo/' + directory.name)
        for record in git_sources:
            bundle.add(ROOT / 'cargo/git/checkouts' / record['checkout'],
                       arcname='cargo-git/' + record['checkout'], filter=without_git_metadata)
        provenance = (json.dumps(git_sources, indent=2) + '\n').encode()
        entry = tarfile.TarInfo('cargo-git-sources.json')
        entry.size = len(provenance)
        entry.mode = 0o644
        bundle.addfile(entry, io.BytesIO(provenance))
        for directory in (ROOT / 'installed', ROOT / 'qt-deps'):
            for path in directory.glob('*/share/*/vcpkg.spdx.json'):
                bundle.add(path, arcname='inventories/' + path.relative_to(ROOT).as_posix())
    temporary.replace(output)
    print('Source archive:', output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepare', action='store_true')
    parser.add_argument('--archive', type=Path)
    args = parser.parse_args()
    if not args.prepare and not args.archive:
        parser.error('Select --prepare and/or --archive')
    if args.prepare:
        prepare()
    if args.archive:
        archive(args.archive)


if __name__ == '__main__':
    main()
