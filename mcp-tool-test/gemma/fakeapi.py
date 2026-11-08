#!/usr/bin/env python3
"""A stand-in for the Pushover API that pushover-mcp-rs talks to in the
measurement (--api-url): nothing reaches Pushover, and every request is
kept, one JSON line each, in LOG.

    fakeapi.py PORT LOG.jsonl

Its world: two devices (a phone and a laptop whose names do not say so),
every built-in sound and one of the account's own (a kitchen gong), a
monthly quota of 10000 with 9500 left, which starts over on the first of
next month, and a receipt of its own for every emergency notification,
kept with its tags until it is cancelled. A receipt is not acknowledged. A
device the user does not have goes out to all devices, as with Pushover.
"""
import calendar
import datetime
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qsl, urlsplit

PORT, LOG = int(sys.argv[1]), sys.argv[2]
LOCK = threading.Lock()
DEVICES = ["pixel9", "thinkpad"]
# The built-in sounds as Pushover names them, and one of the account's own.
SOUNDS = {
    "pushover": "Pushover (default)", "bike": "Bike", "bugle": "Bugle", "cashregister": "Cash Register",
    "classical": "Classical", "cosmic": "Cosmic", "falling": "Falling", "gamelan": "Gamelan",
    "incoming": "Incoming", "intermission": "Intermission", "magic": "Magic", "mechanical": "Mechanical",
    "pianobar": "Piano Bar", "siren": "Siren", "spacealarm": "Space Alarm", "tugboat": "Tug Boat",
    "alien": "Alien Alarm (long)", "climb": "Climb (long)", "persistent": "Persistent (long)",
    "echo": "Pushover Echo (long)", "updown": "Up Down (long)", "vibrate": "Vibrate Only", "none": "None (silent)",
    "kuechengong": "Küchengong",
}
# Every emergency notification: its receipt, its tags and whether it still repeats.
receipts = {}


def next_reset():
    """The first of next month, 06:00 UTC, as Pushover starts the count over at the turn of the month."""
    now = datetime.datetime.now(datetime.timezone.utc)
    year, month = (now.year + 1, 1) if now.month == 12 else (now.year, now.month + 1)
    return calendar.timegm((year, month, 1, 6, 0, 0))


def form_fields(content_type, body):
    """The fields of a URL-encoded or multipart body, file parts by their size."""
    if content_type.startswith("multipart/form-data"):
        boundary = content_type.split("boundary=", 1)[1].strip('"').encode()
        fields = []
        for part in body.split(b"--" + boundary)[1:-1]:
            head, _, data = part.strip(b"\r\n").partition(b"\r\n\r\n")
            head = head.decode(errors="replace")
            name = head.split('name="', 1)[1].split('"', 1)[0]
            fields.append((name, f"<file, {len(data)} bytes>" if "filename=" in head else data.decode(errors="replace")))
        return fields
    return parse_qsl(body.decode(errors="replace"), keep_blank_values=True)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def answer(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def handle_any(self, method):
        url = urlsplit(self.path)
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length) if length else b""
        fields = dict(parse_qsl(url.query))
        fields.update(form_fields(self.headers.get("Content-Type", ""), body))
        fields.pop("token", None)
        fields.pop("user", None)
        path = url.path
        with LOCK:
            reply = self.reply(method, path, fields)
            with open(LOG, "a") as f:
                f.write(json.dumps({"t": time.time(), "method": method, "path": path, "fields": fields,
                                    "reply": reply[1]}, ensure_ascii=False) + "\n")
        self.answer(*reply)

    def reply(self, method, path, fields):
        now = int(time.time())
        if method == "POST" and path == "/1/messages.json":
            # A device the user does not have is no error: Pushover delivers to all devices then (its API
            # documentation, checked on 08.11.2026).
            reply = {"status": 1, "request": f"req{time.time_ns()}"}
            if fields.get("priority") == "2":
                receipt = f"r{len(receipts) + 1:029d}"
                receipts[receipt] = {"tags": [t for t in fields.get("tags", "").split(",") if t], "active": True}
                reply["receipt"] = receipt
            return 200, reply
        if method == "POST" and path == "/1/users/validate.json":
            return 200, {"status": 1, "group": 0, "devices": DEVICES, "request": "req-validate"}
        if method == "GET" and path == "/1/sounds.json":
            return 200, {"status": 1, "request": "req-sounds", "sounds": SOUNDS}
        if method == "GET" and path == "/1/apps/limits.json":
            return 200, {"status": 1, "request": "req-limits", "limit": 10000, "remaining": 9500, "reset": next_reset()}
        if path.startswith("/1/receipts/cancel_by_tag/") and method == "POST":
            tag = path.removeprefix("/1/receipts/cancel_by_tag/").removesuffix(".json")
            hit = [r for r in receipts.values() if r["active"] and tag in r["tags"]]
            for r in hit:
                r["active"] = False
            return 200, {"status": 1, "request": "req-cancel-tag", "canceled": len(hit)}
        if path.startswith("/1/receipts/") and path.endswith("/cancel.json") and method == "POST":
            receipt = path.removeprefix("/1/receipts/").removesuffix("/cancel.json")
            if receipt not in receipts:
                return 400, {"status": 0, "errors": ["receipt not found; may be invalid or expired"], "request": "req"}
            receipts[receipt]["active"] = False
            return 200, {"status": 1, "request": "req-cancel"}
        if path.startswith("/1/receipts/") and path.endswith(".json") and method == "GET":
            receipt = path.removeprefix("/1/receipts/").removesuffix(".json")
            if receipt not in receipts:
                return 400, {"status": 0, "errors": ["receipt not found; may be invalid or expired"], "request": "req"}
            return 200, {"status": 1, "request": "req-receipt", "acknowledged": 0, "acknowledged_at": 0,
                         "last_delivered_at": now - 30, "expired": 0, "expires_at": now + 3600}
        return 404, {"status": 0, "errors": ["not found"], "request": "req"}

    def do_GET(self):
        self.handle_any("GET")

    def do_POST(self):
        self.handle_any("POST")


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
