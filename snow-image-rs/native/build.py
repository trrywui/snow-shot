#!/usr/bin/env python3
"""Explicit, checksum-verified native dependency preparation. Cargo never downloads."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import urllib.request

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
CACHE = ROOT / 'target' / 'native'
MANIFEST = json.loads((HERE / 'sources.json').read_text())


def host_target():
    arch = 'aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'
    system = {'Darwin': 'apple-darwin', 'Windows': 'pc-windows-msvc'}.get(platform.system())
    if not system:
        raise SystemExit('Native release builds support Windows and macOS only')
    return f'{arch}-{system}'


def checked_archive(name, fetch):
    spec = MANIFEST['sources'][name]
    dest = CACHE / 'downloads' / spec['archive']
    dest.parent.mkdir(parents=True, exist_ok=True)
    if not dest.exists():
        if not fetch:
            raise SystemExit(f'{name}: missing archive; run native/build.py fetch first')
        partial = dest.with_suffix('.partial')
        try:
            with urllib.request.urlopen(spec['url'], timeout=120) as src, partial.open('wb') as out:
                shutil.copyfileobj(src, out, 1024 * 1024)
            if hashlib.sha256(partial.read_bytes()).hexdigest() != spec['sha256']:
                raise SystemExit(f'{name}: SHA-256 mismatch')
            partial.replace(dest)
        finally:
            partial.unlink(missing_ok=True)
    with dest.open('rb') as stream:
        digest = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    if digest.hexdigest() != spec['sha256']:
        raise SystemExit(f'{name}: SHA-256 mismatch in cached archive')
    return dest


def extract(name, archive):
    out = CACHE / 'sources' / name
    stamp = out / '.snow-sha256'
    expected = MANIFEST['sources'][name]['sha256']
    if stamp.exists() and stamp.read_text() == expected:
        return out
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            components = Path(member.name).parts
            if len(components) < 2:
                continue
            member.name = str(Path(*components[1:]))
            dest = out / member.name
            if not dest.resolve().is_relative_to(out.resolve()):
                raise SystemExit(f'{name}: invalid archive path')
            if member.issym() or member.islnk():
                # Verified dependencies do not require links; prevent archive escapes.
                continue
            if not (member.isfile() or member.isdir()):
                continue
            tar.extract(member, out)
    stamp.write_text(expected)
    return out


def run(command, env):
    print(' '.join(map(str, command)), flush=True)
    subprocess.run(list(map(str, command)), check=True, env=env)


def build(target, jobs, sanitizers=""):
    variant = target + ("-" + sanitizers.replace(",", "-") if sanitizers else "")
    if target not in ('aarch64-apple-darwin', 'x86_64-apple-darwin',
                      'aarch64-pc-windows-msvc', 'x86_64-pc-windows-msvc'):
        raise SystemExit('Unsupported target')
    if target != host_target():
        raise SystemExit('Use a native build host matching --target; cross outputs are not release validated')
    prefix = CACHE / variant / 'prefix'
    work = CACHE / variant / 'build'
    prefix.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env['PKG_CONFIG_PATH'] = str(prefix / 'lib' / 'pkgconfig')
    env['PKG_CONFIG_LIBDIR'] = env['PKG_CONFIG_PATH']
    env['PYTHONPATH'] = str(CACHE / 'tools') + os.pathsep + env.get('PYTHONPATH', '')
    if platform.system() == 'Windows':
        env.setdefault('CC', 'clang-cl')
        env.setdefault('CXX', 'clang-cl')
    common = ['-G', 'Ninja', '-DCMAKE_BUILD_TYPE=Release', f'-DCMAKE_INSTALL_PREFIX={prefix}',
              f'-DCMAKE_PREFIX_PATH={prefix}', '-DCMAKE_INSTALL_LIBDIR=lib',
              '-DBUILD_SHARED_LIBS=OFF', '-DCMAKE_POSITION_INDEPENDENT_CODE=ON',
              '-DCMAKE_POLICY_VERSION_MINIMUM=3.10']
    if platform.system() == 'Darwin':
        common += ['-DCMAKE_OSX_DEPLOYMENT_TARGET=15.0',
                   '-DCMAKE_OSX_ARCHITECTURES=' + ('arm64' if target.startswith('aarch64') else 'x86_64')]
    else:
        common += ['-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL', '-DCMAKE_C_COMPILER=clang-cl', '-DCMAKE_CXX_COMPILER=clang-cl',
                   f'-DCMAKE_C_COMPILER_TARGET={target}', f'-DCMAKE_CXX_COMPILER_TARGET={target}']
    if sanitizers:
        flags = '-fsanitize=' + sanitizers + ' -fno-omit-frame-pointer'
        common += [f'-DCMAKE_C_FLAGS={flags}', f'-DCMAKE_CXX_FLAGS={flags}', f'-DCMAKE_EXE_LINKER_FLAGS={flags}', f'-DCMAKE_SHARED_LINKER_FLAGS={flags}']
        env['CFLAGS'] = flags
        env['CXXFLAGS'] = flags
    sources = {name: extract(name, checked_archive(name, False)) for name in MANIFEST['sources']}

    def cmake(name, options=(), source=None):
        path = work / name
        run(['cmake', '-S', source or sources[name], '-B', path, *common, *options], env)
        run(['cmake', '--build', path, '--parallel', jobs], env)
        run(['cmake', '--install', path], env)

    cmake('lcms', [f'-DSNOW_SOURCE={sources["lcms"]}'], HERE / 'lcms')
    cmake('zimg', [f'-DSNOW_SOURCE={sources["zimg"]}'], HERE / 'zimg')
    # libjpeg-turbo otherwise overrides CMAKE_MSVC_RUNTIME_LIBRARY to /MT.
    cmake('jpeg', ['-DENABLE_SHARED=OFF', '-DENABLE_STATIC=ON', '-DWITH_CRT_DLL=ON', '-DWITH_TURBOJPEG=OFF',
                   '-DWITH_TOOLS=OFF', '-DWITH_TESTS=OFF'])
    cmake('webp', ['-DWEBP_BUILD_ANIM_UTILS=OFF', '-DWEBP_BUILD_CWEBP=OFF', '-DWEBP_BUILD_DWEBP=OFF',
                  '-DWEBP_BUILD_GIF2WEBP=OFF', '-DWEBP_BUILD_IMG2WEBP=OFF', '-DWEBP_BUILD_VWEBP=OFF',
                  '-DWEBP_BUILD_WEBPINFO=OFF', '-DWEBP_BUILD_WEBPMUX=OFF', '-DWEBP_BUILD_EXTRAS=OFF'])
    cmake('highway', ['-DHWY_ENABLE_TESTS=OFF', '-DHWY_ENABLE_EXAMPLES=OFF', '-DHWY_ENABLE_CONTRIB=OFF'])
    cmake('brotli-private', [f'-DSNOW_SOURCE={sources["brotli"]}'], HERE / 'brotli')
    cmake('jxl', ['-DBUILD_TESTING=OFF', '-DJPEGXL_ENABLE_TOOLS=OFF', '-DJPEGXL_ENABLE_DEVTOOLS=OFF',
                 '-DJPEGXL_ENABLE_EXAMPLES=OFF', '-DJPEGXL_ENABLE_BENCHMARK=OFF', '-DJPEGXL_ENABLE_SJPEG=OFF', '-DJPEGXL_ENABLE_OPENEXR=OFF',
                 '-DJPEGXL_ENABLE_MANPAGES=OFF', '-DJPEGXL_ENABLE_JPEGLI=OFF',
                 '-DJPEGXL_ENABLE_JPEGLI_LIBJPEG=OFF', '-DJPEGXL_ENABLE_SKCMS=OFF',
                 '-DJPEGXL_FORCE_SYSTEM_HWY=ON', '-DJPEGXL_FORCE_SYSTEM_BROTLI=ON',
                 '-DJPEGXL_FORCE_SYSTEM_LCMS2=ON', '-DJPEGXL_ENABLE_DOXYGEN=OFF',
                 '-DJPEGXL_ENABLE_PLUGINS=OFF', '-DJPEGXL_ENABLE_FUZZERS=OFF', '-DJPEGXL_ENABLE_JNI=OFF', '-DJPEGXL_ENABLE_VIEWERS=OFF'])
    meson_work = work / 'dav1d'
    meson = [sys.executable, '-m', 'mesonbuild.mesonmain']
    options = ['--prefix', str(prefix), '--libdir', 'lib', '--buildtype', 'release',
               '--default-library', 'static', '-Denable_tools=false', '-Denable_tests=false',
               '-Denable_examples=false', '-Denable_docs=false']
    if platform.system() == 'Windows':
        options += ['-Db_vscrt=md']
    run([*meson, 'setup', *(['--reconfigure'] if (meson_work / 'build.ninja').exists() else []),
         meson_work, sources['dav1d'], *options], env)
    run([*meson, 'compile', '-C', meson_work, '-j', jobs], env)
    run([*meson, 'install', '-C', meson_work], env)
    cmake('aom', ['-DENABLE_TESTS=OFF', '-DENABLE_DOCS=OFF', '-DENABLE_EXAMPLES=OFF',
                 '-DENABLE_TOOLS=OFF', '-DCONFIG_AV1_DECODER=0'])
    cmake('avif', ['-DAVIF_CODEC_AOM=SYSTEM', '-DAVIF_CODEC_AOM_DECODE=OFF',
                  '-DAVIF_CODEC_AOM_ENCODE=ON', '-DAVIF_CODEC_DAV1D=SYSTEM',
                  '-DAVIF_CODEC_LIBGAV1=OFF', '-DAVIF_CODEC_RAV1E=OFF', '-DAVIF_CODEC_SVT=OFF',
                  '-DAVIF_LIBYUV=OFF', '-DAVIF_LIBXML2=OFF', '-DAVIF_BUILD_APPS=OFF',
                  '-DAVIF_BUILD_TESTS=OFF'])
    digest = hashlib.sha256((HERE / 'sources.json').read_bytes()).hexdigest()
    (prefix / '.snow-native-manifest').write_text(digest + '\n' + target + '\n')
    print(f'Prepared native prefix: {prefix}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['fetch', 'build', 'verify', 'check-adapter'])
    parser.add_argument('--target', default=None)
    parser.add_argument('--sanitizers', choices=['', 'address', 'undefined', 'address,undefined'], default='')
    parser.add_argument('--instrumented-dependencies', action='store_true', help='check-adapter uses the separately instrumented dependency prefix')
    parser.add_argument('--jobs', type=int, default=min(os.cpu_count() or 2, 8))
    args = parser.parse_args()
    if args.jobs < 1 or args.jobs > 256:
        parser.error('--jobs must be 1..256')
    if args.command == 'build':
        build(args.target or host_target(), args.jobs, args.sanitizers)
    elif args.command == 'check-adapter':
        target = args.target or host_target()
        if target != host_target():
            parser.error('Adapter checks must run natively')
        variant = target + ('-' + args.sanitizers.replace(',', '-') if args.instrumented_dependencies and args.sanitizers else '')
        prefix = CACHE / variant / 'prefix'
        output = CACHE / variant / ('adapter-' + (args.sanitizers.replace(',', '-') or 'plain'))
        if not (prefix / '.snow-native-manifest').exists():
            parser.error('Native prefix missing; run build with the matching sanitizer arguments first')
        env = os.environ.copy()
        if args.sanitizers:
            env.setdefault('UBSAN_OPTIONS', 'halt_on_error=1:print_stacktrace=1')
        compiler = (['-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL',
                     '-DCMAKE_C_COMPILER=clang-cl', '-DCMAKE_CXX_COMPILER=clang-cl',
                     f'-DCMAKE_C_COMPILER_TARGET={target}', f'-DCMAKE_CXX_COMPILER_TARGET={target}']
                    if platform.system() == 'Windows' else ['-DCMAKE_OSX_DEPLOYMENT_TARGET=15.0'])
        run(['cmake', '-S', HERE / 'shim', '-B', output, '-G', 'Ninja', '-DCMAKE_BUILD_TYPE=RelWithDebInfo',
             f'-DSNOW_NATIVE_PREFIX={prefix}', '-DSNOW_BUILD_TESTS=ON', f'-DSNOW_SANITIZERS={args.sanitizers}', *compiler], env)
        run(['cmake', '--build', output, '--parallel', args.jobs], env)
        run(['ctest', '--test-dir', output, '-R', '^snow-native-lifecycle$', '--output-on-failure'], env)
    else:
        for name in MANIFEST['sources']:
            checked_archive(name, args.command == 'fetch')
            print(f'{name}: verified')


if __name__ == '__main__':
    main()
