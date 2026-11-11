#!/usr/bin/env python3
"""Byte comparison of two builds of pushover-mcp-rs, without a model and
without the real API.

`record` keeps what a binary answers, one JSON line per step: --version and
--help, initialize for every protocol version with tools/list after it,
server/discover with tools/list for 2026-07-28, every call in calls.jsonl,
the requests in EXTRA (faulty tool calls, ping, other methods) and what
reached the API, under every --max-priority with and without --sender. The
API is the measurement kit's fakeapi.py with a fixed clock (detfake.py), so
two runs of one binary write the same bytes.

`diff` compares two recordings row by row. A row that differs only in the
version counts as equal; any other difference is shown and fails.

`calls` adds the distinct calls of the kit's runs (../gemma/work) to
calls.jsonl. Recordings made before it lack the new rows, so record both
sides again.

    compare.py record BINARY OUT.jsonl
    compare.py diff OLD.jsonl NEW.jsonl
    compare.py calls
"""
import concurrent.futures
import difflib
import json
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
KIT = HERE.parent / "gemma"
CALLS = HERE / "calls.jsonl"
CAPS = [None, "silent", "low", "normal", "high", "emergency"]
SENDERS = [None, "Otomo"]
VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25", "2026-07-28", "1999-01-01"]
ORDER = ["send_notification", "list_devices", "list_sounds", "get_message_limits",
         "check_emergency_receipt", "cancel_emergency_notification"]
