"""Run real HTTP, watched-folder, CLI and recovery checks against a local Vendredi binary."""
import argparse
import contextlib
import http.client
import http.server
import json
import tomllib
import os
from pathlib import Path
import queue
import shutil
import socket
import ssl
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
CASES = ROOT / "examples/cases"
X12 = ROOT / "examples/x12"


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


@contextlib.contextmanager
def https_library(root):
    """Serve the starter cases and some broken sources over local HTTPS."""
    authority, server_key, server_cert = root / "ca.pem", root / "server.key", root / "server.pem"
    openssl = lambda *args: subprocess.run(["openssl", *args], check=True, capture_output=True)
    # A config of our own, so the machine's openssl.cnf adds nothing: a CA with
    # duplicated extensions is valid to some tools and rejected by rustls.
    config = root / "openssl.cnf"
    # Names come only from -subj: with "prompt = no" LibreSSL ignores -subj.
    config.write_text("[req]\ndistinguished_name = name\n\n[name]\n\n"
                      "[authority]\nbasicConstraints = critical,CA:TRUE\n"
                      "keyUsage = critical,keyCertSign\nsubjectKeyIdentifier = hash\n")
    openssl("req", "-config", config, "-x509", "-extensions", "authority", "-newkey", "rsa:2048", "-nodes",
            "-days", "1", "-keyout", root / "ca.key", "-out", authority, "-subj", "/CN=Vendredi test CA")
    openssl("req", "-config", config, "-newkey", "rsa:2048", "-nodes", "-keyout", server_key,
            "-out", root / "server.csr", "-subj", "/CN=127.0.0.1")
    (root / "server.ext").write_text("basicConstraints=CA:FALSE\nsubjectAltName=IP:127.0.0.1\n")
    openssl("x509", "-req", "-in", root / "server.csr", "-CA", authority, "-CAkey", root / "ca.key",
            "-CAcreateserial", "-days", "1", "-out", server_cert, "-extfile", root / "server.ext")
    # The chain must verify with the same tool, whatever openssl this machine has.
    openssl("verify", "-CAfile", authority, server_cert)
    broken = json.loads((CASES / "reject-orders.json").read_text())
    broken["files"]["nack.edi"] = "not EDIFACT"
    extra = {
        "/broken/catalog.json": json.dumps({"vendredi_catalog": 1, "cases": ["reject.json"]}),
        "/broken/reject.json": json.dumps(broken),
        "/big.json": "{" + " " * (1024 * 1024 + 1) + "}",
    }
    class Library(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=CASES, **kwargs)
        def do_GET(self):
            if self.path == "/redirect.json":
                self.send_response(302)
                self.send_header("Location", f"http://127.0.0.1:{self.server.server_port}/catalog.json")
                self.end_headers()
            elif self.path in extra:
                body = extra[self.path].encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
            else:
                super().do_GET()
        def log_message(self, *args):
            pass
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(server_cert, server_key)
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Library) as server:
        server.socket = context.wrap_socket(server.socket, server_side=True)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"https://127.0.0.1:{server.server_port}", authority
        finally:
            server.shutdown()
            thread.join()


def snapshot(folder):
    """Every installed case file, without the import time and source."""
    files = {}
    for path in sorted(folder.rglob("*")):
        if path.is_file():
            data = path.read_bytes()
            if path.name == ".vendredi-source.json":
                data = json.loads(data)["sha256"].encode()
            files[path.relative_to(folder).as_posix()] = data
    return files


def write_case(root, standard, message, version, actions):
    """Writes the workspace's `orders` case with one rule. Actions use the
    TOML inline form; files they name are copied from the workspace root."""
    folder = root / "cases/orders"
    folder.mkdir(parents=True, exist_ok=True)
    parsed = tomllib.loads(f"actions = [{actions}]")["actions"]
    for action in parsed:
        name = action.get("document") or action.get("script")
        if name and (root / name).exists():
            shutil.copy(root / name, folder / name)
    case = {"vendredi_case": 1, "id": "orders", "name": "Orders", "standard": standard,
            "rules": [{"name": "Orders", "message": message, "version": version, "actions": parsed}]}
    (folder / "case.json").write_text(json.dumps(case, indent=2))


