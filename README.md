# pushover-mcp-rs

An [MCP](https://modelcontextprotocol.io/) server that lets an LLM send push notifications to your devices through the [Pushover API](https://pushover.net/api).

## Overview

When a long task is done or something needs your decision, the assistant sends a notification to your phone, watch or desktop. Emergency notifications repeat until you acknowledge them, and the assistant can check and cancel them.

## Features

- **Sender in the title** - `send_notification` needs `message` and `sender`; the sender starts the title, so you see which session or project a notification comes from. `--sender NAME` sets it for a client with a single identity, such as a voice assistant
- **Priorities** - `silent`, `low`, `normal`, `high` and `emergency`; emergency notifications repeat until acknowledged and can be checked and canceled, the latest one without its receipt, others by receipt or by tag
- **Loud only when allowed** - `high` rings through quiet hours and `emergency` repeats until acknowledged, so a client sends them only with `--max-priority high` or `--max-priority emergency`. A louder request goes out with the loudest priority allowed, and the result tells the model to say so
- **Checked names** - device and sound names are checked against the account, display names like "Küchengong" included; the API itself would send to all devices or drop the sound without a word
- **Image attachments** - JPEG, PNG, GIF or WebP up to 5 MB from a local file
- **Formatting** - HTML or monospace text, a supplementary URL, a time to live and a custom message time
- **Sounds** - the built-in sounds and the custom sounds of your account (`list_sounds`)
- **Devices and quota** - target single devices (`list_devices`) and read the monthly message quota (`get_message_limits`)
- **API etiquette** - at most two concurrent connections and receipt polls at least five seconds apart, as Pushover asks
- **No secrets in the context** - the token, the user key and the encryption key appear neither in tool results nor in error messages or logs
- **Live config** - the config file is read on every call, so a token added after the first start works without restarting the client
- **End-to-end encryption** - with a key in the config, message, title and link leave the machine encrypted, and only Pushover's iOS and Android apps with the same key can read them

## Quick Start

### 1. Install

With Nix (flakes enabled), from a checkout:

```bash
nix profile add .
```

Updates: `git pull`, then `nix profile upgrade pushover-mcp-rs` and a restart of the MCP client. Without Nix, `cargo build --release` builds `./target/release/pushover-mcp-rs`.

### 2. Configure

The first start writes an empty config file, which only you can read on Linux and macOS:

- **Linux**: `~/.config/pushover-mcp-rs/config.json` (or under `$XDG_CONFIG_HOME`)
- **macOS**: `~/Library/Application Support/pushover-mcp-rs/config.json`
- **Windows**: `C:\Users\{user}\AppData\Roaming\pushover-mcp-rs\config.json`

```json
{
  "token": "your-application-token",
  "default_user_key": "your-user-key",
  "default_device": null,
  "encryption_key": null
}
```

| Key | Required | Meaning |
|-----|----------|---------|
| `token` | Yes | API token of a Pushover application, from [pushover.net/apps/build](https://pushover.net/apps/build) |
| `default_user_key` | Yes | Your user key (shown on [pushover.net](https://pushover.net/) after login) or a group key |
| `default_device` | No | Device, or several joined with commas, for calls that name none |
| `encryption_key` | No | Key for end-to-end encryption, 64 hexadecimal characters (see below) |

`--config <PATH>` selects another file, for example for a second Pushover account:

```bash
pushover-mcp-rs --config ~/.config/pushover-mcp-rs/work.json
```

`--sender NAME` fixes the sender for a client that always speaks as the same one, such as the voice assistant otomo. The model then neither sees nor fills the `sender` parameter:

```bash
pushover-mcp-rs --sender Otomo
```

`--max-priority LEVEL` is the loudest priority a client may send: `silent`, `low`, `normal` (the default), `high` or `emergency`, or the numbers -2 to 2. A request above it goes out with this priority, without the emergency settings (a request without a priority counts as normal), and the result carries a `note` for the model, such as "Sent as an ordinary notification, with normal priority instead of high: alarms are switched off for this client (--max-priority normal). Tell the user that it went out as an ordinary notification." Below `emergency` the two emergency tools and the parameter `tags` are gone. `retry` and `expire` stay, because without them a small model took a request for an alarm repeated every five minutes for one to watch; the server drops them when it lowers the priority. To allow alarms:

```bash
pushover-mcp-rs --max-priority emergency
```

`encryption_key` turns on Pushover's end-to-end encryption. The server encrypts `message`, the title (with the sender), `url` and `url_title` with this key before they leave the machine, as Pushover's API documentation describes it (gzip, AES-256-CBC, HMAC-SHA256), so Pushover's servers never see them. Pushover's iOS and Android apps decrypt them from version 5.0 with the same key, which you store in the app on each device; for the desktop and browser clients Pushover documents no end-to-end encryption. All other parameters, such as priority, sound and device, and an image attachment go out unencrypted. The encrypted values are longer than the text (a title of 17 characters becomes 128 characters), and Pushover does not say whether its length limits count them. To make a key:

```bash
openssl rand -hex 32
```

### 3. Run

```bash
pushover-mcp-rs
```

The server communicates via JSON-RPC 2.0 over stdin/stdout and logs to stderr. It speaks the MCP protocol versions 2024-11-05 to 2025-11-25 through `initialize`, each client in the version it asks for, and 2026-07-28 through `server/discover`.

## MCP Tools

### Notifications

| Tool | Description |
|------|-------------|
| `send_notification` | Send a notification; `message` and `sender` are required |
| `list_devices` | Names of the active devices, for the `device` parameter |
| `list_sounds` | Built-in sounds and the custom sounds of the account |
| `get_message_limits` | Monthly message quota: limit, messages left, next reset |

### Emergency Management

| Tool | Description |
|------|-------------|
| `check_emergency_receipt` | Whether an emergency notification was acknowledged (when, on which device) or expired: the latest one sent, or the one of `receipt` |
| `cancel_emergency_notification` | Stop the repetition of an emergency notification: the latest one sent, the one of `receipt`, or all with `tag` |

Both exist only with `--max-priority emergency`. "The latest one sent" is the latest emergency notification of this server process, so of this client. It spares the model the receipt, which a client that keeps only the spoken conversation has lost by the next question.

MCP annotations mark `send_notification` and `cancel_emergency_notification` as changing something and the other tools as read-only. Every result has an output schema; clients without structured output get the same JSON as text.

A failure is a tool result with `isError: true` and a message for the model: invalid input (checked before anything is sent), a missing token, Pushover rejecting the request (with its reason and request ID) or the API being unreachable.

## Notification Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `message` | **Yes** | The text, at most 1024 characters |
| `sender` | **Yes** (not with `--sender`) | Who sends: the model's own name, with the session or project it works in, e.g. `Claude (shop backend)`. At most 64 characters. Starts the title |
| `title` | No | Comes after the sender: `Claude (pushover mcp): Build finished`. Sender and title together at most 250 characters. Without a title, the sender alone is the title |
| `priority` | No | `silent`, `low`, `normal` (default), `high` or `emergency`; Pushover's numbers -2 to 2 work as well. Above `--max-priority` (default `normal`) the notification goes out with that priority and a `note` |
| `sound` | No | A built-in sound (`pushover`, `bike`, `bugle`, `cashregister`, `classical`, `cosmic`, `falling`, `gamelan`, `incoming`, `intermission`, `magic`, `mechanical`, `pianobar`, `siren`, `spacealarm`, `tugboat`, `alien`, `climb`, `persistent`, `echo`, `updown`, `vibrate`, `none`) or a custom sound of the account; default is the user's sound. The name is checked against the account's sounds, a display name like "Küchengong" becomes its name |
| `device` | No | Device name, or several joined with commas; default is all devices (or `default_device`). The names are checked against the account's devices, without regard to case |
| `url` | No | Supplementary URL, at most 512 characters |
| `url_title` | No | Link text for `url`, at most 100 characters |
| `html` | No | Render as HTML: `<b>`, `<i>`, `<u>`, `<font color="#rrggbb">`, `<a href="...">`. Not with `monospace` |
| `monospace` | No | Monospace font. Not with `html` |
| `attachment_path` | No | Absolute path of a JPEG, PNG, GIF or WebP image, at most 5 MB. The type is read from the file's content; the local file name is not sent |
| `ttl` | No | Seconds until the message is deleted from the devices; ignored for emergency priority |
| `timestamp` | No | Unix time shown as the message time, for an event that happened earlier |
| `retry` | No | Emergency only (`--max-priority emergency`): seconds between repetitions, at least 30, default 60 |
| `expire` | No | Emergency only: seconds of repetition, at most 10800, default 3600. Pushover stops after 50 repetitions |
| `tags` | No | Emergency only: tags (without commas or slashes) for canceling with `cancel_emergency_notification` |

The result holds `request_id` (Pushover's ID for support inquiries), for emergency priority the `receipt`, and a `note` when the priority was lowered to `--max-priority`.

## Priority Levels

| Value | Behavior |
|-------|----------|
| `silent` | No alert; the message only shows up in the Pushover app |
| `low` | Shown without sound or vibration |
| `normal` | Sound and vibration as set on the device (the default); quiet hours apply |
| `high` | Bypasses quiet hours, always plays a sound and vibrates. Needs `--max-priority high` or `emergency` |
| `emergency` | Like `high`, repeated every 60 s for an hour (unless `retry`/`expire` say otherwise) until acknowledged. Returns a receipt. Needs `--max-priority emergency` |

## MCP Client Integration

### Claude Code

Add to your Claude Code configuration using the CLI:

```bash
claude mcp add pushover -s user -- pushover-mcp-rs
```

Claude may then send up to `normal`. To let it raise alarms, add `--max-priority emergency` (or `high`) after the command.

Or add directly to `~/.claude.json`:

```json
{
  "mcpServers": {
    "pushover": {
      "type": "stdio",
      "command": "pushover-mcp-rs"
    }
  }
}
```

Verify with `claude mcp list`, then use `/mcp` inside Claude Code to check the connection status.

### Goose CLI

Add to `~/.config/goose/config.yaml`:

```yaml
extensions:
  pushover:
    bundled: false
    display_name: "Pushover"
    enabled: true
    name: "pushover"
    timeout: 300
    type: "stdio"
    cmd: "pushover-mcp-rs"
    args: []
```

Or add it interactively with `goose configure`: "Add Extension" -> "Command-line Extension", then the command `pushover-mcp-rs`. Verify with `goose info -v`.

### Claude Desktop

Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "pushover": {
      "command": "/absolute/path/to/pushover-mcp-rs"
    }
  }
}
```

Claude Desktop starts the command without your shell, so the `PATH` entries of your shell profile are missing: give the absolute path, with Nix `/Users/<you>/.nix-profile/bin/pushover-mcp-rs`.

### Generic MCP Client

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"cli","version":"0"}}}' | pushover-mcp-rs
```

## Usage Examples

### Simple Notification

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Build completed successfully"
  }
}
```

The notification shows the title `Claude (pushover mcp)`; with `"title": "Build"` it would be `Claude (pushover mcp): Build`.

### High Priority with Title

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Deployment to production finished",
    "title": "CI/CD Pipeline",
    "priority": "high"
  }
}
```

### Emergency Notification

Needs a server started with `--max-priority emergency`; otherwise it goes out with the loudest priority allowed and a `note`.

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Server mail01 is down",
    "title": "Critical Alert",
    "priority": "emergency",
    "retry": 60,
    "expire": 3600,
    "tags": ["mail01", "chicago"]
  }
}
```

Returns `{"request_id": "...", "receipt": "..."}`.

### Check an Emergency Notification

The latest one sent:

```json
{
  "name": "check_emergency_receipt",
  "arguments": {}
}
```

Or one by its receipt:

```json
{
  "name": "check_emergency_receipt",
  "arguments": {
    "receipt": "KAWxmB7eX5spZ1aR3H4hQNa5jxzq5j"
  }
}
```

Returns for example `{"acknowledged": true, "acknowledged_at": "2026-11-08T14:03:11Z", "acknowledged_by_device": "iphone", "expired": false, "expires_at": "2026-11-08T15:02:40Z", "last_delivered_at": "2026-11-08T14:02:40Z"}`.

### Cancel Emergency Notifications

The latest one sent, with `"arguments": {}`, or one by its receipt:

```json
{
  "name": "cancel_emergency_notification",
  "arguments": {
    "receipt": "KAWxmB7eX5spZ1aR3H4hQNa5jxzq5j"
  }
}
```

All that were sent with a tag:

```json
{
  "name": "cancel_emergency_notification",
  "arguments": {
    "tag": "chicago"
  }
}
```

### Image Attachment

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Benchmark results attached",
    "attachment_path": "/home/user/project/bench.png"
  }
}
```

