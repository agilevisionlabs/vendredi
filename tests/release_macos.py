"""Measure the release binary in a fresh workspace and confirm a real exchange in both modes."""
import argparse
import http.client
import json
from pathlib import Path
import platform
import socket
import subprocess
import tempfile
import time


def usage(pid):
    text = subprocess.check_output(["ps", "-p", str(pid), "-o", "rss=", "-o", "time="], text=True)
    rss, total = text.split()
    cpu = 0
    for field in total.split(":"):
        cpu = 60 * cpu + float(field)
    return int(rss) * 1024, cpu


def profile(binary, helper, root, gui):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    subprocess.run([binary, "init", root], check=True)
    config = root / "vendredi.toml"
    config.write_text(config.read_text().replace(":8180", f":{port}"))
    args = ["gui", root] if gui else ["run", "--headless", root]
    start = time.monotonic()
    with subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stdin=subprocess.PIPE, text=True) as proc:
        try:
            if gui:
                subprocess.run([helper, str(proc.pid)], check=True, timeout=12)
            else:
                assert json.loads(proc.stdout.readline())["event"] == "ready"
            ready = time.monotonic() - start
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=4)
            try:
                conn.request("POST", "/supplier/orders", (root / "order.edi").read_bytes())
                response = conn.getresponse()
                assert response.status == 202 and response.read() == b""
            finally:
                conn.close()
            deadline = time.monotonic() + 5
            while len(list((root / "replies").glob("*.edi"))) != 2:
                if time.monotonic() > deadline:
                    raise AssertionError("sample replies missing")
                time.sleep(0.01)
            time.sleep(1)
            rss, cpu = usage(proc.pid)
            begin = time.monotonic()
            time.sleep(5)
            end_rss, end_cpu = usage(proc.pid)
            return dict(mode="gui" if gui else "headless", ready_ms=round(1000 * ready),
                        rss_bytes=max(rss, end_rss), idle_cpu_percent=round(100*(end_cpu-cpu)/(time.monotonic()-begin), 2))
        finally:
            if proc.poll() is None:
                if gui:
                    proc.terminate()
                else:
                    proc.stdin.write("quit\n")
                    proc.stdin.flush()
                proc.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        helper = root / "window"
        subprocess.run(["swiftc", Path(__file__).with_name("macos_window.swift"), "-o", helper], check=True)
        info = dict(binary=str(binary), bytes=binary.stat().st_size, platform=platform.platform())
        info["measurements"] = [profile(binary, helper, root / str(gui), gui) for gui in [False, True]]
        print(json.dumps(info, indent=2))
