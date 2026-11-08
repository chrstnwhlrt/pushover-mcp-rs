# Changelog

## 2.0.0 (2026-11-09)

### Changed

- `send_notification` requires `sender`, the model's own name with the
  session or project it works in, e.g. "Claude (shop backend)". It starts
  the title (`sender: title`, without a title the sender alone), so
  notifications of parallel sessions can be told apart. The sender has at
  most 64 characters, sender and title together at most 250.
- The texts the model reads (instructions and the descriptions of
  `send_notification`, `sender`, `device`, `sound` and the two emergency
  tools) were measured with Gemma 4 E4B in otomo. With the texts of 1.0.0
  it got 62 of 96 spoken requests right, now 96 of 96; held-out requests
  went from 24 of 39 (three runs) to 25 of 26 (two runs), new ones written
  only at the end from 8 of 18 to 10 of 12. Claude Sonnet 5.5 stays at all
  right (26 of 26 and 11 of 11).
- Input schemas are flat: no `$ref` into `$defs`, `priority` as a plain enum
  whose values the description names. Small models lost the referenced
  definitions and sent `"priority": 1`; Pushover's numbers -2 to 2 are now
  accepted, anything else gets "priority must be one of silent, low,
  normal, high, emergency".
- Dependencies on their newest versions: rmcp 3.9 (from 0.9), reqwest 0.13,
  dirs 7, tokio 1.54, clap 4.6, schemars 1.2. The minimum Rust version is
  1.89, which the newest uuid (through rmcp) needs. Features no code uses are
  switched off (rmcp's base64, reqwest's HTTP/2, charset and JSON helpers,
  the ANSI colors of tracing-subscriber). The tree still grew from 146 to
  166 crates, through what rmcp 3.9 itself needs, the attachment upload and
  the new TLS setup.
- TLS checks certificates against the system's CA store (reqwest 0.13's
  default, rustls with aws-lc-rs) instead of Mozilla roots built into the
  binary. A system without CA certificates needs `SSL_CERT_FILE`.
- The server negotiates the MCP protocol version instead of answering every
  client with 2024-11-05: versions 2024-11-05 to 2025-11-25 through
  `initialize`, 2026-07-28 through `server/discover`.
- A failure is a tool error (`isError: true`) with a message for the model,
  also when Pushover rejects a request. Results no longer carry `success`,
  and only `cancel_emergency_notification` keeps a `message`.
  `send_notification` returns `request_id` and, for emergency priority,
  `receipt`; `list_devices` adds `group`.
- `sound` takes any sound name, so the custom sounds of the account work.
  Before, only the built-in names passed.
- `check_emergency_receipt` returns `acknowledged_by_device`,
  `acknowledged_at`, `expires_at` and `last_delivered_at`, times in RFC 3339.
- The config file is read on every call, so a token added after the first
  start works without restarting the client. Its errors name the file.
- The URL limit counts 512 characters, as the API documentation says, not
  bytes. `ttl` must be at least 1 and is not sent for emergency priority,
  which ignores it. `device` names are checked (1 to 25 letters, digits,
  `_`, `-`); several are joined with commas.
- Tools have titles and MCP annotations (read-only, destructive,
  idempotent). The server announces title, description and website.

### Added

- `--max-priority LEVEL`, by default `normal`: the loudest priority a client
  may send. `high` rings through quiet hours and `emergency` repeats until
  acknowledged, so a client sends them only when started with
  `--max-priority high` or `emergency`, and a model cannot ring through the
  night unasked. A louder request goes out with the limit, without retry,
  expire and tags; a request without a priority counts as normal. The
  result then carries a `note` telling the model what to say ("Sent as an
  ordinary notification, with normal priority instead of high: ..."). Below
  `emergency` the two emergency tools and the parameter tags are gone.
  Measured with Gemma 4 E4B in otomo in three runs: honest answers in 27
  of 27 at the limit normal, 24 of 24 at high, 12 of 12 at low and 9 of 9
  at silent, while the ordinary requests stay at 48 of 48 (two runs) under
  the default limit. Hiding the louder values got 8 of 24, a refusal 19 of
  24. A note that starts with how the notification went out took the loud
  requests from 17 of 20 to 38 of 39. retry and expire stay visible: with
  them hidden, a request for an alarm repeated every five minutes read as
  one to watch (0 of 6, 6 of 6 with them). This changes clients started
  without the option, which could send every priority before.
- `check_emergency_receipt` and `cancel_emergency_notification` without a
  receipt (and, for canceling, without a tag) act on the latest emergency
  notification of the server process. A client that keeps only the spoken
  conversation has lost the receipt by the next question.
- `--sender NAME` fixes the sender for a client with a single identity, such
  as the voice assistant otomo; the parameter then disappears from the
  schema and the texts.
- Device and sound names are checked against the account (`users/validate.json`
  and `sounds.json`, fetched once and again for a missing name). The API
  sends a notification for an unknown device to all devices and drops an
  unknown sound without an error; now the model gets the account's names
  and sends again. Display names such as "Küchengong" become the sound's
  name, device names match without regard to case.
- `attachment_path` attaches a JPEG, PNG, GIF or WebP image of up to 5 MB.
  The type is read from the content, so a renamed file of another kind is
  refused; the local file name is not sent.
- `tags` on emergency notifications and `cancel_emergency_notification` with
  `tag`, which cancels all emergency notifications with that tag.
- `list_sounds`: built-in and custom sounds.
- `get_message_limits`: the monthly quota, shared by all applications of an
  account since May 2026.
- `--config <PATH>` for another config file, for example of a second account,
  and `--api-url`, which the tests point at a fake API.
- At most two concurrent connections to the API and receipt polls at least
  five seconds apart, as Pushover asks of clients.
- `flake.nix` with package, app, dev shell (with cargo-audit and
  cargo-machete) and checks for build, tests, clippy and rustfmt.
- `mcp-tool-test/gemma`, the measurement kit behind these numbers. It puts
  the real server in front of Gemma 4 E4B through the voice assistant
  otomo and answers with a stand-in of the Pushover API, so nothing reaches
  a phone. Its README (in German) says how to run it.
- Tests: unit tests and end-to-end tests that run the binary over stdio
  against a fake Pushover API on 127.0.0.1.

### Fixed

- `check_emergency_receipt` reported the user key of whoever acknowledged as
  device name (`acknowledged_by` is the user key) and so handed the private
  key to the model.
- When the API could not be reached, the error message of
  `check_emergency_receipt` contained the request URL with the API token.
  Error messages and logs no longer contain token or user key.
- The receipt went into the URL path unchecked; now only letters and digits
  pass.
- A line on stdin that is not JSON ended the session (rmcp 0.9).
- A new config file was readable by everyone (umask); it is now created with
  mode 0600.
- A config file that is not valid JSON made every tool report a missing
  token. The tools now report the parse error.

## 1.0.0 (2025-11-30)

- First release.
