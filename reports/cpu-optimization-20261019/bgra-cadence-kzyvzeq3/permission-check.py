from pathlib import Path
import importlib.util,json,os,subprocess,sys,time
root=Path(__file__).resolve().parent
mode=sys.argv[1]
metadata=json.loads((root/'metadata.json').read_text())
app=Path(metadata['reference_app'] if mode=='full' else metadata['app'])
data=root/('permission-data-'+mode);data.mkdir(mode=0o700,exist_ok=True)
config=json.loads((root/'baseline-data/config.json').read_text());config.setdefault('mcp',{})['enabled']=True
(data/'config.json').write_text(json.dumps(config));(data/'config.json').chmod(0o600)
endpoint=root/('permission-endpoint-'+mode);endpoint.mkdir(mode=0o700,exist_ok=True)
descriptor=endpoint/'server.json';descriptor.unlink(missing_ok=True)
env=os.environ.copy();env.pop('QT_NO_GUI_THREADPOOL',None)
env.update(SNOW_SHOT_PERSONAL_DATA_DIR=str(data),SNOW_SHOT_MCP_DESCRIPTOR=str(descriptor),SNOW_SHOT_CAPTURE_PRESENTATION_FPS='30',QT_LOGGING_RULES='')
with (root/('permission-'+mode+'-app.log')).open('w') as log:
 process=subprocess.Popen([str(app/'Contents/MacOS/snow_shot'),'--show-main-window','--e2e-instance-id=permission-'+mode],env=env,stdout=log,stderr=subprocess.STDOUT)
(root/('permission-'+mode+'-pid.txt')).write_text(str(process.pid))
print('PERMISSION_APP',mode,process.pid,flush=True)
try:
 process.wait(timeout=900)
except subprocess.TimeoutExpired:
 process.terminate();process.wait(timeout=5)