META = {"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {}}
EXTRA = [
    ("tools/call", {"name": "no_such_tool", "arguments": {}}),
    ("tools/call", {"name": "send_notification", "arguments": {}}),
    ("tools/call", {"name": "send_notification", "arguments": {"message": 5, "sender": "x"}}),
    ("tools/call", {"name": "send_notification", "arguments": {"message": "x", "sender": "x", "priority": 7}}),
    ("tools/call", {"name": "send_notification", "arguments": {"message": "x", "sender": "x", "bogus": 1}}),
    ("ping", {}),
    ("resources/list", {}),
    ("prompts/list", {}),
    ("logging/setLevel", {"level": "debug"}),
    ("no/such/method", {}),
]
SHOWN = 10


def load_calls():
    """The calls to replay, in the order of the file; the arguments keep the
    order of their keys, which is the order the server receives them in."""
    lines = CALLS.read_text(encoding="utf-8").splitlines()
    return [(d["name"], d["arguments"]) for d in map(json.loads, lines)]


def call_key(name, args):
    return name, json.dumps(args, sort_keys=True, ensure_ascii=False)


def add_calls():
    calls = {call_key(*call) for call in load_calls()} if CALLS.exists() else set()
    before = len(calls)
    for path in sorted(KIT.glob("work/*.calls.jsonl")):
        for line in path.read_text(encoding="utf-8").splitlines():
            d = json.loads(line)
            calls.add(call_key(d["name"], d["sent"]))
    ordered = sorted(calls, key=lambda k: (ORDER.index(k[0]) if k[0] in ORDER else len(ORDER), *k))
    CALLS.write_text("".join(json.dumps({"name": name, "arguments": json.loads(args)}, ensure_ascii=False) + "\n"
                             for name, args in ordered), encoding="utf-8")
    print(f"{len(calls) - before} new, {len(calls)} calls in {CALLS.name}")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Server:
    def __init__(self, cmd):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.n = 0

    def send(self, obj):
        self.p.stdin.write(json.dumps(obj).encode() + b"\n")
        self.p.stdin.flush()

    def request(self, method, params):
        self.n += 1
        self.send({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
        while True:
            raw = self.p.stdout.readline()
            if not raw:
                return "<EOF>"
            if json.loads(raw).get("id") == self.n:
                return raw.decode()

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=30)


def one_config(binary, sender, cap, calls):
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        port = free_port()
        fake = subprocess.Popen([sys.executable, "-I", str(HERE / "detfake.py"), str(KIT), str(port),
                                 f"{tmp}/api.jsonl"])
        try:
            for _ in range(100):
                try:
                    socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                    break
                except OSError:
                    time.sleep(0.05)
            cmd = [binary, "--config", str(KIT / "dummy-config.json"), "--api-url", f"http://127.0.0.1:{port}/1",
                   *(["--sender", sender] if sender else []), *(["--max-priority", cap] if cap else [])]
            tag = f"sender={sender} cap={cap}"
            for version in VERSIONS:
                s = Server(cmd)
                out.append((tag, f"initialize {version}", s.request("initialize", {
                    "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "cmp", "version": "1"}})))
                s.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
                out.append((tag, f"tools/list after {version}", s.request("tools/list", {})))
                s.close()
            s = Server(cmd)
            out.append((tag, "server/discover", s.request("server/discover", {"_meta": META})))
            out.append((tag, "tools/list 2026-07-28", s.request("tools/list", {"_meta": META})))
            s.close()
            s = Server(cmd)
            s.request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                     "clientInfo": {"name": "cmp", "version": "1"}})
            s.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            for name, args in calls:
                out.append((tag, f"call {name} {json.dumps(args, sort_keys=True, ensure_ascii=False)}",
                            s.request("tools/call", {"name": name, "arguments": args})))
            for method, params in EXTRA:
                out.append((tag, f"{method} {json.dumps(params, sort_keys=True)}", s.request(method, params)))
            s.close()
            api = Path(f"{tmp}/api.jsonl")
            out.append((tag, "api log", api.read_text(encoding="utf-8") if api.exists() else ""))
        finally:
            fake.terminate()
            fake.wait()
    return out


def record(binary, out):
    calls = load_calls()
    jobs = [(sender, cap) for sender in SENDERS for cap in CAPS]
    with concurrent.futures.ThreadPoolExecutor(len(jobs)) as pool:
        results = list(pool.map(lambda job: one_config(binary, *job, calls), jobs))
    rows = [{"step": flag, "raw": subprocess.run([binary, flag], capture_output=True, text=True).stdout}
            for flag in ["--version", "--help"]]
    rows += [{"config": tag, "step": step, "raw": raw} for result in results for tag, step, raw in result]
    out = Path(out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows), encoding="utf-8")
    print(f"{len(calls)} calls, {len(rows)} rows -> {out}")


def load_rows(path):
    rows = {}
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        d = json.loads(line)
        key = (d.get("config"), d["step"])
        if key in rows:
            sys.exit(f"{path}: {label(key)} twice")
        rows[key] = d["raw"]
    return rows


def label(key):
    config, step = key
    return f"{config} | {step}" if config else step


def show(old, new):
    """The difference of two answers line by line, JSON pretty-printed; if
    the content is the same and only whitespace or escapes differ, where."""
    try:
        old_lines = json.dumps(json.loads(old), indent=2, ensure_ascii=False).splitlines()
        new_lines = json.dumps(json.loads(new), indent=2, ensure_ascii=False).splitlines()
    except ValueError:
        old_lines, new_lines = old.splitlines(), new.splitlines()
    lines = list(difflib.unified_diff(old_lines, new_lines, "old", "new", n=1, lineterm=""))
    if lines:
        return "\n".join(lines)
    at = next((i for i, (a, b) in enumerate(zip(old, new)) if a != b), min(len(old), len(new)))
    start = max(0, at - 30)
    return f"same content, other bytes from offset {at}: {old[start:at + 30]!r} / {new[start:at + 30]!r}"


def diff(old_path, new_path):
    old, new = load_rows(old_path), load_rows(new_path)
    old_version = old[(None, "--version")].split()[-1]
    new_version = new[(None, "--version")].split()[-1]

    def only_the_version(a, b):
        return a.replace(f'"version":"{old_version}"', f'"version":"{new_version}"').replace(
            f"pushover-mcp-rs {old_version}", f"pushover-mcp-rs {new_version}") == b

    version_only, changed = [], []
    for key, raw in old.items():
        if key in new and raw != new[key]:
            (version_only if only_the_version(raw, new[key]) else changed).append(key)
    only_old = [key for key in old if key not in new]
    only_new = [key for key in new if key not in old]

    def differ(rows):
        return f"{len(rows)} differ{'s' if len(rows) == 1 else ''}"

    summary = [f"{len(old)} rows" if len(old) == len(new) else f"{len(old)} and {len(new)} rows"]
    if version_only:
        steps = ", ".join(sorted({step.split(" ")[0] for _, step in version_only}))
        summary.append(f"{differ(version_only)} only in the version ({old_version} -> {new_version}: {steps})")
    if changed:
        summary.append(f"{differ(changed)} otherwise")
    if only_old:
        summary.append(f"{len(only_old)} only in {old_path}")
    if only_new:
        summary.append(f"{len(only_new)} only in {new_path}")
    if len(summary) == 1:
        summary.append("byte for byte equal")
    print(", ".join(summary))

    for key in changed[:SHOWN]:
        print(f"\n== {label(key)}\n{show(old[key], new[key])}")
    for path, keys in [(old_path, only_old), (new_path, only_new)]:
        if keys:
            print(f"\nonly in {path}:")
            for key in keys[:SHOWN]:
                print(f"  {label(key)}")
    hidden = max(0, len(changed) - SHOWN) + max(0, len(only_old) - SHOWN) + max(0, len(only_new) - SHOWN)
    if hidden:
        print(f"\n... and {hidden} more")
    return 1 if changed or only_old or only_new else 0


def main():
    args = sys.argv[1:]
    if len(args) == 3 and args[0] == "record":
        record(args[1], args[2])
    elif len(args) == 3 and args[0] == "diff":
        sys.exit(diff(args[1], args[2]))
    elif args == ["calls"]:
        add_calls()
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
