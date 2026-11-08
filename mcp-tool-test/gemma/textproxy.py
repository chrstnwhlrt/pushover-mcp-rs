#!/usr/bin/env python3
"""The MCP server between the assistant and pushover-mcp-rs in the
measurement. Every message passes through; the texts of TEXTS.json take
the place of the server's, and every tool call is kept with its answer in
CALLS.jsonl, under the number of the model request it came in (TURNS, which
proxy.py counts).

    textproxy.py TEXTS.json CALLS.jsonl TURNS -- COMMAND [ARGS...]

TEXTS.json, every part optional:
  {"instructions": str,
   "flatten": true,             # $ref resolved, a oneOf of constants as a plain enum
   "tools": {name: {"name": shown name, "title": str, "description": str,
                    "params": {param: description},
                    "rename": {shown param: real param},
                    "hide": [param, ...],          # left out of the schema
                    "required": [param, ...]}}}    # in place of the server's list
A tool shown under another name is called by its own, a renamed parameter
by its own; a hidden one is left to the server's default.

"emulate" tries a change of the server before it has it:
  {"latest_receipt": true}   check_emergency_receipt and
      cancel_emergency_notification without receipt and tag take the receipt
      of the latest emergency notification
  {"default_sender": "Otomo"}   send_notification without sender sends as this
  {"device_check": [names]}   a device that is none of these is refused before
      it goes out, with the user's devices named (users/validate.json)
  {"sound_check": {name: display name}}   a sound given by its display name is
      taken by its name; one that is neither is refused with the sounds named
  {"max_priority": "normal", "refused": text}   a priority above it (by name or
      by Pushover's number) is refused with `refused`
  {"max_priority": "low", "downgrade": text}   a priority above it goes out
      with the cap instead, without retry, expire and tags, and the result
      gets `downgrade` (with {priority} and {cap} filled in) as its note;
      with "absent_is_normal": true a missing priority counts as normal

"hide_tools": [names] leaves tools out of the list; "enum_only": {tool: {param:
[values]}} keeps only these values of a parameter's enum.
"""
import copy
import json
import subprocess
import sys
import threading

TEXTS, CALLS, TURNS = sys.argv[1:4]
COMMAND = sys.argv[sys.argv.index("--") + 1:]
texts = json.loads(open(TEXTS).read() or "{}")
shown_to_real = {spec.get("name", name): name for name, spec in texts.get("tools", {}).items()}
real_to_shown = {real: shown for shown, real in shown_to_real.items()}
pending = {}
LOCK = threading.Lock()
WRITE = threading.Lock()
emulate = texts.get("emulate", {})
latest = {"receipt": None}


def turn():
    try:
        return int(open(TURNS).read().split()[0])
    except (OSError, ValueError, IndexError):
        return 0


def resolve(node, defs):
    """`node` with every local $ref replaced by its definition, and a oneOf
    of constants as a plain enum whose values the description names."""
    if isinstance(node, list):
        return [resolve(n, defs) for n in node]
    if not isinstance(node, dict):
        return node
    if "$ref" in node:
        target = copy.deepcopy(defs[node["$ref"].rsplit("/", 1)[-1]])
        node = {**{k: v for k, v in node.items() if k != "$ref"}, **target}
    node = {k: resolve(v, defs) for k, v in node.items() if k != "$defs"}
    if "oneOf" in node and all(isinstance(v, dict) and "const" in v for v in node["oneOf"]):
        values = node.pop("oneOf")
        node["type"] = "string"
        node["enum"] = [v["const"] for v in values]
        named = "; ".join(f"{v['const']}: {v['description']}" for v in values if v.get("description"))
        node["description"] = " ".join(filter(None, [node.get("description"), named]))
    if "anyOf" in node:
        options = node["anyOf"]
        concrete = [o for o in options if o.get("type") != "null"]
        if len(concrete) == 1 and len(options) == 2:
            node.pop("anyOf")
            inner = concrete[0]
            description = " ".join(filter(None, [node.get("description"), inner.get("description")]))
            node = {**inner, **{k: v for k, v in node.items() if k != "description"}}
            if description:
                node["description"] = description
    return node


LEVELS = {"silent": -2, "low": -1, "normal": 0, "high": 1, "emergency": 2}


