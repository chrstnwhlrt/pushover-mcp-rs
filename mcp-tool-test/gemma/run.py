#!/usr/bin/env python3
"""Whether Gemma 4 E4B uses the Pushover tools right, as the voice
assistant Otomo puts them in front of it: its own system prompt, its gate,
its built-in tools and the tools of pushover-mcp-rs as its MCP client lists
them. The real server answers, against a stand-in of the Pushover API
(fakeapi.py), so nothing reaches anybody's phone; textproxy.py between them
puts the texts of a variant in place (--texts) and keeps every call.
proxy.py in front of Ollama keeps every model call with its reasoning, for
why.py.

    run.py TAG [--runs N] [--only REGEX] [--texts FILE] [--set NAME]
    run.py TAG --judge        score the answers of TAG again

The checkout of Otomo, with a release build of otomo-probe and its
config.toml, is OTOMO_DIR (default: otomo next to this repository); the
identity of that configuration that asks is OTOMO_IDENTITY (default:
Christian).

Writes work/TAG.jsonl (one line per scored answer), TAG.calls.jsonl (the
tool calls), TAG.api.jsonl (what reached the API), TAG.bodies.jsonl (the
model calls) and TAG.txt (the score).
"""
import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
WORK = HERE / "work"
OTOMO = Path(os.environ.get("OTOMO_DIR") or HERE.parents[2] / "otomo")
SERVER = "pushover"
IDENTITY = os.environ.get("OTOMO_IDENTITY") or "Christian"
BINARY = shutil.which("pushover-mcp-rs") or "pushover-mcp-rs"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_for(port, seconds=10):
    end = time.time() + seconds
    while time.time() < end:
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            return
        except OSError:
            time.sleep(0.1)
    sys.exit(f"nothing answers on port {port}")


def sections(text):
    """The configuration as its tables: (header, lines), the lines before the first table under the header None."""
    found, header, lines = [], None, []
    for line in text.splitlines(keepends=True):
        if line.startswith("["):
            found.append((header, lines))
            header, lines = line.split("#")[0].strip(), [line]
        else:
            lines.append(line)
    found.append((header, lines))
    return found


def assistant_config(path, proxy, server_args):
    """The assistant's own configuration for the run: IDENTITY alone, with the Pushover server as its only tool
    server, never asking before a call (an open question would carry over into the next request), and the proxy in
    front of Ollama. Returns it with the address of Ollama. Never printed: it holds the assistant's secrets."""
    text = path.read_text()
    config = tomllib.loads(text)
    upstream = config.get("llm", {}).get("ollama", {}).get("base_url")
    if not upstream or text.count(f'"{upstream}"') != 1:
        sys.exit(f"{path} needs llm.ollama.base_url, once, for the proxy to take its place")
    kept = []
    for header, lines in sections(text):
        if header == "[[mcp.servers]]" or (header or "").startswith("[identities."):
            continue
        if header == "[[identities]]":
            if not any(re.match(rf'\s*name\s*=\s*"{IDENTITY}"', line) for line in lines):
                continue
            lines = [re.sub(r"^(\s*servers\s*=\s*)\[[^\]]*\]", rf'\1["{SERVER}"]', line) for line in lines
                     if not re.match(r"\s*ids\s*=", line)]
        kept.append((header, lines))
    block = (f'[[mcp.servers]]\nname = "{SERVER}"\ntransport = "stdio"\nconfirm = "never"\n'
             f'command = {json.dumps(sys.executable)}\nargs = {json.dumps(server_args)}\n\n')
    at = next(i for i, (header, _) in enumerate(kept) if header == "[mcp]")
    kept.insert(at + 1, ("[[mcp.servers]]", [block]))
    text = "".join(line for _, lines in kept for line in lines).replace(f'"{upstream}"', f'"{proxy}"')
    checked = tomllib.loads(text)
    identities = checked.get("identities", [])
    assert [i["name"] for i in identities] == [IDENTITY] and identities[0]["servers"] == [SERVER], "identities"
    assert [s["name"] for s in checked["mcp"]["servers"]] == [SERVER], "servers"
    return text, upstream


def turn_spans(questions, log):
    """Where each request of a run stands in the proxy's log: the numbers of its own model requests up to the next
    request of the run ("turns"), and those of the requests asked before it in its conversation ("context")."""
    entries = [json.loads(line) for line in open(log)] if log.exists() else []
    found, at = [], 0
    for question in questions:
        hit = next((i for i in range(at, len(entries)) if question.removeprefix("+") in entries[i]["text"]), None)
        found.append(hit)
        at = at if hit is None else hit + 1
    spans, begun = [], None
    for k, (question, hit) in enumerate(zip(questions, found)):
        if not question.startswith("+"):
            begun = hit
        if hit is None:
            spans.append({})
            continue
        following = next((i for i in found[k + 1:] if i is not None), len(entries))
        span = {"turns": [entries[hit]["asked"], entries[following]["asked"] if following < len(entries)
                          else entries[-1]["asked"] + 1]}
        if question.startswith("+") and begun is not None and begun < hit:
            span["context"] = [entries[begun]["asked"], entries[hit]["asked"]]
        spans.append(span)
    return spans


