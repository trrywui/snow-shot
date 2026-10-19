from pathlib import Path
import os, json, subprocess, time, threading, sys, signal, importlib.util
root = Path(__file__).resolve().parent
mode = int(sys.argv[1])
app = root/"deploy/snow_shot.app/Contents/MacOS/snow_shot"
data = root/("manual-data-"+str(mode)); data.mkdir(mode=0o700, exist_ok=True)
config = json.loads((root/"baseline-data/config.json").read_text())
config.setdefault("mcp", {})["enabled"] = True
endpoint=root/("manual-endpoint-"+str(mode)); endpoint.mkdir(mode=0o700,exist_ok=True)
descriptor=endpoint/"server.json"
if descriptor.exists(): descriptor.unlink()
(data/"config.json").write_text(json.dumps(config)); (data/"config.json").chmod(0o600)
env = os.environ.copy(); env.update(SNOW_SHOT_PERSONAL_DATA_DIR=str(data), SNOW_SHOT_STATIC_BACKDROP=str(mode), SNOW_SHOT_MCP_DESCRIPTOR=str(descriptor), QT_LOGGING_RULES="", LC_CTYPE="en_US.UTF-8")
with (root/("manual-"+str(mode)+"-app.log")).open("w") as log:
    process = subprocess.Popen([str(app),"--show-main-window","--e2e-instance-id=backdrop-manual-"+str(mode)],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(root/("manual-"+str(mode)+"-pid.txt")).write_text(str(process.pid))
ws = int(subprocess.check_output(["pgrep","-x","WindowServer"],text=True).splitlines()[0])
probe = subprocess.Popen([str(root/"capture-presence"),str(process.pid)],stdout=subprocess.PIPE,text=True)
starts=[]; ends=[]; state={"count":0,"first":None,"ended":None}
def observe():
    previous = False
    for line in probe.stdout:
        now=time.monotonic(); count=int(line); active=count>0
        state["count"] = count
        if active and not previous:
            starts.append(now); state["first"] = now if state["first"] is None else state["first"]; state["ended"]=None
            print("CAPTURE_START",mode,len(starts),flush=True)
        elif previous and not active:
            ends.append(now); state["ended"]=now
            print("CAPTURE_END",mode,len(ends),flush=True)
        previous=active
thread=threading.Thread(target=observe,daemon=True);thread.start()
def snapshot():
    result={}
    for line in subprocess.check_output(["ps","-p",str(process.pid)+","+str(ws),"-o","pid=,time=,rss="],text=True).splitlines():
        pid,clock,rss=line.split(); seconds=0
        for part in clock.split(":"): seconds=seconds*60+float(part)
        result[int(pid)]={"cpu_seconds":seconds,"rss_mib":int(rss)/1024}
    return result
print("READY_MANUAL",mode,process.pid,flush=True)
rows=[]; begin=time.monotonic(); previous=snapshot(); stamp=time.monotonic()
try:
    while process.poll() is None and time.monotonic()-begin < 300:
        time.sleep(1); current=snapshot(); now=time.monotonic()
        if process.pid not in current: break
        row={"elapsed":now-begin,"capture_count":state["count"],"capture_elapsed":None if state["first"] is None else now-state["first"],"app_cpu":100*(current[process.pid]["cpu_seconds"]-previous[process.pid]["cpu_seconds"])/(now-stamp),"windowserver_cpu":100*(current[ws]["cpu_seconds"]-previous[ws]["cpu_seconds"])/(now-stamp),"app_rss_mib":current[process.pid]["rss_mib"]}
        rows.append(row); (root/("manual-"+str(mode)+"-cpu.json")).write_text(json.dumps({"mode":mode,"rows":rows,"capture_starts":starts,"capture_ends":ends},indent=2))
        previous=current;stamp=now
        if (root/("manual-"+str(mode)+".stop")).exists(): break

finally:
    if probe.poll() is None: probe.terminate(); probe.wait(timeout=5)
    if process.poll() is None and descriptor.exists():
        try:
            spec=importlib.util.spec_from_file_location("manual_mcp",root/"mcp-client.py")
            module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
            client=module.Client(descriptor,process.pid)
            try: client.request("snow_shot_app_action",{"action":"quit"})
            finally: client.socket.close()
            process.wait(timeout=5)
        except Exception: pass
    if process.poll() is None:
        process.terminate()
        try: process.wait(timeout=5)
        except subprocess.TimeoutExpired: process.kill();process.wait()
print("MANUAL_DONE",mode,"samples",len(rows),flush=True)
