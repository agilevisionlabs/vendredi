"""Measure a real local script command through delivery. Requires psutil."""
import argparse
import json
import statistics
import tempfile
import threading
import time
from pathlib import Path

import psutil
import local_instance as local


def measure(binary):
    local.BINARY = binary.resolve()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "vendredi.toml").write_text(
            (local.SAMPLE / "vendredi.toml").read_text()
            .replace(":8180", ":0")
            .replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"'))
        (root / "reply.js").write_text("""function reply(i,a) {
            a.set("runs", (a.get("runs") || 0) + 1);
            a.send(a.build("ORDRSP", "D96A", [["BGM", "231"]]));
        }""")
        # The partner's rule lives in its active case, `orders`.
        local.write_case(root, "edifact", "ORDERS", "D96A", '{ script = "reply.js", function = "reply" }')
        start = time.perf_counter()
        app = local.Instance(root)
        ready = time.perf_counter() - start
        proc = psutil.Process(app.proc.pid)
        stop = threading.Event()
        peak = proc.memory_info().rss

        def sample():
            nonlocal peak
            while not stop.wait(0.005):
                peak = max(peak, proc.memory_info().rss)

        sampler = threading.Thread(target=sample)
        sampler.start()
        elapsed = []
        try:
            for _ in range(21):
                start = time.perf_counter()
                app.command("send", "Supplier", "reply.js", "reply")
                app.wait("delivered")
                elapsed.append(1000 * (time.perf_counter() - start))
            stop.set()
            sampler.join()
            proc.cpu_percent()
            time.sleep(2)
            idle_cpu = proc.cpu_percent()
            idle_rss = proc.memory_info().rss
        finally:
            stop.set()
            sampler.join()
            app.close()
    return dict(binary=str(binary), bytes=binary.stat().st_size, ready_ms=round(ready*1000, 1),
                first_script_ms=round(elapsed[0], 1), warm_median_ms=round(statistics.median(elapsed[1:]), 1),
                sampled_peak_rss=peak, idle_rss=idle_rss, idle_cpu_percent=idle_cpu)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path, nargs="+")
    for binary in parser.parse_args().binary:
        print(json.dumps(measure(binary)), flush=True)
