from pathlib import Path
import json,statistics
root=Path(__file__).resolve().parent
rows=json.loads((root/'controlled-cpu-runs.json').read_text())
summary=[]
for phase in ['move-selection','resize-selection','settle']:
 for mode in [0,1]:
  selected=[r for r in rows if r['mode']==mode and r['phase']==phase]
  valid=[r for r in selected if phase=='settle' or r['updates']>=285]
  if not valid: continue
  item={'mode':mode,'phase':phase,'valid':len(valid),'total':len(selected),'app_cpu_median':statistics.median(r['app_cpu'] for r in valid),'app_cpu_values':[r['app_cpu'] for r in valid],'ws_cpu_median':statistics.median(r['windowserver_cpu'] for r in valid),'rss_mib_median':statistics.median(r['app_rss_mib'] for r in valid),'updates':[r['updates'] for r in selected]}
  summary.append(item)
  print(json.dumps(item))
(root/'controlled-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
