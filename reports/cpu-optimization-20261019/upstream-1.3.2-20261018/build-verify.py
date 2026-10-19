from pathlib import Path
import os
import subprocess

logs = Path(__file__).resolve().parent
personal = logs.parent.parent
repo = personal.parent.parent
env = os.environ.copy()
env.pop('SNOW_SHOT_CAPTURE_PRESENTATION_FPS', None)


def run(name, command):
    print('START', name, flush=True)
    with (logs / name).open('w') as output:
        result = subprocess.run(command, cwd=repo, env=env, stdout=output,
                                stderr=subprocess.STDOUT)
    print('EXIT', result.returncode, name, flush=True)
    if result.returncode:
        print((logs / name).read_text(errors='replace')[-8000:], flush=True)
        raise SystemExit(result.returncode)


run('build-app.log', ['bash', 'scripts/macos14-personal.sh', 'build-target', 'snow_shot'])
run('build-tests.log', ['bash', 'scripts/macos14-personal.sh', 'build-target',
                        'snow-shot-selection-presentation-tests',
                        'snow-shot-application-storage-tests'])
run('tests.log', [str(personal / 'host/venv/bin/ctest'), '--test-dir',
                  str(personal / 'build'), '-R',
                  '^snow-shot-(selection-presentation|local-shortcut-migration)-tests$',
                  '--output-on-failure'])
print((logs / 'tests.log').read_text(), flush=True)
print('BUILD_AND_FOCUSED_TESTS_COMPLETE; no installation performed', flush=True)
