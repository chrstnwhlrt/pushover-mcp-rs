#!/usr/bin/env python3
"""The catalogue through Claude Code instead of Gemma, to see that texts
made for Gemma do not get worse for Claude, the server's first user: every
request without a context in a fresh headless session, with a system
prompt that names its session, no built-in tools, and the Pushover server
behind textproxy.py (with the texts of --texts) as its only MCP server.
Neither user settings, CLAUDE.md nor memory reach it (an empty working
directory, setting sources cut to local). The calls and the answer are
scored as run.py scores Gemma's, with Claude's rule for the sender: it
names Claude and the session.

    claude_check.py TAG [--texts FILE] [--held | --capped | --set NAME] [--server-arg ARG ...] [--jobs 4] [--model M] [--only REGEX]

Writes work/TAG.jsonl, TAG.calls.jsonl and the score as run.py --judge does.
"""
import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
WORK = HERE / "work"
SESSION = "garden planner"
SYSTEM = (f'You are Claude, working in the Claude Code session "{SESSION}" for the user. '
          "Answer in German, briefly.")


def session(prompt, config, model):
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLAUDE")}
    workdir = WORK / "claude-cwd"
    workdir.mkdir(parents=True, exist_ok=True)
    cmd = ["claude", "-p", "--model", model, "--setting-sources", "local", "--system-prompt", SYSTEM, "--tools", "",
           "--mcp-config", str(config), "--strict-mcp-config", "--allowedTools", "mcp__pushover",
           "--no-session-persistence", "--output-format", "stream-json", "--verbose", prompt]
    return subprocess.run(cmd, cwd=workdir, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                          timeout=600)


def ask(case, texts_file, api_port, model, binary, server_args):
    with tempfile.TemporaryDirectory(dir=WORK) as tmp:
        tmp = Path(tmp)
        (tmp / "turns").write_text("1 1")
        calls = tmp / "calls.jsonl"
        args = ["-I", str(HERE / "textproxy.py"), str(texts_file), str(calls), str(tmp / "turns"), "--",
                binary, "--config", str(HERE / "dummy-config.json"),
                "--api-url", f"http://127.0.0.1:{api_port}/1", *server_args]
        config = tmp / "mcp.json"
        config.write_text(json.dumps({"mcpServers": {"pushover": {"command": sys.executable, "args": args}}}))
        result = session(case["q"], config, model)
        answer = ""
        for line in result.stdout.splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if event.get("type") == "result":
                answer = event.get("result") or ""
        found = [json.loads(line) for line in open(calls)] if calls.exists() else []
        # Only the server of the run may be there: a tool of another server (the real Pushover one of the user's
        # settings) would send real notifications.
        foreign = sorted(set(re.findall(r'"name":\s*"(mcp__(?!pushover__)[a-z_]+)', result.stdout)))
        if foreign:
            sys.exit(f"STOP: tools of another server in the session: {foreign}")
        return case, answer, found


def main():
    p = argparse.ArgumentParser()
    p.add_argument("tag")
    p.add_argument("--texts")
    p.add_argument("--held", action="store_true")
    p.add_argument("--capped", action="store_true", help="the requests for alarms under --max-priority")
    p.add_argument("--set", help="the set of requests by name, as run.py --set names them")
    p.add_argument("--server-arg", action="append", default=[], help="an argument for the server, such as "
                   "--max-priority=high")
    p.add_argument("--jobs", type=int, default=4)
    p.add_argument("--model", default="claude-sonnet-5-5")
    p.add_argument("--only", default="")
    p.add_argument("--binary", default="pushover-mcp-rs", help="the pushover-mcp-rs to measure")
    a = p.parse_args()
    import cases as catalogue
    import run
    name = a.set or ("held" if a.held else "capped" if a.capped else "cases")
    pool = run.pools(catalogue)[name]
    chosen = [c for c in pool if not c["context"] and (not a.only or re.search(a.only, c["key"]))]
    WORK.mkdir(exist_ok=True)
    (WORK / f"{a.tag}.pool").write_text(name)
    texts_file = WORK / f"{a.tag}.texts.json"
    texts_file.write_text(Path(a.texts).read_text() if a.texts else "{}")
    api_port = run.free_port()
    fake = subprocess.Popen([sys.executable, "-I", str(HERE / "fakeapi.py"), str(api_port), str(WORK / f"{a.tag}.api.jsonl")])
    try:
        run.wait_for(api_port)
        with concurrent.futures.ThreadPoolExecutor(a.jobs) as pool_:
            results = list(pool_.map(lambda c: ask(c, texts_file, api_port, a.model, a.binary, a.server_arg),
                                     chosen))
    finally:
        fake.terminate()
        fake.wait()
    # One turn per request: number the requests, and their calls with them.
    with open(WORK / f"{a.tag}.jsonl", "w") as rows, open(WORK / f"{a.tag}.calls.jsonl", "w") as calls:
        for n, (case, answer, found) in enumerate(results, 1):
            rows.write(json.dumps({"case": case["key"], "n": n, "turns": [n, n + 1], "answer": answer},
                                  ensure_ascii=False) + "\n")
            for call in found:
                calls.write(json.dumps({**call, "turn": n}, ensure_ascii=False) + "\n")
    catalogue.SENDER_RULE = "claude"
    run.score(a.tag, match_api=False)


main()
