from pathlib import Path
import subprocess, sys, time, json, datetime
root = Path(__file__).resolve().parent
pid = int(sys.argv[1])
server = subprocess.check_output(["pgrep", "-x", "WindowServer"], text=True).splitlines()[0]
print("READY: waiting for development screenshot", pid, flush=True)
deadline = time.monotonic() + 180
while time.monotonic() < deadline:
    captured = False
    for path in (root / "user-data/logs").glob("**/snow-shot-*.log"):
        for line in path.read_text().splitlines():
            try:
                event = json.loads(line)
            except (ValueError, TypeError):
                continue
            if event.get("pid") == pid and event.get("event") == "capture.finished" and event.get("fields", {}).get("outcome") == "succeeded":
                captured = True
                break
        if captured:
            break
    if captured:
        break
    time.sleep(0.2)
else:
    raise SystemExit("No development screenshot was detected within 180 seconds")
print("CAPTURE DETECTED", datetime.datetime.now().isoformat(), flush=True)
with (root / "interaction-bgra-cpu.txt").open("w") as cpu, (root / "interaction-bgra-collector.log").open("w") as log:
    sample = subprocess.Popen(["sample", str(pid), "60", "2", "-file", str(root / "interaction-bgra.sample.txt")], stdout=log, stderr=log)
    top = subprocess.Popen(["top", "-l", "55", "-s", "1", "-pid", str(pid), "-pid", server, "-stats", "pid,command,cpu,time,threads,state"], stdout=cpu, stderr=log)
    codes = [sample.wait(), top.wait()]
print("DONE", codes, flush=True)
