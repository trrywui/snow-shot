from pathlib import Path
import json,math,statistics,re
root=Path(__file__).resolve().parent
excluded=json.loads((root/'metadata.json').read_text()).get('excluded_manual_runs',{})
out=[];summary=[]
for path in sorted(root.glob('manual-*-cpu.json')):
 d=json.loads(path.read_text());label=d['label']
 if label in excluded:continue
 p=[json.loads(x) for x in (root/f'manual-{label}-pointer.jsonl').read_text().splitlines()]
 if d.get('paint_trace'):
  clocks=json.loads((root/f'manual-{label}-clock-check.json').read_text())
  if abs(clocks['start_offset']-clocks['end_offset'])>.01:raise RuntimeError('Clock domains shifted during capture; do not align paints')
 paints=[(float(m[1])-d.get('steady_minus_monotonic',0),int(m[2])) for m in re.finditer(r'^\s*([0-9]+\.[0-9]+) snow_shot\.capture\.render\.canvas paint_ns=(\d+)',(root/f'manual-{label}-app.log').read_text(),re.M)]
 firstpress=next((x[0] for x in p if x[4]),float('inf'))
 for row in d['rows']:
  s=[x for x in p if row['start']<=x[0]<=row['end']]
  if len(s)<15 or any(not x[1] for x in s):continue
  distance=sum(math.hypot(b[2]-a[2],b[3]-a[3]) for a,b in zip(s,s[1:]))
  pressed=sum(x[4] for x in s)/len(s)
  phase='drag' if pressed>.95 and distance>5 else 'hover' if pressed==0 and row['end']<firstpress and distance>5 else 'idle' if pressed==0 and distance<1 else 'transition'
  frames=[x for x in paints if row['start']<=x[0]<row['end']]
  row.update(phase=phase,pointer_distance=distance,pressed=pressed,label=label,canvas_paints=len(frames),canvas_paints_per_second=len(frames)/(row['end']-row['start']));out.append(row)
 for phase in ['hover','drag','idle']:
  a=[r for r in out if r['label']==label and r['phase']==phase]
  if not a:continue
  item={'label':label,'phase':phase,'samples':len(a),'cpu_median':statistics.median(r['app_cpu'] for r in a),'cpu_range':[f(r['app_cpu'] for r in a) for f in [min,max]],'ws_median':statistics.median(r['windowserver_cpu'] for r in a),'pointer_distance_median':statistics.median(r['pointer_distance'] for r in a)}
  if paints:item['canvas_paints_per_second_median']=statistics.median(r['canvas_paints_per_second'] for r in a)
  summary.append(item);print(json.dumps(item))
(root/'comparison-rows.json').write_text(json.dumps(out,indent=2))
(root/'comparison-summary.json').write_text(json.dumps(summary,indent=2))
