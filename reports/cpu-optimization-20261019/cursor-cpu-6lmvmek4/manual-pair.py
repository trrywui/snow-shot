from pathlib import Path
import subprocess,sys,json,time
root=Path(__file__).resolve().parent
meta=json.loads((root/'metadata.json').read_text())
for label,app in [('clip-confirm',meta['candidate_app']),('baseline-confirm',meta['app'])]:
 subprocess.run([sys.executable,str(root/'manual-profile.py'),'0',label,'--app='+app,'--stop-after-capture'],check=True)
 record=json.loads((root/f'manual-{label}-cpu.json').read_text())
 if not record['capture_ends']:
  print('PAIR_ABORTED_NO_CAPTURE',label,flush=True)
  break
 time.sleep(2)
else:print('PAIR_DONE',flush=True)
