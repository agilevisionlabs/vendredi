"""Measure the release binary in a fresh workspace and confirm a real exchange in both modes."""
import argparse
import http.client
import json
import os
from pathlib import Path
import platform
import plistlib
import signal
import socket
import subprocess
import tarfile
import tempfile
import time


def usage(pid):
    text = subprocess.check_output(["ps", "-p", str(pid), "-o", "rss=", "-o", "time="], text=True)
    rss, total = text.split()
    cpu = 0
    for field in total.split(":"):
        cpu = 60 * cpu + float(field)
    return int(rss) * 1024, cpu


def init_workspace(binary, root):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    subprocess.run([binary, "init", root], check=True)
    config = root / "vendredi.toml"
    config.write_text(config.read_text().replace(":8180", f":{port}"))
    return port


def sample_exchange(root, port):
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


def verify_finder_launch(app, binary, helper, home):
    workspace = home / "Vendredi"
    port = init_workspace(binary, workspace)
    # Launch the extracted app with no arguments and a disposable home directory.
    pid = int(subprocess.check_output([helper, "launch", app, home], timeout=30))
    try:
        subprocess.run([helper, str(pid)], check=True, timeout=12)
        tty = subprocess.check_output(["ps", "-p", str(pid), "-o", "tty="], text=True)
        assert tty.strip() == "??", f"Finder launch has a controlling terminal: {tty}"
        sample_exchange(workspace, port)
    finally:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass


def profile(binary, helper, root, gui):
    port = init_workspace(binary, root)
    args = ["gui", root] if gui else ["run", "--headless", root]
    start = time.monotonic()
    with subprocess.Popen([binary, *args], stdout=subprocess.PIPE, stdin=subprocess.PIPE, text=True) as proc:
        try:
            if gui:
                subprocess.run([helper, str(proc.pid)], check=True, timeout=12)
            else:
                assert json.loads(proc.stdout.readline())["event"] == "ready"
            ready = time.monotonic() - start
            sample_exchange(root, port)
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
    parser.add_argument("release", type=Path, help="macOS release archive or standalone executable")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        helper = root / "window"
        subprocess.run(["swiftc", Path(__file__).with_name("macos_window.swift"), "-o", helper], check=True)
        binary = args.release.resolve()
        if binary.name.endswith(".tar.gz"):
            with tarfile.open(binary) as archive:
                assert all(name.split("/")[0] == "Vendredi.app" for name in archive.getnames())
                archive.extractall(root, filter="data")
            app = root / "Vendredi.app"
            subprocess.run(["codesign", "--verify", "--strict", app], check=True)
            contents = app / "Contents"
            info = plistlib.loads((contents / "Info.plist").read_bytes())
            assert info["CFBundlePackageType"] == "APPL"
            icon = contents / "Resources" / info["CFBundleIconFile"]
            assert icon.read_bytes()[:4] == b"icns"
            binary = contents / "MacOS" / info["CFBundleExecutable"]
            verify_finder_launch(app, binary, helper, root / "Finder Home")
            print("Verified Finder launch, visible window, detached terminal and sample exchange.")
        info = dict(binary=str(binary), bytes=binary.stat().st_size, platform=platform.platform())
        info["measurements"] = [profile(binary, helper, root / str(gui), gui) for gui in [False, True]]
        print(json.dumps(info, indent=2))
