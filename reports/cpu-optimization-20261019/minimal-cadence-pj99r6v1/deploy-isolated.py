from pathlib import Path
import os, subprocess, plistlib, hashlib
root = Path(__file__).resolve().parent
personal = root.parent.parent
repo = personal.parent.parent
local_env = os.environ.copy()
local_env.update(CARGO_HOME=str(personal/"cargo"), RUSTUP_HOME=str(personal/"rustup"), RUSTUP_TOOLCHAIN="1.97.1")
local_env["PATH"] = os.pathsep.join([str(personal/"rustup/toolchains/1.97.1-x86_64-apple-darwin/bin"),str(personal/"host/venv/bin"),str(personal/"host/bin"),local_env.get("PATH", "")])
with (root/"deployment.log").open("w") as log:
    subprocess.run([str(personal/"host/venv/bin/cmake"),"--install",str(personal/"build"),"--component","SnowShot","--prefix",str(root/"deploy")],env=local_env,stdout=log,stderr=subprocess.STDOUT,check=True)
app=root/"deploy/snow_shot.app"
info=app/"Contents/Info.plist"
data=plistlib.loads(info.read_bytes())
data.update(CFBundleIdentifier="io.github.trrywui.snowshot.dev.rendering",CFBundleName="Snow Shot Minimal 30 Hz Test",CFBundleDisplayName="Snow Shot Minimal 30 Hz Test")
info.write_bytes(plistlib.dumps(data))
subprocess.run(["codesign","--force","--sign","-","--identifier",data["CFBundleIdentifier"],"--preserve-metadata=entitlements,flags","--timestamp=none",str(app)],check=True)
subprocess.run(["codesign","--verify","--deep","--strict",str(app)],check=True)
exe=app/"Contents/MacOS/snow_shot"
(root/"binary-sha256.txt").write_text(hashlib.file_digest(exe.open("rb"),"sha256").hexdigest()+"  "+str(exe)+"\n")
print(app)
