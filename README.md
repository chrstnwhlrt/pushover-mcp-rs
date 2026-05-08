# pushover-mcp-rs

A high-performance [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) server for sending push notifications via the [Pushover API](https://pushover.net/api). Built in Rust for speed and reliability.

## Overview

This server enables LLMs to send push notifications to your devices through the Pushover service. Perfect for getting notified when long-running tasks complete, receiving alerts, or any scenario where an AI assistant needs to get your attention.

## Features

- **Simple notifications** - Send messages with just a single parameter
- **Rich formatting** - HTML or monospace text formatting
- **Priority levels** - Named levels: silent, low, normal, high, emergency
- **Emergency management** - Check receipt status and cancel emergency notifications
- **Device targeting** - Send to specific devices when needed
- **Supplementary URLs** - Attach links to notifications
- **Custom sounds** - Discover and use built-in or account-specific Pushover sounds
- **Auto-configuration** - Config file created automatically on first run
- **Startup sound cache** - Loads supported sounds when credentials are configured

## Quick Start

### 1. Build

```bash
cargo build --release
```

### 2. Configure

On first run, a config file is created automatically. Edit it to add your Pushover credentials:

- **Linux**: `~/.config/pushover-mcp-rs/config.json`
- **macOS**: `~/Library/Application Support/pushover-mcp-rs/config.json`
- **Windows**: `C:\Users\{user}\AppData\Roaming\pushover-mcp-rs\config.json`

```json
{
  "token": "your-pushover-api-token",
  "default_user_key": "your-user-key",
  "default_device": null
}
```

Get your API token and user key from [pushover.net](https://pushover.net/).

### 3. Run

```bash
./target/release/pushover-mcp-rs
```

The server communicates via JSON-RPC 2.0 over stdin/stdout.

## MCP Tools

The server provides 5 tools:

### Notifications

| Tool | Description |
|------|-------------|
| `send_notification` | Send a push notification (only `message` is required) |
| `list_sounds` | List built-in and custom notification sound identifiers |
| `list_devices` | List available device names for targeting specific devices |

### Emergency Management

| Tool | Description |
|------|-------------|
| `check_emergency_receipt` | Check if an emergency notification was acknowledged |
| `cancel_emergency_notification` | Cancel an emergency notification that is still retrying |

## Notification Parameters

The `send_notification` tool supports these parameters:

| Parameter | Required | Description |
|-----------|----------|-------------|
| `message` | **Yes** | The notification message (max 1024 characters) |
| `title` | No | Message title (max 250 characters, defaults to app name) |
| `priority` | No | One of: `silent`, `low`, `normal` (default), `high`, `emergency` |
| `sound` | No | Sound identifier from `list_sounds`, including built-in and custom sounds. Omit or pass an empty value to use the user's default sound. |
| `url` | No | Supplementary URL to include (max 512 bytes) |
| `url_title` | No | Title for the URL, requires `url` (max 100 characters) |
| `html` | No | Enable HTML formatting (b, i, u, a, font tags). Cannot use with `monospace` |
| `monospace` | No | Display message in monospace font. Cannot use with `html` |
| `device` | No | Target specific device (only use if explicitly requested) |
| `ttl` | No | Time until auto-deleted from device, in seconds. Unrelated to `expire`. |
| `timestamp` | No | Unix timestamp to show as sent time. Useful for delayed notifications. |
| `retry` | No | Emergency only: retry interval in seconds (min 30, default 60) |
| `expire` | No | Emergency only: how long to keep retrying in seconds (max 10800, default 3600). Unrelated to `ttl`. |

## Priority Levels

| Value | Behavior |
|-------|----------|
| `silent` | No alert generated. Message only visible when user opens the app. |
| `low` | Quiet notification. No sound but may show on screen. |
| `normal` | Default notification with sound and vibration (default if not specified). |
| `high` | Important: always plays sound and vibrates, even during quiet hours. |
| `emergency` | Critical: repeats every 60s for 1 hour until acknowledged. Returns receipt ID. |

## Notification Sounds

On startup, the server loads the supported sound list from Pushover using the configured application token. This includes Pushover's built-in sounds plus any custom sounds uploaded to the account that owns the token.

Use `list_sounds` to see the cached list. Pass `refresh: true` to reload after adding or removing custom sounds.

## MCP Client Integration

### Claude Code

Add to your Claude Code configuration using the CLI:

```bash
claude mcp add pushover -s user -- /path/to/pushover-mcp-rs
```

Or add directly to `~/.claude.json`:

```json
{
  "mcpServers": {
    "pushover": {
      "type": "stdio",
      "command": "/path/to/pushover-mcp-rs"
    }
  }
}
```

Verify with:

```bash
claude mcp list
```

Then use `/mcp` inside Claude Code to check the connection status.

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
    cmd: "/path/to/pushover-mcp-rs"
    args: []
```

Or add interactively:

```bash
goose configure
```

Select "Add Extension" -> "Command-line Extension" and enter the path to the binary.

Verify with:

```bash
goose info -v
```

### Claude Desktop

Add to your Claude Desktop config (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "pushover": {
      "command": "/path/to/pushover-mcp-rs"
    }
  }
}
```

### Generic MCP Client

The server uses JSON-RPC 2.0 over stdin/stdout:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./pushover-mcp-rs
```

## Usage Examples

### Simple Notification

```json
{
  "name": "send_notification",
  "arguments": {
    "message": "Build completed successfully!"
  }
}
```

### High Priority with Title

```json
{
  "name": "send_notification",
  "arguments": {
    "message": "Deployment to production finished",
    "title": "CI/CD Pipeline",
    "priority": "high"
  }
}
```

### Emergency Notification

```json
{
  "name": "send_notification",
  "arguments": {
    "message": "Server is down! Immediate attention required.",
    "title": "Critical Alert",
    "priority": "emergency",
    "retry": 60,
    "expire": 3600
  }
}
```

Returns a `receipt` ID for tracking acknowledgment.

### Check Emergency Status

```json
{
  "name": "check_emergency_receipt",
  "arguments": {
    "receipt": "receipt-id-from-send"
  }
}
```

### Cancel Emergency

```json
{
  "name": "cancel_emergency_notification",
  "arguments": {
    "receipt": "receipt-id-from-send"
  }
}
```

### With URL

```json
{
  "name": "send_notification",
  "arguments": {
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
    "message": "<b>Task Complete</b><br>Processed <i>1,234</i> records",
    "html": true
  }
}
```

### Monospace (Code)

```json
{
  "name": "send_notification",
  "arguments": {
    "message": "Error: connection refused\n  at main.rs:42",
    "monospace": true
  }
}
```

### Custom Sound

List the supported sound identifiers:

```json
{
  "name": "list_sounds",
  "arguments": {}
}
```

Send with a built-in or custom sound identifier:

```json
{
  "name": "send_notification",
  "arguments": {
    "message": "Timer finished!",
    "sound": "siren"
  }
}
```

### List Devices

```json
{
  "name": "list_devices",
  "arguments": {}
}
```

### Send to Specific Device

```json
{
  "name": "send_notification",
  "arguments": {
    "message": "Sent to your phone only",
    "device": "iphone"
  }
}
```

## Development

### Prerequisites

- Rust 1.85 or later (edition 2024)

### Build

```bash
cargo build --release
```

### Lint

```bash
cargo clippy --release -- -W clippy::all -W clippy::pedantic
```

### Debug Logging

```bash
RUST_LOG=pushover_mcp_rs=debug ./target/release/pushover-mcp-rs
```

## Version Management

Check version:

```bash
./pushover-mcp-rs --version
```

Release new version:

```bash
./set-version.sh 1.2.3
git push origin main --follow-tags
```

## Project Structure

```
src/
├── main.rs     # Entry point, CLI, MCP server initialization
├── server.rs   # MCP server and tool implementations
├── model.rs    # Data structures for parameters and responses
└── config.rs   # Configuration file management
```

## Troubleshooting

### "No Pushover API token configured"

- Edit the config file and add your API token
- Get your token from [pushover.net/apps](https://pushover.net/apps)

### "No user_key provided"

- Add `default_user_key` to your config file
- Get your user key from [pushover.net](https://pushover.net/)

### "Message cannot be empty"

- The `message` parameter is required and cannot be blank

### "Cannot use both html and monospace formatting"

- Choose one formatting option: either `html: true` or `monospace: true`

### Connection issues in Claude Code

- Run `/mcp` to check server status
- Verify the binary path is correct
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
