from pathlib import Path
import json
import subprocess
import sys
import time

root = Path(__file__).resolve().parent
metadata = json.loads((root / 'metadata.json').read_text())
for label, app in [('cache30', metadata['app']), ('full30', metadata['reference_app'])]:
    if list(root.glob('manual-' + label + '*')):
        raise SystemExit('Run label already used: ' + label)
    with (root / ('manual-' + label + '-console.log')).open('w') as log:
        result = subprocess.run([sys.executable, str(root / 'manual-profile.py'), '0', label,
                                 '--fps=30', '--stop-after-capture', '--app=' + app],
                                stdout=log, stderr=subprocess.STDOUT)
    path = root / ('manual-' + label + '-cpu.json')
    data = json.loads(path.read_text()) if path.exists() else {}
    if result.returncode or len(data.get('capture_starts', [])) != 1 or len(data.get('capture_ends', [])) != 1:
        raise SystemExit('Capture incomplete; pair stopped at ' + label)
    print('ROUND_FINISHED', label, flush=True)
    if label == 'cache30':
        time.sleep(2)
print('PAIR_FINISHED', flush=True)
