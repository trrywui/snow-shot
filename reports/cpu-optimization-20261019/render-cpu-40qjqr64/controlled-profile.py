from pathlib import Path
import os,json,subprocess,time,math,importlib.util,sys
root=Path(__file__).resolve().parent
app=Path(sys.argv[1]).resolve()
spec=importlib.util.spec_from_file_location('private_mcp',root/'mcp-client.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
windowserver=int(subprocess.check_output(['pgrep','-x','WindowServer'],text=True).splitlines()[0])
processes={};clients={};results=[]
trace = '--trace' in sys.argv
input_rate = 30

def cpu_times(pid):
    values={}
    for line in subprocess.check_output(['ps','-p',f'{pid},{windowserver}','-o','pid=,time=,rss='],text=True).splitlines():
        ident,clock,rss=line.split();parts=[float(x) for x in clock.split(':')];seconds=0
        for part in parts:seconds=seconds*60+part
        values[int(ident)]={'seconds':seconds,'rss_kib':int(rss)}
    return values

def record(mode,name,job):
    pid=processes[mode].pid;before=cpu_times(pid);start=time.monotonic();updates=job();elapsed=time.monotonic()-start;after=cpu_times(pid)
    item={'experiment':'canvas-opaque-paint','input_rate':input_rate,'trial':trial,'mode':mode,'phase':name,'seconds':elapsed,'updates':updates,'app_rss_mib':after[pid]['rss_kib']/1024,'app_cpu':100*(after[pid]['seconds']-before[pid]['seconds'])/elapsed,'windowserver_cpu':100*(after[windowserver]['seconds']-before[windowserver]['seconds'])/elapsed}
    results.append(item);(root/('controlled-trace-runs.json' if trace else 'controlled-cpu-runs.json')).write_text(json.dumps(results,indent=2));print(json.dumps(item),flush=True)

def stationary(seconds):time.sleep(seconds);return 0

def geometry(client,phase,duration):
    start=time.monotonic();count=0
    while (elapsed:=time.monotonic()-start)<duration:
        fraction=elapsed/duration
        if phase=='move':rect=[round(400+200*math.sin(fraction*4*math.pi)),round(300+120*math.sin(fraction*6*math.pi)),500,350]
        else:rect=[300,220,round(400+500*fraction),round(250+350*fraction)]
        client.request('snow_shot_screenshot_set_selection',{'bounds':rect})
        count+=1;target=(math.floor((time.monotonic()-start)*input_rate)+1)/input_rate
        time.sleep(max(0,start+target-time.monotonic()))
    return count


def stop_process(mode):
    client=clients.pop(mode,None)
    if client is not None:
        try:
            client.request('snow_shot_screenshot_cancel');client.session='';client.revision=None
            time.sleep(.25)
            client.request('snow_shot_app_action',{'action':'quit'})
        except Exception:pass
        finally:client.socket.close()
    process=processes.pop(mode,None)
    if process is not None and process.poll() is None:
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.wait()

try:
    order=[0,1] if trace else [0,1,1,0,0,1]
    for trial,mode in enumerate(order):
        data=root/f'controlled-data-{mode}';data.mkdir(mode=0o700,exist_ok=True)
        config=json.loads((root/'baseline-data/config.json').read_text());config.setdefault('mcp',{})['enabled']=True
        (data/'config.json').write_text(json.dumps(config));(data/'config.json').chmod(0o600)
        endpoint=root/f'controlled-endpoint-{mode}';endpoint.mkdir(mode=0o700,exist_ok=True)
        descriptor=endpoint/'server.json'
        if descriptor.exists():descriptor.unlink()
        env=os.environ.copy();env.update(SNOW_SHOT_PERSONAL_DATA_DIR=str(data),SNOW_SHOT_MCP_DESCRIPTOR=str(descriptor),SNOW_SHOT_CAPTURE_OPAQUE=str(mode),QT_QPA_PLATFORM='cocoa',LC_CTYPE='en_US.UTF-8')
        env['QT_LOGGING_RULES']='qt.qpa.backingstore.debug=true;qt.qpa.backingstore.info=true;snow_shot.capture.render.*.info=true' if trace else ''
        log=root/f'controlled-{mode}-{"trace" if trace else "cpu-"+str(trial)}.log'
        with log.open('w') as stream:
            process=subprocess.Popen([str(app/'Contents/MacOS/snow_shot'),f'--e2e-instance-id=render-controlled-{mode}','--show-main-window'],env=env,stdout=stream,stderr=subprocess.STDOUT,start_new_session=True)
        processes[mode]=process
        deadline=time.monotonic()+30
        while not descriptor.exists() and time.monotonic()<deadline:
            if process.poll() is not None:raise RuntimeError('Development process exited during startup')
            time.sleep(.1)
        client=m.Client(descriptor,process.pid);clients[mode]=client
        print('READY',mode,process.pid,'trial',trial,flush=True)
        for capture_attempt in range(2):
            try:
                client.request('snow_shot_screenshot_begin',{'presentation':'visible','target':'current_monitor','smart_selection':False,'capture_cursor':False})
                break
            except RuntimeError as error:
                if str(error) != 'capture_unavailable' or capture_attempt:raise
                print('DISCARDED acquisition timeout before measurement',mode,trial,flush=True)
                client.request('snow_shot_screenshot_cancel');client.session='';client.revision=None
                time.sleep(2)
        client.request('snow_shot_screenshot_set_selection',{'bounds':[400,300,500,350]})
        record(mode,'stationary',lambda:stationary(3))
        record(mode,'move-selection',lambda:geometry(client,'move',5 if trace else 10))
        record(mode,'resize-selection',lambda:geometry(client,'resize',5 if trace else 10))
        record(mode,'settle',lambda:stationary(2))
        stop_process(mode)
        time.sleep(3)
finally:
    for mode in list(processes):stop_process(mode)