### With URL

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "New PR ready for review",
    "url": "https://github.com/org/repo/pull/123",
    "url_title": "View Pull Request"
  }
}
```

### HTML Formatting

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "<b>Task complete</b>\nProcessed <i>1,234</i> records",
    "html": true
  }
}
```

### Monospace (Code)

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Error: connection refused\n  at main.rs:42",
    "monospace": true
  }
}
```

### Custom Sound

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Timer finished",
    "sound": "siren"
  }
}
```

`list_sounds` returns the names, including the custom sounds of the account:

```json
{"sounds": [{"name": "bike", "description": "Bike"}, {"name": "my_bell", "description": "My Bell"}]}
```

### Send to Specific Devices

```json
{
  "name": "send_notification",
  "arguments": {
    "sender": "Claude (pushover mcp)",
    "message": "Sent to two devices only",
    "device": "iphone,pixel_8"
  }
}
```

`list_devices` returns the names: `{"devices": ["iphone", "pixel_8"], "group": false}`. Pushover ignores `device` for delivery groups outside Pushover for Teams.

### Message Quota

`get_message_limits` returns `{"limit": 10000, "remaining": 7496, "reset_at": "2026-12-01T06:00:00Z"}`. Since May 2026 all applications of an account share the quota.

## Development

### Prerequisites