class LocalExchange(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.listener = port()
        self.config = (SAMPLE / "vendredi.toml").read_text().replace(":8180", f":{self.listener}")
        self.order = (SAMPLE / "order.edi").read_bytes()
        shutil.copy(SAMPLE / "cases/orders/confirmation.edi", self.root)
        self.db = self.root / ".vendredi/vendredi.db"
        # The rule of the workspace's `orders` case, written on each launch.
        self.case = ("edifact", "ORDERS", "D96A")
        self.actions = '{ document = "confirmation.edi" }'

    def launch(self):
        (self.root / "vendredi.toml").write_text(self.config)
        if 'case = "orders"' in self.config:
            write_case(self.root, *self.case, self.actions)
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
        self.actions = '{ fault = "http500" }, { fault = "reject" }'
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
            api.send(api.build("ORDRSP", "D96A", [["FTX", "example"]]));
        }""")
        for fault in ["slow_response", "no_response"]:
            with self.subTest(fault=fault):
                actions = '{ fault = "' + fault + '" }, { fault = "suppress_ack" }, { fault = "malformed" }, { script = "reply.js", function = "reply" }'
                self.config = base
                self.actions = actions
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
        self.actions = '{ fault = "suppress_ack" }, { script = "reply.js", function = "reply" }'
        good = """function reply(inbound, api) {
            if (globalThis.previous) throw Error("shared JS global");
            globalThis.previous = true;
            const calls = (api.get("calls") || 0) + 1;
            api.set("calls", calls);
            api.log((inbound === null ? "command" : "order") + "-" + calls);
            api.send(api.build("ORDRSP", "D96A", [["BGM", "231", api.nextReference()]]));
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
            a.send(a.build("ORDRSP", "D96A", [["BGM", "discard"]]));
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

    def test_structured_and_legacy_scripts_reply_identically(self):
        base = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        self.actions = '{ fault = "suppress_ack" }, { script = "reply.js", function = "reply" }'
        scripts = {
            "legacy": """function reply(inbound, api) {
                const order = inbound.messages[0].segments[1].elements[1][0];
                api.send(api.build("ORDRSP:D:96A:UN", "BGM+231+" + order + "+9'FTX+AAI+++a?+b?:c'"));
            }""",
            "structured": """function reply(inbound, api) {
                const order = inbound.groups[0].messages[0];
                const bgm = order.segments.find(s => s[0] === "BGM");
                api.send(api.build("ORDRSP", order.version, [["BGM", "231", bgm[2], "9"], ["FTX", "AAI", "", "", "a+b:c"]]));
            }""",
        }
        messages = {}
        for name, source in scripts.items():
            with self.subTest(script=name):
                (self.root / "reply.js").write_text(source)
                self.config = base
                app = self.launch()
                self.assertEqual(self.post(self.order), (202, b""))
                transfer = app.wait("delivered")["transfer"]
                app.close()
                reply = (self.root / "out" / f"vendredi-{transfer}.edi").read_bytes()
                messages[name] = reply[reply.index(b"UNH"):reply.index(b"UNZ")]
                features = [e["feature"] for e in app.seen if e["event"] == "deprecated"]
                expected = ["interchange.messages", "api.build(type, text)", "api.send(text)"]
                self.assertEqual(features, expected if name == "legacy" else [])
        self.assertEqual(messages["legacy"], messages["structured"])
        self.assertEqual(messages["structured"],
                         b"UNH+1+ORDRSP:D:96A:UN'BGM+231+PO42+9'FTX+AAI+++a?+b?:c'UNT+4+1'")

    def test_cases_switch_while_listening_and_persist(self):
        subprocess.run([BINARY, "init", self.root], check=True, capture_output=True)
        config = (self.root / "vendredi.toml").read_text()
        self.assertIn('case = "confirm-orders"', config)
        self.config = config.replace(":8180", f":{self.listener}")
        listed = subprocess.run([BINARY, "case", "list", self.root], check=True, capture_output=True, text=True).stdout
        self.assertIn("* confirm-orders", listed)
        self.assertIn("reject-orders", listed)
        app = self.launch()
        self.assertEqual(self.post(self.order), (202, b""))
        app.wait("delivered")
        app.wait("delivered")
        app.command("case", "Supplier", "reject-orders")
        self.assertEqual(app.wait("case_switched")["to"], "reject-orders")
        self.assertEqual(self.post(self.order), (202, b""))
        execution = app.wait("execution")
        self.assertEqual((execution["name"], execution["case"]), ("acknowledge", "reject-orders"))
        rejected = app.wait("delivered")["transfer"]
        app.command("case", "Supplier", "failing-http")
        app.wait("case_switched")
        self.assertEqual(self.post(self.order)[0], 500)
        app.wait("delivered")
        app.wait("delivered")
        app.command("case", "Supplier", "missing")
        self.assertIn("not installed", app.events.get(timeout=10)["error"])
        app.close()
        self.assertIn(b"UCI+ORDER42+BUYER:ZZ+SUPPLIER:ZZ+4'",
                      (self.root / "replies" / f"vendredi-{rejected}.edi").read_bytes())
        self.assertIn('case = "failing-http"', (self.root / "vendredi.toml").read_text())

        # The active case survives a restart, and samples come from it.
        self.config = (self.root / "vendredi.toml").read_text()
        app = self.launch()
        app.command("case", "Supplier", "send-invoice")
        app.wait("case_switched")
        app.command("sample", "Supplier", "Order")
        received = app.wait("received")
        self.assertEqual(received["metadata"], {"case": "send-invoice", "sample": "Order"})
        app.wait("delivered")
        app.wait("delivered")
        app.command("initiate", "Supplier", "Invoice")
        invoice = app.wait("delivered")["transfer"]
        app.close()
        self.assertIn(b"INVOIC:D:96A:UN", (self.root / "replies" / f"vendredi-{invoice}.edi").read_bytes())
        subprocess.run([BINARY, "case", "use", self.root, "Supplier", "confirm-orders"], check=True, capture_output=True)
        self.assertIn('case = "confirm-orders"', (self.root / "vendredi.toml").read_text())

    def test_import_cases_from_folder_and_https(self):
        folder, remote = self.root / "folder", self.root / "remote"
        for workspace in (folder, remote):
            subprocess.run([BINARY, "init", workspace], check=True, capture_output=True)
            shutil.rmtree(workspace / "cases")
        run = lambda workspace, *args, env=None: subprocess.run(
            [BINARY, "case", "import", workspace, *args], capture_output=True, text=True, env=env)
        result = run(folder, CASES)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Imported 13 cases", result.stdout)
        with https_library(self.root) as (base, certificate):
            env = dict(os.environ, SSL_CERT_FILE=str(certificate))
            # What a failure on another machine needs to be understood.
            tools = subprocess.run(["openssl", "version"], capture_output=True, text=True).stdout.strip()
            context = f"openssl: {tools}; SSL variables: " + ", ".join(
                f"{k}={v}" for k, v in sorted(env.items()) if k.startswith("SSL_"))
            untrusted = run(remote, f"{base}/catalog.json")
            self.assertNotEqual(untrusted.returncode, 0)
            self.assertFalse((remote / "cases").exists())
            for source, message in [
                (f"{base}/broken/catalog.json", "nack.edi"),
                (f"{base}/big.json", "exceeds"),
                (f"{base}/redirect.json", "non-HTTPS"),
                (f"http://127.0.0.1:1/catalog.json", "only https"),
            ]:
                with self.subTest(source=source):
                    result = run(remote, source, env=env)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(message, result.stderr, context)
                    self.assertFalse((remote / "cases").exists())
            result = run(remote, f"{base}/catalog.json", env=env)
            self.assertEqual(result.returncode, 0, result.stderr + context)
            source = json.loads((remote / "cases/send-invoice/.vendredi-source.json").read_text())
            self.assertEqual(source["source"], f"{base}/send-invoice.json")
        self.assertEqual(snapshot(folder / "cases"), snapshot(remote / "cases"))
        listed = subprocess.run([BINARY, "case", "list", remote], check=True, capture_output=True, text=True)
        self.assertIn("send-invoice", listed.stdout)

    def test_case_self_test(self):
        result = subprocess.run([BINARY, "case", "test", CASES], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("14 passed, 0 failed", result.stdout)
        case = json.loads((CASES / "send-invoice.json").read_text())
        case["files"]["expected.edi"] = case["files"]["invoic.edi"].replace("1250.00", "1250.50")
        case["expect"][1]["replies"] = ["expected.edi"]
        changed = self.root / "send-invoice.json"
        changed.write_text(json.dumps(case))
        result = subprocess.run([BINARY, "case", "test", changed], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL  send-invoice — initiate Invoice", result.stdout)
        self.assertIn("ok    send-invoice — sample Order", result.stdout)
        self.assertIn("reply 1 (expected.edi) differs at byte 73", result.stdout)

    def x12_workspace(self, actions=None, send='directory = "out"'):
        shutil.copy(X12 / "cases/orders/855.edi", self.root)
        config = (X12 / "vendredi.toml").read_text().replace(":8180", f":{self.listener}")
        self.config = config.replace('url = "http://127.0.0.1:8181/replies"', send)
        self.case = ("x12", "850", "005010")
        self.actions = actions or '{ document = "855.edi" }'
        return (X12 / "850-5010.edi").read_bytes()

    def x12_replies(self):
        replies = {}
        for path in sorted((self.root / "out").glob("*.edi"), key=lambda p: int(p.stem.split("-")[1])):
            data = path.read_bytes()
            kind = data[data.find(b"ST*") + 3:data.find(b"ST*") + 6] if b"ST*" in data else b"TA1"
            replies.setdefault(kind.decode(), []).append(data)
        return replies

    def post_x12(self, data, path="/supplier/850"):
        conn = http.client.HTTPConnection("127.0.0.1", self.listener, timeout=5)
        try:
            conn.request("POST", path, data, {"Content-Type": "application/edi-x12"})
            r = conn.getresponse()
            return r.status, r.read()
        finally:
            conn.close()

    def test_x12_exchange_over_http_and_folders(self):
        replies = queue.Queue()
        class Partner(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                replies.put(self.rfile.read(int(self.headers["Content-Length"])))
                self.send_response(204)
                self.end_headers()
            def log_message(self, *args):
                pass
        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Partner) as server:
            orders = self.x12_workspace(send=f'url = "http://127.0.0.1:{server.server_port}/replies"')
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                app = self.launch()
                self.assertEqual(self.post_x12(orders), (202, b""))
                received = sorted(replies.get(timeout=5) for _ in range(5))
                for _ in range(5):
                    app.wait("delivered")
                app.close()
            finally:
                server.shutdown()
                thread.join()
        acks = [r for r in received if b"ST*997*" in r]
        self.assertEqual(len(acks), 2)
        self.assertTrue(any(b"AK1*PO*1001*005010~" in a and b"AK9*A*2*2*2~" in a for a in acks))
        self.assertEqual(sum(b"ST*855*0001~BAK*00*AC*PO5001*20261125~SE*3*0001~" in r for r in received), 3)
        self.assertTrue(all(r.startswith(b"ISA*00*          *00*          *ZZ*SUPPLIER       *ZZ*BUYER") for r in received))

        # The same exchange through watched folders, then an incoming 997.
        self.config = self.config.replace(f'url = "http://127.0.0.1:{server.server_port}/replies"', 'directory = "out"')
        app = self.launch()
        partial = self.root / "x12.part"
        partial.write_bytes(orders)
        os.replace(partial, self.root / "inbox" / "orders.x12")
        delivered = [app.wait("delivered")["transfer"] for _ in range(5)]
        files = self.x12_replies()
        self.assertEqual((len(files["997"]), len(files["855"])), (2, 3))
        reply = files["855"][0]
        control = int(reply[90:99])
        ack = ("ISA*00*          *00*          *ZZ*BUYER          *ZZ*SUPPLIER       *261125*1100*^*00501*000000900*0*T*:~"
               f"GS*FA*BUYER*SUPPLIER*20261125*1100*900*X*005010~ST*997*0001~AK1*PR*{control}*005010~"
               "AK9*A*1*1*1~SE*4*0001~GE*1*900~IEA*1*000000900~").encode()
        self.assertEqual(self.post_x12(ack), (202, b""))
        self.assertEqual(app.wait("acknowledged")["status"], "accepted")
        app.close()
        self.assertEqual(len(self.x12_replies()["997"]), 2, "a 997 is not acknowledged")

    def test_x12_faults(self):
        expectations = {
            "reject": lambda r, status: (self.assertIn(b"AK9*R*2*2*0~", r["997"][0]), self.assertNotIn("855", r)),
            "suppress_ack": lambda r, status: self.assertEqual(sorted(r), ["855"]),
            "wrong_count": lambda r, status: self.assertTrue(r["855"][0].endswith(b"GE*0*3~IEA*0*000000003~")),
            "duplicate_reference": lambda r, status: self.assertEqual(r["855"][0][90:99], r["997"][1][90:99]),
            "duplicate": lambda r, status: self.assertEqual(len(r["855"]), 6),
            "malformed": lambda r, status: self.assertFalse(r["855"][0].endswith(b"~")),
            "http500": lambda r, status: self.assertEqual(status, 500),
            "slow_response": lambda r, status: self.assertEqual(status, 202),
        }
        for fault, check in expectations.items():
            with self.subTest(fault=fault):
                shutil.rmtree(self.root / "out", ignore_errors=True)
                shutil.rmtree(self.root / ".vendredi", ignore_errors=True)
                orders = self.x12_workspace(f'{{ fault = "{fault}" }}, {{ document = "855.edi" }}')
                app = self.launch()
                status = self.post_x12(orders)[0]
                expected = {"reject": 2, "suppress_ack": 3, "duplicate": 8}.get(fault, 5)
                for _ in range(expected):
                    app.wait("delivered")
                app.close()
                check(self.x12_replies(), status)

    def test_envelope_faults_and_duplicate_delivery(self):
        self.config = self.config.replace('url = "http://127.0.0.1:8181/replies"', 'directory = "out"')
        actions = '{ fault = "duplicate_reference" }, { fault = "wrong_count" }, { fault = "duplicate" }, { document = "confirmation.edi" }'
        self.actions = actions
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
    parser.add_argument("tests", nargs="*", help="test names, such as LocalExchange.test_folder_publication_and_recovered_claim")
    args = parser.parse_args()
    BINARY = args.binary.resolve()
    with tempfile.TemporaryDirectory() as tmp:
        if args.archive:
            if args.archive.suffix == ".zip":
                # A Developer ID signature binds the Mac executable to its bundle, and macOS
                # kills it when it runs anywhere else, so the app is extracted whole.
                with zipfile.ZipFile(args.archive) as package:
                    names = {"vendredi.exe", "Vendredi.app/Contents/MacOS/vendredi"}
                    member, = names.intersection(package.namelist())
                    package.extractall(tmp)
                    BINARY = Path(tmp) / member
            else:
                BINARY = Path(tmp) / "vendredi"
                with tarfile.open(args.archive) as package:
                    BINARY.write_bytes(package.extractfile("vendredi").read())
            BINARY.chmod(0o755)
        unittest.main(argv=["local_instance", *args.tests], verbosity=2)
