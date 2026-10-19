from pathlib import Path
import os,json,subprocess,time,threading,sys,importlib.util
root=Path(__file__).resolve().parent
def clock_offset():
 return float(subprocess.check_output([str(root/'check-clock')],text=True).split('difference_seconds=')[1])
steady_minus_monotonic=clock_offset()
mode=int(sys.argv[1]); label=sys.argv[2] if len(sys.argv)>2 else str(mode)
threadpool_off='--no-gui-threadpool' in sys.argv
fps=next((int(arg.split('=',1)[1]) for arg in sys.argv if arg.startswith('--fps=')),0)
if fps not in (0,30,45,60): raise ValueError('Unsupported presentation cadence')
paint_trace='--paint-trace' in sys.argv
app_path=next((arg.split('=',1)[1] for arg in sys.argv if arg.startswith('--app=')),json.loads((root/'metadata.json').read_text())['app'])
app=Path(app_path)/'Contents/MacOS/snow_shot'
data=root/('manual-data-'+label);data.mkdir(mode=0o700,exist_ok=True)
config=json.loads((root/'baseline-data/config.json').read_text())
config.setdefault('mcp',{})['enabled']=True
if mode: config.setdefault('screenshot_ui',{})['color_picker_display_mode']='always_hide'
(data/'config.json').write_text(json.dumps(config));(data/'config.json').chmod(0o600)
endpoint=root/('manual-endpoint-'+label);endpoint.mkdir(mode=0o700,exist_ok=True)
descriptor=endpoint/'server.json'
if descriptor.exists(): descriptor.unlink()
env=os.environ.copy();env.pop('QT_NO_GUI_THREADPOOL',None)
env.pop('SNOW_SHOT_CAPTURE_PRESENTATION_FPS',None)
if fps:env['SNOW_SHOT_CAPTURE_PRESENTATION_FPS']=str(fps)
if threadpool_off:env['QT_NO_GUI_THREADPOOL']='1'
env.update(SNOW_SHOT_PERSONAL_DATA_DIR=str(data),SNOW_SHOT_STATIC_BACKDROP='0',SNOW_SHOT_MCP_DESCRIPTOR=str(descriptor),QT_LOGGING_RULES='',LC_ALL='en_US.UTF-8')
if paint_trace:
 env['QT_LOGGING_RULES']='snow_shot.capture.render.canvas.info=true'
 env['QT_MESSAGE_PATTERN']='%{time boot} %{category} %{message}'
with (root/f'manual-{label}-app.log').open('w') as log:
 process=subprocess.Popen([str(app),'--show-main-window',f'--e2e-instance-id=cursor-manual-{label}'],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(root/f'manual-{label}-pid.txt').write_text(str(process.pid))
ws=int(subprocess.check_output(['pgrep','-x','WindowServer'],text=True).splitlines()[0])
probe=subprocess.Popen([str(root/'pointer-presence'),str(process.pid)],stdout=subprocess.PIPE,text=True)
starts=[];ends=[];state={'count':0,'first':None};pointer=[]
def observe():
 previous=False
 with (root/f'manual-{label}-pointer.jsonl').open('w') as log:
  for line in probe.stdout:
   record=json.loads(line);now,count=record[:2];active=count>0
   state['count']=count
   if active or previous:
    log.write(line);log.flush();pointer.append(record)
   if active and not previous:
    starts.append(now);state['first']=now if state['first'] is None else state['first']
    print('CAPTURE_START',label,len(starts),flush=True)
   elif previous and not active:
    ends.append(now);print('CAPTURE_END',label,len(ends),flush=True)
   previous=active
thread=threading.Thread(target=observe,daemon=True);thread.start()
def snapshot():
 result={}
 for line in subprocess.check_output(['ps','-p',f'{process.pid},{ws}','-o','pid=,time=,rss='],text=True).splitlines():
  pid,clock,rss=line.split();seconds=0
  for part in clock.split(':'): seconds=seconds*60+float(part)
  result[int(pid)]={'cpu_seconds':seconds,'rss_mib':int(rss)/1024}
 return result
print('READY_MANUAL',label,process.pid,flush=True)
rows=[];begin=time.monotonic();previous=snapshot();stamp=time.monotonic()
try:
 while process.poll() is None and time.monotonic()-begin<420:
  time.sleep(1);current=snapshot();now=time.monotonic()
  if process.pid not in current: break
  rows.append({'start':stamp,'end':now,'elapsed':now-begin,'capture_count':state['count'],'capture_elapsed':None if state['first'] is None else now-state['first'],'app_cpu':100*(current[process.pid]['cpu_seconds']-previous[process.pid]['cpu_seconds'])/(now-stamp),'windowserver_cpu':100*(current[ws]['cpu_seconds']-previous[ws]['cpu_seconds'])/(now-stamp),'app_rss_mib':current[process.pid]['rss_mib']})
  (root/f'manual-{label}-cpu.json').write_text(json.dumps({'mode':mode,'label':label,'threadpool_disabled':threadpool_off,'presentation_fps_limit':fps,'paint_trace':paint_trace,'steady_minus_monotonic':steady_minus_monotonic,'app':str(app),'rows':rows,'capture_starts':starts,'capture_ends':ends},indent=2))
  previous=current;stamp=now
  if (root/f'manual-{label}.stop').exists(): break
  if '--stop-after-capture' in sys.argv and ends and now-ends[-1]>3: break
finally:
 if probe.poll() is None: probe.terminate();probe.wait(timeout=5)
 if process.poll() is None and descriptor.exists():
  try:
   spec=importlib.util.spec_from_file_location('private_mcp',root/'mcp-client.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
   client=module.Client(descriptor,process.pid)
   try:client.request('snow_shot_app_action',{'action':'quit'})
   finally:client.socket.close()
   process.wait(timeout=5)
  except Exception:pass
 if process.poll() is None:
  process.terminate()
  try:process.wait(timeout=5)
  except subprocess.TimeoutExpired:process.kill();process.wait()
(root/f'manual-{label}-clock-check.json').write_text(json.dumps({'start_offset':steady_minus_monotonic,'end_offset':clock_offset()}))
print('MANUAL_DONE',label,'samples',len(rows),flush=True)
