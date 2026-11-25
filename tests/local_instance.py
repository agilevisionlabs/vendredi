"""Run real HTTP, watched-folder, CLI and recovery checks against a local Vendredi binary."""
import argparse
import contextlib
import http.client
import http.server
import json
import os
from pathlib import Path
import queue
import shutil
import socket
import sqlite3
import subprocess
import tarfile
import zipfile
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/debug/vendredi"
SAMPLE = ROOT / "examples/orders"


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Instance:
    def __init__(self, workspace):
        self.proc = subprocess.Popen([BINARY, "run", "--headless", workspace],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True)
        self.events = queue.Queue()
        self.seen = []
        def read():
            for line in self.proc.stdout:
                self.events.put(json.loads(line))
            self.events.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        try:
            self.wait("ready")
        except BaseException:
            self.close()
            raise

    def wait(self, kind):
        while True:
            event = self.events.get(timeout=10)
            if event is None:
                raise AssertionError(self.proc.stderr.read())
            self.seen.append(event)
            if event["event"] == "error":
                raise AssertionError(event)
            if event["event"] == kind:
                return event

    def command(self, *args):
        self.proc.stdin.write(json.dumps(args) + "\n")
        self.proc.stdin.flush()

    def close(self, crash=False):
        if self.proc.poll() is None:
            if crash:
                self.proc.kill()
            else:
                self.proc.stdin.write("quit\n")
                self.proc.stdin.flush()
            self.proc.wait(timeout=10)
        self.reader.join(timeout=2)
        for pipe in (self.proc.stdin, self.proc.stdout, self.proc.stderr):
            pipe.close()