- Rust 1.89 or later (edition 2024), or Nix

### Build and check

```bash
nix develop                          # toolchain with rust-analyzer, clippy, rustfmt, cargo-audit, cargo-machete
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets -- -W clippy::pedantic
cargo audit                          # known vulnerabilities
cargo machete                        # unused dependencies
nix build                            # release binary in ./result/bin
nix flake check                      # build, tests, clippy and fmt as Nix checks
```

The tests in `tests/stdio.rs` run the binary over stdio against a fake Pushover API on 127.0.0.1 (`--api-url`), so they need neither network nor account, also in the Nix build sandbox.

### Measuring the tool texts

`mcp-tool-test/gemma` measures whether Gemma 4 E4B, behind the voice assistant otomo, calls the tools right and tells the user what really went out. The real server answers, against a stand-in of the Pushover API, so nothing reaches a phone. The measurement needs a checkout of otomo with its release build, Ollama with Gemma 4 E4B and a built pushover-mcp-rs; `captest.py`, which checks the priority limits without a model, needs only the binary. The README there (in German) has the details.

### Comparing a build with the installed release

`mcp-tool-test/compare` shows whether a build answers byte for byte like the installed release. `record` runs a binary against the measurement's stand-in of the Pushover API with a fixed clock: `initialize` for every protocol version, `tools/list`, `server/discover`, the tool calls in `calls.jsonl` (what Gemma sent during the measurements) and ten further requests (faulty tool calls, `ping`, other MCP methods and an unknown one), each under every `--max-priority` with and without `--sender`. `diff` compares two recordings. A row that differs only in the version counts as equal, any other difference is shown and fails. A recording takes about five seconds and needs only Python and the binary.