def level(priority):
    """A priority by name or by Pushover's number, as a number; None where it is neither."""
    if isinstance(priority, int):
        return priority
    if isinstance(priority, str):
        return LEVELS.get(priority.strip().lower(), int(priority) if priority.strip().lstrip("-").isdigit() else None)
    return None


def shown_tools(tools):
    out = []
    hidden = set(texts.get("hide_tools", []))
    for tool in tools:
        if tool["name"] in hidden:
            continue
        spec = texts.get("tools", {}).get(tool["name"], {})
        tool = copy.deepcopy(tool)
        schema = tool.get("inputSchema", {})
        if texts.get("flatten"):
            schema = resolve(schema, schema.get("$defs", {}))
        props = schema.get("properties", {})
        for param in spec.get("hide", []):
            props.pop(param, None)
            if param in schema.get("required", []):
                schema["required"].remove(param)
        for shown, real in spec.get("rename", {}).items():
            if real in props:
                props[shown] = props.pop(real)
                schema["required"] = [shown if r == real else r for r in schema.get("required", [])]
        for param, description in spec.get("params", {}).items():
            if param in props:
                props[param]["description"] = description
        for param, values in texts.get("enum_only", {}).get(tool["name"], {}).items():
            if param in props and "enum" in props[param]:
                props[param]["enum"] = [v for v in props[param]["enum"] if v in values]
        if "required" in spec:
            schema["required"] = spec["required"]
        tool["inputSchema"] = schema
        for key in ("description", "title"):
            if key in spec:
                tool[key] = spec[key]
        tool["name"] = spec.get("name", tool["name"])
        out.append(tool)
    return out


def from_client(child):
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            child.stdin.write(line)
            child.stdin.flush()
            continue
        if "id" in message and "method" in message:
            entry = {"method": message["method"]}
            if message["method"] == "tools/call":
                params = message.setdefault("params", {})
                shown = params.get("name")
                real = shown_to_real.get(shown, shown)
                arguments = dict(params.get("arguments") or {})
                rename = texts.get("tools", {}).get(real, {}).get("rename", {})
                params["name"] = real
                params["arguments"] = {rename.get(k, k): v for k, v in arguments.items()}
                entry.update(turn=turn(), name=real, shown=shown, arguments=arguments)
                given = params["arguments"]
                if (emulate.get("latest_receipt") and real in ("check_emergency_receipt", "cancel_emergency_notification")
                        and not given.get("receipt") and not given.get("tag") and latest["receipt"]):
                    given["receipt"] = latest["receipt"]
                if emulate.get("default_sender") and real == "send_notification" and not str(given.get("sender") or "").strip():
                    given["sender"] = emulate["default_sender"]
                cap = emulate.get("max_priority")
                if cap and emulate.get("absent_is_normal") and real == "send_notification" and given.get("priority") is None:
                    # As the server does: no priority counts as normal, which a quiet cap lowers too.
                    given["priority"] = "normal"
                asked = level(given.get("priority"))
                if cap and emulate.get("downgrade") and real == "send_notification" and asked is not None and asked > LEVELS[cap]:
                    entry["downgraded"] = given.get("priority")
                    given["priority"] = cap
                    for field in ("retry", "expire", "tags"):
                        given.pop(field, None)
                elif cap and real == "send_notification" and asked is not None and asked > LEVELS[cap]:
                    text = emulate["refused"].format(priority=given.get("priority"), cap=cap)
                    reply = {"jsonrpc": "2.0", "id": message["id"],
                             "result": {"content": [{"type": "text", "text": text}], "isError": True}}
                    with open(CALLS, "a") as f:
                        f.write(json.dumps({**{k: entry[k] for k in ("turn", "name", "shown", "arguments")},
                                            "error": True, "text": text, "structured": None}, ensure_ascii=False) + "\n")
                    send(reply)
                    continue
                sounds = emulate.get("sound_check")
                sound = str(given.get("sound") or "").strip()
                if sounds and real == "send_notification" and sound and sound not in sounds:
                    by_display = {d.lower(): n for n, d in sounds.items()} | {n.lower(): n for n in sounds}
                    if sound.lower() in by_display:
                        given["sound"] = by_display[sound.lower()]
                    else:
                        named = ", ".join(f"{n} ({d})" for n, d in sounds.items())
                        text = (f"Unknown sound {sound!r}. The sounds are {named}. Send again with the name that "
                                "fits what the user asked for, or omit sound for the user's default.")
                        reply = {"jsonrpc": "2.0", "id": message["id"],
                                 "result": {"content": [{"type": "text", "text": text}], "isError": True}}
                        with open(CALLS, "a") as f:
                            f.write(json.dumps({**{k: entry[k] for k in ("turn", "name", "shown", "arguments")},
                                                "error": True, "text": text, "structured": None}, ensure_ascii=False) + "\n")
                        send(reply)
                        continue
                known = emulate.get("device_check")
                unknown = [d.strip() for d in str(given.get("device") or "").split(",") if d.strip() and d.strip() not in (known or [])]
                if known and real == "send_notification" and unknown:
                    text = (f"Unknown device {unknown[0]!r}. The user's devices are {', '.join(known)}: send again with "
                            "the one the user means (a phone, a laptop: tell it by its name), or omit device to reach "
                            "all of them.")
                    reply = {"jsonrpc": "2.0", "id": message["id"],
                             "result": {"content": [{"type": "text", "text": text}], "isError": True}}
                    with open(CALLS, "a") as f:
                        f.write(json.dumps({**{k: entry[k] for k in ("turn", "name", "shown", "arguments")},
                                            "error": True, "text": text, "structured": None}, ensure_ascii=False) + "\n")
                    send(reply)
                    continue
            if message["method"] == "tools/call":
                entry["sent"] = dict(message["params"].get("arguments") or {})
            with LOCK:
                pending[message["id"]] = entry
        child.stdin.write(json.dumps(message, ensure_ascii=False) + "\n")
        child.stdin.flush()
    child.stdin.close()


