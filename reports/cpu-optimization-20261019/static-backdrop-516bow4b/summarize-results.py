from pathlib import Path
import json, statistics
r = Path(__file__).resolve().parent
rows = json.loads((r / "controlled-cpu-runs.json").read_text())
summary = []
for phase in ("move-selection", "resize-selection"):
    for mode in (0, 1):
        all_rows = [x for x in rows if x["phase"] == phase and x["mode"] == mode]
        valid = [x for x in all_rows if x["updates"] >= .98 * x["input_rate"] * x["seconds"]]
        item = {"phase": phase, "mode": mode, "total_runs": len(all_rows), "valid_runs": len(valid),
                "raw": [{k: x[k] for k in ("trial", "updates", "seconds", "app_cpu", "windowserver_cpu", "app_rss_mib")} for x in all_rows]}
        if valid:
            item["medians"] = {k: statistics.median(x[k] for x in valid) for k in
                               ("app_cpu", "windowserver_cpu", "app_rss_mib", "windowserver_rss_mib")}
            item["medians"]["combined_cpu"] = statistics.median(x["app_cpu"] + x["windowserver_cpu"] for x in valid)
        summary.append(item)
(r / "cpu-summary.json").write_text(json.dumps(summary, indent=2))
for item in summary:
    print(item["phase"], item["mode"], str(item["valid_runs"])+"/"+str(item["total_runs"]),
          {k: round(v, 2) for k,v in item.get("medians", {}).items()})