```bash
nix build
cd mcp-tool-test/compare
python3 -I compare.py record "$(command -v pushover-mcp-rs)" work/installed.jsonl
python3 -I compare.py record ../../result/bin/pushover-mcp-rs work/new.jsonl
python3 -I compare.py diff work/installed.jsonl work/new.jsonl
```

`compare.py calls` adds the calls of later measurement runs to `calls.jsonl`; record both sides again after it.

### Debug Logging

The server logs warnings and its own start and sent notifications on stderr. More detail:

```bash
RUST_LOG=pushover_mcp_rs=debug pushover-mcp-rs
```

## Version Management

Check version:

```bash
pushover-mcp-rs --version
```

Release new version:

```bash
./set-version.sh 1.2.3
git push origin main --follow-tags
nix profile upgrade pushover-mcp-rs
```

`set-version.sh` sets the version in `Cargo.toml`, which `flake.nix` reads, and creates commit and tag. Before pushing, compare the new build (`nix build`) with the installed release, see [Comparing a build with the installed release](#comparing-a-build-with-the-installed-release).

## Project Structure

```
src/
├── main.rs        # CLI, logging, config bootstrap, stdio transport
├── server.rs      # MCP tools, input checks, server metadata
├── pushover.rs    # Pushover API client: endpoints, connection limit, error mapping
├── model.rs       # Tool parameters and results, Pushover API responses
├── schema.rs      # Flat input schemas, as small models read them best
├── attachment.rs  # Image attachments: size and type checks
├── encryption.rs  # End-to-end encryption of the message fields
└── config.rs      # Config file: location, template, loading
tests/
└── stdio.rs       # The binary over stdio against a fake Pushover API
mcp-tool-test/
├── gemma/         # Measurement of the tool texts with Gemma 4 E4B (Python)
└── compare/       # Byte comparison of a build with a release (Python)
```

## Troubleshooting

### "No Pushover API token configured" or "No Pushover user key configured"

- Add `token` and `default_user_key` to the config file named in the message; no restart is needed
- Tokens come from [pushover.net/apps](https://pushover.net/apps); the user key is on [pushover.net](https://pushover.net/) after login

### "Pushover rejected the request (HTTP 400 ...): application token is invalid"

- The `token` in the config file is wrong. Pushover's reason follows the colon; the request ID helps Pushover's support

### "Pushover refused the request (HTTP 429 ...)"

- The monthly message quota is used up; `get_message_limits` shows when it resets

### "The encryption_key in ... is not a 256-bit key of 64 hexadecimal characters"

- The key has exactly 64 hexadecimal characters (0-9, a-f), as `openssl rand -hex 32` prints it. Without the line in the config file, notifications go out without end-to-end encryption

### "Invalid device name" or "Unknown device"

- Device names have 1 to 25 letters, digits, `_` or `-`; `list_devices` shows them
- "Unknown device" lists the account's devices; the model sends again with the right one or without `device`

### "Unknown sound"

- The message lists the account's sounds with their display names; the model sends again with the right name

### "The attachment ... is not a JPEG, PNG, GIF or WebP image"

- Only these image types are sent; the check reads the file's content, not its name

### "Could not set up the HTTPS client: ... No CA certificates were loaded from the system"

- The server uses the system's CA certificates. Install them (`ca-certificates`) or point `SSL_CERT_FILE` at a CA bundle

### Connection issues in Claude Code

- Run `/mcp` to check server status
- Verify that `pushover-mcp-rs` is on the `PATH` of Claude Code, or configure the absolute path
- Check logs: `RUST_LOG=pushover_mcp_rs=debug` for verbose output

### Connection issues in Goose

- Run `goose info -v` to verify configuration
- Increase timeout if the server is slow to start

## License

MIT License. See [LICENSE](LICENSE) for details.

## Author

Christian Wohlert

## Related Projects

- **appbase-cloud-log-mcp-rs** - MCP server for analyzing Kubernetes log files
- **ro-mongodb-mcp-rs** - Read-only MongoDB MCP server
