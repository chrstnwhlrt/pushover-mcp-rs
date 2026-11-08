#!/usr/bin/env python3
"""What pushover-mcp-rs does under each --max-priority, without a model: for every cap the tools it lists, and
for sends with every priority (none given, each name, a Pushover number) the priority that reached the API and
the note of the result. Runs against fakeapi.py, never the real API.

    captest.py BINARY [--sender NAME]
"""
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run  # noqa: E402

CAPS = [None, "silent", "low", "normal", "high", "emergency"]
ASKED = [None, "silent", "low", "normal", "high", "emergency", 2]


class Server:
    def __init__(self, cmd):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.n = 0

    def call(self, method, params=None):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError(self.p.stderr.read()[-500:])
            message = json.loads(line)
            if message.get("id") == self.n:
                return message

    def notify(self, method):
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.p.stdin.flush()

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)


def main():
    binary = sys.argv[1]
    extra = sys.argv[2:]
    with tempfile.TemporaryDirectory() as tmp:
        log = Path(tmp) / "api.jsonl"
        port = run.free_port()
        fake = subprocess.Popen([sys.executable, "-I", str(HERE / "fakeapi.py"), str(port), str(log)])
        try:
            run.wait_for(port)
            for cap in CAPS:
                cmd = [binary, "--config", str(HERE / "dummy-config.json"), "--api-url", f"http://127.0.0.1:{port}/1",
                       *extra, *(["--max-priority", cap] if cap else [])]
                server = Server(cmd)
                server.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                           "clientInfo": {"name": "captest", "version": "1"}})
                server.notify("notifications/initialized")
                tools = server.call("tools/list")["result"]["tools"]
                send = next(t for t in tools if t["name"] == "send_notification")
                fields = sorted(send["inputSchema"].get("properties", {}))
                print(f"== --max-priority {cap or '(default)'}: tools {sorted(t['name'] for t in tools)}")
                print(f"   send fields {fields}")
                for asked in ASKED:
                    before = len(log.read_text().splitlines()) if log.exists() else 0
                    args = {"message": f"test {asked}"}
                    if asked is not None:
                        args["priority"] = asked
                    if "sender" in fields:
                        args["sender"] = "captest"
                    reply = server.call("tools/call", {"name": "send_notification", "arguments": args})
                    result = reply.get("result") or {}
                    lines = log.read_text().splitlines() if log.exists() else []
                    api = [json.loads(l) for l in lines[before:] if json.loads(l)["path"] == "/1/messages.json"]
                    delivered = api[-1]["fields"].get("priority", "(none)") if api else "-"
                    text = " ".join(c.get("text", "") for c in result.get("content", []))
                    note = (result.get("structuredContent") or {}).get("note")
                    flag = "ERROR " if result.get("isError") or "error" in reply else ""
                    print(f"   asked {asked!s:9} -> api priority {delivered!s:6} {flag}{('note: ' + note) if note else text[:80]}")
                server.close()
        finally:
            fake.terminate()
            fake.wait()


main()
