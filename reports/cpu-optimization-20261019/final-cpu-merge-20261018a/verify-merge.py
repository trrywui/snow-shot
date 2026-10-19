from pathlib import Path
import os
import subprocess

root = Path(__file__).resolve().parent
personal = root.parent.parent
repo = personal.parent.parent
env = os.environ.copy()
env.update(CARGO_HOME=str(personal / 'cargo'), RUSTUP_HOME=str(personal / 'rustup'),
           RUSTUP_TOOLCHAIN='1.97.1')
env['PATH'] = os.pathsep.join([
    str(personal / 'rustup/toolchains/1.97.1-x86_64-apple-darwin/bin'),
    str(personal / 'host/venv/bin'), str(personal / 'host/bin'), env.get('PATH', '')])
env.pop('SNOW_SHOT_CAPTURE_PRESENTATION_FPS', None)


def run_logged(name, command):
    with (root / name).open('w') as log:
        result = subprocess.run(command, cwd=repo, env=env, stdout=log, stderr=subprocess.STDOUT)
    print(name, 'EXIT', result.returncode, flush=True)
    if result.returncode:
        print((root / name).read_text(errors='replace')[-6000:], flush=True)
        raise SystemExit(result.returncode)


run_logged('build-tests.log', [str(personal / 'host/venv/bin/cmake'), '--build',
           str(personal / 'build'), '--target', 'snow-shot-selection-presentation-tests',
           '--parallel', '2'])
run_logged('tests.log', [str(personal / 'host/venv/bin/ctest'), '--test-dir',
           str(personal / 'build'), '-R', '^snow-shot-selection-presentation-tests$',
           '--output-on-failure'])
print((root / 'tests.log').read_text(), flush=True)
print('MERGE_TESTS_COMPLETE; no app build, installation or launch performed', flush=True)