def ask(tag, cases, runs, texts, binary=BINARY, server_args=()):
    probe = OTOMO / "target" / "release" / "otomo-probe"
    WORK.mkdir(exist_ok=True)
    files = {k: WORK / f"{tag}.{k}" for k in ("calls.jsonl", "api.jsonl", "bodies.jsonl", "turns", "texts.json")}
    for path in list(files.values()) + [WORK / f"{tag}.turns.log"]:
        path.unlink(missing_ok=True)
    files["texts.json"].write_text(Path(texts).read_text() if texts else "{}")
    questions, scored = [], []
    for _ in range(runs):
        for case in cases:
            for i, before in enumerate(case["context"]):
                questions.append(before if i == 0 else "+" + before)
                scored.append(None)
            questions.append(("+" if case["context"] else "") + case["q"])
            scored.append(case["key"])
    api_port, model_port = free_port(), free_port()
    fake = subprocess.Popen([sys.executable, "-I", str(HERE / "fakeapi.py"), str(api_port), str(files["api.jsonl"])])
    server_args = ["-I", str(HERE / "textproxy.py"), str(files["texts.json"]), str(files["calls.jsonl"]),
                   str(files["turns"]), "--", binary, "--config", str(HERE / "dummy-config.json"),
                   "--api-url", f"http://127.0.0.1:{api_port}/1", *server_args]
    text, upstream = assistant_config(OTOMO / "config.toml", f"http://127.0.0.1:{model_port}", server_args)
    proxy = subprocess.Popen([sys.executable, "-I", str(HERE / "proxy.py"), str(model_port), upstream,
                              str(files["bodies.jsonl"]), str(files["turns"])],
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        wait_for(api_port)
        wait_for(model_port)
        with tempfile.TemporaryDirectory(dir=WORK) as tmp:
            path = Path(tmp) / "config.toml"
            path.touch(mode=0o600)
            path.write_text(text)
            result = subprocess.run(["nix", "develop", "-c", str(probe), "--config", str(path), "chat", "--json",
                                     "--identity", IDENTITY, *questions], cwd=OTOMO, capture_output=True, text=True)
    finally:
        for process in (proxy, fake):
            process.terminate()
            process.wait()
    (WORK / f"{tag}.log").write_text(result.stderr)
    answers = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")][1:]
    if len(answers) != len(questions):
        sys.exit(f"{len(answers)} answers to {len(questions)} questions, see {WORK / (tag + '.log')}")
    spans = turn_spans(questions, WORK / f"{tag}.turns.log")
    with open(WORK / f"{tag}.jsonl", "w") as f:
        for n, (key, answer, span) in enumerate(zip(scored, answers, spans), 1):
            if key is not None:
                f.write(json.dumps({"case": key, "n": n, **span, **answer}, ensure_ascii=False) + "\n")


def delivered(tag, calls):
    """The fields of the API request behind each successful send, in order: one request per send. Calls of
    sessions run side by side (claude_check.py) cannot be matched this way and keep none."""
    api = WORK / f"{tag}.api.jsonl"
    if not api.exists():
        return
    sent = [json.loads(line)["fields"] for line in open(api) if json.loads(line)["path"] == "/1/messages.json"]
    sends = [c for c in calls if c["name"] == "send_notification" and not c["error"]]
    if len(sent) != len(sends):
        print(f"{tag}: {len(sends)} sends against {len(sent)} API requests, sound and device as the model gave them")
        return
    for call, fields in zip(sends, sent):
        call["delivered"] = fields


def pools(catalogue):
    return {"cases": catalogue.CASES, "held": catalogue.HELD, "fresh": catalogue.FRESH, "capped": catalogue.CAPPED,
            "capped-high": catalogue.CAPPED_HIGH, "capped-low": catalogue.CAPPED_LOW,
            "capped-silent": catalogue.CAPPED_SILENT, "wecker": catalogue.WECKER}


def score(tag, only="", match_api=True):
    import cases as catalogue
    # A request can be in two sets with a check of each (a server with --max-priority sends no alarm): the run's own
    # set decides, as run.py and claude_check.py wrote it; the catalogue's for runs without one.
    pool = WORK / f"{tag}.pool"
    by_key = {}
    for case in (catalogue.WECKER + catalogue.CAPPED_SILENT + catalogue.CAPPED_LOW + catalogue.CAPPED_HIGH
                 + catalogue.CAPPED + catalogue.FRESH + catalogue.HELD + catalogue.CASES):
        by_key[case["key"]] = case
    for case in pools(catalogue)[pool.read_text().strip()] if pool.exists() else []:
        by_key[case["key"]] = case
    calls = [json.loads(line) for line in open(WORK / f"{tag}.calls.jsonl")] if (WORK / f"{tag}.calls.jsonl").exists() else []
    if match_api:
        delivered(tag, calls)
    rows = [json.loads(line) for line in open(WORK / f"{tag}.jsonl")]
    per_case, groups, notes = defaultdict(list), defaultdict(lambda: [0, 0]), Counter()
    out = []
    for row in rows:
        case = by_key.get(row["case"])
        if case is None or (only and not re.search(only, row["case"])):
            continue
        lo, hi = row.get("turns", [None, None])
        mine = [c for c in calls if lo is not None and lo <= c["turn"] < hi]
        ctx = row.get("context")
        context = [c for c in calls if ctx and ctx[0] <= c["turn"] < ctx[1]]
        answer = row.get("answer") or ""
        if lo is None:
            ok, why = False, "turn not found"
        elif getattr(case["check"], "wants_answer", False):
            ok, why = case["check"](mine, context, answer)
        else:
            ok, why = case["check"](mine, context)
        if ok and why == "asked back":
            notes["asked back"] += 1
        if ok and case.get("says") and not case["says"].search(answer):
            ok, why = False, "answer: " + answer[:90]
        if ok and case["group"] not in ("not",) and catalogue.WRONG_WORDS.search(answer) and not mine:
            ok, why = False, "answer: " + answer[:90]
        refused = sum(1 for c in mine if c["error"])
        if refused:
            notes["refused calls"] += refused
        for c in mine:
            if c["name"] == catalogue.SEND:
                notes["sender " + repr((c["arguments"].get("sender") or "").strip())] += 1
        per_case[row["case"]].append((ok, why))
        groups[case["group"]][0] += ok
        groups[case["group"]][1] += 1
        out.append({"case": row["case"], "ok": ok, "why": why, "answer": answer,
                    "calls": [{"name": c["name"], "arguments": c["arguments"], "error": c["error"],
                               **({"text": c["text"][:160]} if c["error"] else {})} for c in mine],
                    "seconds": row.get("seconds")})
    right = sum(ok for results in per_case.values() for ok, _ in results)
    total = sum(len(results) for results in per_case.values())
    lines = [f"{tag}: {right}/{total} right"]
    lines.append("groups: " + ", ".join(f"{g} {r}/{t}" for g, (r, t) in sorted(groups.items())))
    lines.append("notes: " + ", ".join(f"{k} {v}" for k, v in notes.most_common()))
    for key, results in per_case.items():
        hits = sum(ok for ok, _ in results)
        whys = "; ".join(sorted({why for ok, why in results if not ok}))
        lines.append(f"  {hits}/{len(results)}  {key[:110]}" + (f"  -- {whys}" if whys else ""))
    (WORK / f"{tag}.txt").write_text("\n".join(lines) + "\n")
    (WORK / f"{tag}.scored.jsonl").write_text("".join(json.dumps(o, ensure_ascii=False) + "\n" for o in out))
    print("\n".join(lines))


def main():
    p = argparse.ArgumentParser()
    p.add_argument("tag")
    p.add_argument("--runs", type=int, default=1)
    p.add_argument("--only", default="")
    p.add_argument("--texts")
    p.add_argument("--judge", action="store_true")
    p.add_argument("--held", action="store_true", help="ask the held-out requests instead of the catalogue")
    p.add_argument("--fresh", action="store_true", help="ask the requests made for the last check alone")
    p.add_argument("--capped", action="store_true", help="ask the requests for a server with --max-priority normal")
    p.add_argument("--set", choices=["cases", "held", "fresh", "capped", "capped-high", "capped-low", "capped-silent",
                                     "wecker"],
                   help="the set of requests to ask, by name (capped-high, -low and -silent are for a server with "
                        "that --max-priority, which --server-arg gives it)")
    p.add_argument("--claude", action="store_true", help="judge with Claude's rule for the sender (claude_check.py)")
    p.add_argument("--binary", default=BINARY, help="the pushover-mcp-rs to measure (default: the one on PATH)")
    p.add_argument("--server-arg", action="append", default=[], help="an argument for the server, such as --sender=Otomo")
    a = p.parse_args()
    import cases as catalogue
    if a.claude:
        catalogue.SENDER_RULE = "claude"
    if not a.judge:
        name = a.set or ("capped" if a.capped else "fresh" if a.fresh else "held" if a.held else "cases")
        chosen = [c for c in pools(catalogue)[name] if not a.only or re.search(a.only, c["key"])]
        WORK.mkdir(exist_ok=True)
        (WORK / f"{a.tag}.pool").write_text(name)
        ask(a.tag, chosen, a.runs, a.texts, a.binary, a.server_arg)
    # Claude's sessions run side by side: their API requests cannot be matched to their calls.
    score(a.tag, a.only, match_api=not a.claude)


if __name__ == "__main__":
    main()
