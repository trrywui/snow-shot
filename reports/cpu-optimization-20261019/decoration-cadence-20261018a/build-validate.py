from pathlib import Path
import hashlib
import json
import os
import subprocess
import sys

root = Path(__file__).resolve().parent
personal = root.parent.parent
repo = personal.parent.parent
metadata_path = root / 'metadata.json'
metadata = json.loads(metadata_path.read_text())
env = os.environ.copy()
env.update(CARGO_HOME=str(personal / 'cargo'), RUSTUP_HOME=str(personal / 'rustup'),
           RUSTUP_TOOLCHAIN='1.97.1')
env['PATH'] = os.pathsep.join([
    str(personal / 'rustup/toolchains/1.97.1-x86_64-apple-darwin/bin'),
    str(personal / 'host/venv/bin'), str(personal / 'host/bin'), env.get('PATH', '')])


def run_logged(name, command, extra_env=None):
    with (root / name).open('w') as log:
        result = subprocess.run(command, cwd=repo, env=extra_env or env,
                                stdout=log, stderr=subprocess.STDOUT)
    print(name, 'EXIT', result.returncode, flush=True)
    if result.returncode:
        print((root / name).read_text(errors='replace')[-6000:], flush=True)
        raise SystemExit(result.returncode)


subprocess.run([str(personal / 'host/venv/bin/clang-format'), '--dry-run', '--Werror',
                *[p for p in metadata['source_files'] if p.endswith(('.cpp', '.h'))]],
               cwd=repo, env=env, check=True)
subprocess.run(['git', 'diff', '--check'], cwd=repo, check=True)
run_logged('build.log', [str(personal / 'host/venv/bin/cmake'), '--build',
           str(personal / 'build'), '--target', 'snow_shot',
           'snow-shot-screenshot-color-picker-tests', '--parallel', '2'])
metadata['build_passed'] = True
ctest = str(personal / 'host/venv/bin/ctest')
run_logged('tests.log', [ctest, '--test-dir', str(personal / 'build'), '-R',
           '^snow-shot-screenshot-color-picker-tests$', '--output-on-failure'])
scaled_env = env.copy()
scaled_env['QT_SCALE_FACTOR'] = '2'
run_logged('tests-scale2.log', [ctest, '--test-dir', str(personal / 'build'), '-R',
           '^snow-shot-screenshot-color-picker-tests$', '--output-on-failure'], scaled_env)
run_logged('native-tests.log', [ctest, '--test-dir', str(personal / 'build'), '-R',
           '^snow-shot-macos-color-picker-animation-native-tests$', '--output-on-failure'])
subprocess.run([sys.executable, str(root / 'deploy-isolated.py')], env=env, cwd=repo, check=True)
metadata['candidate_binary_sha256'] = (root / 'binary-sha256.txt').read_text().split()[0]
metadata['validation'] = {'offscreen': 'passed', 'offscreen_scale2': 'passed',
                          'native_cache_and_lifecycle': 'passed', 'format': 'passed',
                          'diff_check': 'passed', 'full_suite': 'not run'}
metadata['source_patch_sha256'] = hashlib.sha256(
    subprocess.check_output(['git', 'diff', '9359bf30e'], cwd=repo)).hexdigest()
metadata['deployment_signature_verified'] = True
metadata_path.write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + '\n')
print('BUILD_VALIDATION_DEPLOYMENT_COMPLETE', flush=True)
