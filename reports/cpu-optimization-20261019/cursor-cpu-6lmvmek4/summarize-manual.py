from pathlib import Path
import json,math,statistics
root=Path(__file__).resolve().parent
out=[];summary=[]
for path in sorted(root.glob('manual-*-cpu.json')):
 d=json.loads(path.read_text());label=d['label'];p=[json.loads(x) for x in (root/f'manual-{label}-pointer.jsonl').read_text().splitlines()]
 firstpress=next((x[0] for x in p if x[4]),float('inf'))
 for row in d['rows']:
  s=[x for x in p if row['start']<=x[0]<=row['end']]
  if len(s)<15 or any(not x[1] for x in s):continue
  distance=sum(math.hypot(b[2]-a[2],b[3]-a[3]) for a,b in zip(s,s[1:]))
  pressed=sum(x[4] for x in s)/len(s)
  phase='drag' if pressed>.95 and distance>5 else 'hover' if pressed==0 and row['end']<firstpress and distance>5 else 'idle' if pressed==0 and distance<1 else 'transition'
  row.update(phase=phase,pointer_distance=distance,pressed=pressed,label=label);out.append(row)
 for phase in ['hover','drag','idle']:
  a=[r for r in out if r['label']==label and r['phase']==phase]
  if not a:continue
  item={'label':label,'phase':phase,'samples':len(a),'cpu_median':statistics.median(r['app_cpu'] for r in a),'cpu_range':[f(r['app_cpu'] for r in a) for f in [min,max]],'ws_median':statistics.median(r['windowserver_cpu'] for r in a),'pointer_distance_median':statistics.median(r['pointer_distance'] for r in a)}
  summary.append(item);print(json.dumps(item))
(root/'comparison-rows.json').write_text(json.dumps(out,indent=2))
(root/'comparison-summary.json').write_text(json.dumps(summary,indent=2))