def to_client(child):
    for line in child.stdout:
        try:
            message = json.loads(line)
        except ValueError:
            sys.stdout.write(line)
            sys.stdout.flush()
            continue
        with LOCK:
            entry = pending.pop(message.get("id"), None) if "id" in message and "method" not in message else None
        result = message.get("result")
        if entry and isinstance(result, dict):
            if entry["method"] == "tools/list" and "tools" in result:
                result["tools"] = shown_tools(result["tools"])
            elif entry["method"] in ("server/discover", "initialize") and texts.get("instructions") is not None:
                result["instructions"] = texts["instructions"]
        if entry and entry["method"] == "tools/call":
            content = (result or {}).get("content") or []
            if entry.get("downgraded") and result and not result.get("isError") and content:
                note = emulate["downgrade"].format(priority=entry["downgraded"], cap=emulate["max_priority"])
                # As the server would put it: a field of its JSON result, which the text repeats.
                try:
                    shown_result = json.loads(content[0].get("text", ""))
                except ValueError:
                    shown_result = None
                if isinstance(shown_result, dict):
                    content[0]["text"] = json.dumps({**shown_result, "note": note}, ensure_ascii=False,
                                                    separators=(",", ":"), sort_keys=True)
                else:
                    content[0]["text"] = content[0].get("text", "") + " " + note
                if isinstance(result.get("structuredContent"), dict):
                    result["structuredContent"]["note"] = note
            receipt = ((result or {}).get("structuredContent") or {}).get("receipt")
            if receipt:
                latest["receipt"] = receipt
            row = {k: entry[k] for k in ("turn", "name", "shown", "arguments")}
            row["sent"] = entry.get("sent")
            row.update(error=bool((result or {}).get("isError")) or "error" in message,
                       text=" ".join(c.get("text", "") for c in content if isinstance(c, dict)) or
                       json.dumps(message.get("error"), ensure_ascii=False),
                       structured=(result or {}).get("structuredContent"))
            with open(CALLS, "a") as f:
                f.write(json.dumps(row, ensure_ascii=False) + "\n")
        send(message)


def send(message):
    """`message` to the assistant; the answers of the server and the refusals of the proxy come from two threads."""
    with WRITE:
        sys.stdout.write(json.dumps(message, ensure_ascii=False) + "\n")
        sys.stdout.flush()


def main():
    child = subprocess.Popen(COMMAND, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    reader = threading.Thread(target=from_client, args=(child,), daemon=True)
    reader.start()
    to_client(child)
    child.wait()


main()