class LocalExchange(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.listener = port()
        self.config = (SAMPLE / "vendredi.toml").read_text().replace(":8180", f":{self.listener}")
        self.order = (SAMPLE / "order.edi").read_bytes()
        shutil.copy(SAMPLE / "confirmation.edi", self.root)
        self.db = self.root / ".vendredi/vendredi.db"

    def launch(self):
        (self.root / "vendredi.toml").write_text(self.config)
        app = Instance(self.root)
        self.addCleanup(app.close)
        return app

    def post(self, data):
        conn = http.client.HTTPConnection("127.0.0.1", self.listener, timeout=5)
        try:
            conn.request("POST", "/supplier/orders", data, {"Content-Type": "application/edifact"})
            r = conn.getresponse()
            return r.status, r.read()
        finally:
            conn.close()

    def test_http_delivery_failure_restart_and_resend(self):
        replies = queue.Queue()
        class Partner(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                replies.put(self.rfile.read(int(self.headers["Content-Length"])))
                self.send_response(204)
                self.end_headers()
            def log_message(self, *args):
                pass
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Partner) as server:
            self.config = self.config.replace(":8181", f":{server.server_port}")
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                app = self.launch()
                self.assertEqual(self.post(self.order), (202, b""))
                a, b = [replies.get(timeout=5) for _ in range(2)]
                self.assertIn(b"UCI+ORDER42+BUYER:ZZ+SUPPLIER:ZZ+7'", a)
                self.assertIn(b"UNB+UNOC:3+SUPPLIER:ZZ+BUYER:ZZ+", b)
                self.assertIn(b"ORDRSP:D:96A:UN'", b)
                self.assertTrue(b.endswith(b"UNT+3+1'UNZ+1+2'"))
                app.wait("delivered")
                app.wait("delivered")
            finally:
                server.shutdown()
                thread.join()
        self.assertEqual(self.post(b"\xffgarbage"), (202, b""))
        app.wait("finding")
        app.command("send", "Supplier", "confirmation.edi")
        failed = app.wait("failed")["transfer"]
        app.close(crash=True)
        app = self.launch()
        app.command("resend", str(failed))
        self.assertNotEqual(app.wait("failed")["transfer"], failed)
        app.close()
        with contextlib.closing(sqlite3.connect(self.db)) as db:
            self.assertEqual(db.execute("pragma integrity_check").fetchone(), ("ok",))
            self.assertEqual(db.execute("select count(*) from transfers where status='failed'").fetchone(), (2,))
            self.assertEqual(db.execute("select bytes from transfers where id=(select transfer from findings limit 1)").fetchone(), (b"\xffgarbage",))
            self.assertEqual(db.execute("select count(*) from transfers t join executions e on e.id=t.execution where t.direction='out' and e.root=1").fetchone(), (2,))
            self.assertEqual(db.execute("select count(*) from transfers where resend_of=?", (failed,)).fetchone(), (1,))

    def test_transport_and_acknowledgement_faults(self):
        self.config = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        self.config = self.config.replace('{ document = "confirmation.edi" }', '{ fault = "http500" }, { fault = "reject" }')
        app = self.launch()
        self.assertEqual(self.post(self.order), (500, b""))
        app.wait("delivered")
        app.close()
        files = list((self.root / "out").glob("*.edi"))
        self.assertEqual(len(files), 1)
        self.assertIn(b"UCI+ORDER42+BUYER:ZZ+SUPPLIER:ZZ+4'", files[0].read_bytes())

    def test_slow_and_silent_http_receipts_preserve_scripted_content_fault(self):
        base = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        (self.root / "reply.js").write_text("""function reply(inbound, api) {
            api.send(api.build("ORDRSP:D:96A:UN", "FTX+example'"));
        }""")
        for fault in ["slow_response", "no_response"]:
            with self.subTest(fault=fault):
                actions = '{ fault = "' + fault + '" }, { fault = "suppress_ack" }, { fault = "malformed" }, { script = "reply.js", function = "reply" }'
                self.config = base.replace('{ document = "confirmation.edi" }', actions)
                app = self.launch()
                if fault == "slow_response":
                    start = time.monotonic()
                    self.assertEqual(self.post(self.order), (202, b""))
                    self.assertGreaterEqual(time.monotonic() - start, 1.8)
                else:
                    with self.assertRaises((http.client.RemoteDisconnected, TimeoutError, ConnectionResetError)):
                        self.post(self.order)
                transfer = app.wait("delivered")["transfer"]
                app.close()
                reply = (self.root / "out" / f"vendredi-{transfer}.edi").read_bytes()
                self.assertIn(b"FTX+example'", reply)
                self.assertFalse(reply.endswith(b"'"))

    def test_script_sandbox_recovery_and_persistence(self):
        self.config = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        self.config = self.config.replace('{ document = "confirmation.edi" }',
            '{ fault = "suppress_ack" }, { script = "reply.js", function = "reply" }')
        good = """function reply(inbound, api) {
            if (globalThis.previous) throw Error("shared JS global");
            globalThis.previous = true;
            const calls = (api.get("calls") || 0) + 1;
            api.set("calls", calls);
            api.log((inbound === null ? "command" : "order") + "-" + calls);
            api.send(api.build("ORDRSP:D:96A:UN", "BGM+231+" + api.nextReference() + "'"));
        }"""
        (self.root / "reply.js").write_text(good)
        app = self.launch()
        self.assertEqual(self.post(self.order), (202, b""))
        self.assertEqual(app.wait("script_log")["text"], "order-1")
        first = app.wait("delivered")["transfer"]
        self.assertTrue((self.root / "out" / f"vendredi-{first}.edi").read_bytes()
                        .endswith(b"BGM+231+1'UNT+3+1'UNZ+1+2'"))

        # Stage effects before each failure and check the durable state afterwards.
        prefix = """function fail(i,a) {
            a.set("calls", 99); a.nextReference(); a.log("discard");
            a.send(a.build("ORDRSP:D:96A:UN", "BGM+discard'"));
        """
        cases = [
            ("syntax", "function fail( {", "SyntaxError"),
            ("file", prefix + "require('fs'); }", "ReferenceError"),
            ("fuel", prefix + "for(let i=0;i<99000;i++) Math.sin(i); }", "fuel"),
            ("memory", prefix + "new ArrayBuffer(40 * 1024 * 1024); }", "memory"),
            ("logs", prefix + "for(let i=0;i<130;i++) a.log(i); }", "too many"),
            ("source", " " * 65537, "source exceeds"),
        ]
        for name, source, expected in cases:
            with self.subTest(name=name):
                (self.root / "fail.js").write_text(source)
                app.command("send", "Supplier", "fail.js", "fail")
                error = app.wait("action")["error"]
                self.assertIn(expected.lower(), error.lower())
                self.assertLess(len(error), 2048)

        app.command("send", "Supplier", "reply.js", "reply")
        self.assertEqual(app.wait("script_log")["text"], "command-2")
        second = app.wait("delivered")["transfer"]
        self.assertTrue((self.root / "out" / f"vendredi-{second}.edi").read_bytes()
                        .endswith(b"BGM+231+3'UNT+3+1'UNZ+1+4'"))
        app.close()
        app = self.launch()
        app.command("send", "Supplier", "reply.js", "reply")
        self.assertEqual(app.wait("script_log")["text"], "command-3")
        app.wait("delivered")
        app.close()
        with contextlib.closing(sqlite3.connect(self.db)) as db:
            reference, state = db.execute("SELECT reference,data FROM partner_state").fetchone()
            self.assertEqual((reference, json.loads(state)), (6, {"calls": 3}))
            self.assertEqual(db.execute("SELECT count(*) FROM transfers WHERE direction='out'").fetchone(), (3,))
            logs = [json.loads(row[0])["text"] for row in db.execute(
                "SELECT body FROM events WHERE json_extract(body,'$.event')='script_log' ORDER BY id")]
            self.assertEqual(logs, ["order-1", "command-2", "command-3"])

    def test_envelope_faults_and_duplicate_delivery(self):
        self.config = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        actions = '{ fault = "duplicate_reference" }, { fault = "wrong_count" }, { fault = "duplicate" }, { document = "confirmation.edi" }'
        self.config = self.config.replace('{ document = "confirmation.edi" }', actions)
        app = self.launch()
        self.assertEqual(self.post(self.order), (202, b""))
        for _ in range(3):
            app.wait("delivered")
        app.close()
        replies = [p.read_bytes() for p in sorted((self.root / "out").glob("*.edi"))]
        self.assertEqual(replies[1], replies[2])
        self.assertTrue(replies[1].endswith(b"UNZ+0+1'"))
        self.assertTrue(replies[0].endswith(b"UNZ+1+1'"))

    def test_folder_delivery_failure_can_be_resent(self):
        self.config = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        app = self.launch()
        output = self.root / "out"
        output.rmdir()
        output.write_text("unavailable")
        app.command("send", "Supplier", "confirmation.edi")
        failed = app.wait("failed")["transfer"]
        output.unlink()
        output.mkdir()
        app.command("resend", str(failed))
        delivered = app.wait("delivered")["transfer"]
        app.close()
        self.assertIn(b"ORDRSP:D:96A:UN", (output / f"vendredi-{delivered}.edi").read_bytes())

    def test_folder_publication_and_recovered_claim(self):
        self.config = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        app = self.launch()
        inbox = self.root / "inbox"
        partial = inbox / "order.part"
        partial.write_bytes(self.order[:10])
        self.assertTrue(partial.exists())
        with partial.open("ab") as file:
            file.write(self.order[10:])
        os.replace(partial, inbox / "order.edi")
        app.wait("delivered")
        app.wait("delivered")
        files = sorted((self.root / "out").glob("*.edi"))
        self.assertEqual(len(files), 2)
        self.assertIn(b"UCI+ORDER42", files[0].read_bytes())
        self.assertIn(b"ORDRSP:D:96A:UN", files[1].read_bytes())
        app.close()
        with contextlib.closing(sqlite3.connect(self.db)) as db:
            claim, raw = db.execute("select claim,bytes from transfers where direction='in'").fetchone()
            self.assertEqual(raw, self.order)
        (inbox / f".vendredi-claim-{claim}_order.edi").write_bytes(self.order)
        app = self.launch()
        app.command("send", "Supplier", "confirmation.edi")
        app.wait("delivered")
        app.close()
        with contextlib.closing(sqlite3.connect(self.db)) as db:
            self.assertEqual(db.execute("select count(*) from transfers where direction='in'").fetchone(), (1,))
        self.assertFalse(list(inbox.glob(".vendredi-claim-*")))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--binary", type=Path, default=BINARY)
    source.add_argument("--archive", type=Path, help="test the executable extracted from a release archive")
    args = parser.parse_args()
    BINARY = args.binary.resolve()
    with tempfile.TemporaryDirectory() as tmp:
        if args.archive:
            if args.archive.suffix == ".zip":
                with zipfile.ZipFile(args.archive) as package:
                    names = {"vendredi.exe", "Vendredi.app/Contents/MacOS/vendredi"}
                    member, = names.intersection(package.namelist())
                    BINARY = Path(tmp) / Path(member).name
                    BINARY.write_bytes(package.read(member))
            else:
                BINARY = Path(tmp) / "vendredi"
                with tarfile.open(args.archive) as package:
                    BINARY.write_bytes(package.extractfile("vendredi").read())
            BINARY.chmod(0o755)
        unittest.main(argv=["local_instance"], verbosity=2)
