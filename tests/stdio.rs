//! End-to-end tests over stdio: the real binary against a fake Pushover API
//! on 127.0.0.1. Nothing here talks to the network.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Example token and user key from the Pushover API documentation.
const TOKEN: &str = "azGDORePK8gMaC0QOYAMyEEuzJnyUi";
const USER: &str = "uQiRzpo4DXghDmr9QzzfQu27cmVRsG";
const RECEIPT: &str = "KAWxmB7eX5spZ1aR3H4hQNa5jxzq5j";
/// What a model names as sender of its notifications.
const SENDER: &str = "Claude (stdio test)";
/// A key for end-to-end encryption, as `openssl rand -hex 32` makes one.
const E2EE_KEY: &str = "5f1c0e9a8b7d6c4e3a2f1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e";

/// The protocol versions the server supports, oldest first.
const PROTOCOL_VERSIONS: [&str; 5] = [
    "2024-11-05",
    "2025-03-26",
    "2025-06-18",
    "2025-11-25",
    "2026-07-28",
];

const TOOLS: [&str; 6] = [
    "cancel_emergency_notification",
    "check_emergency_receipt",
    "get_message_limits",
    "list_devices",
    "list_sounds",
    "send_notification",
];

// --- Fake Pushover API ---

/// A request the fake API received.
#[derive(Clone, Debug)]
struct Recorded {
    method: String,
    /// Path and query
    target: String,
    content_type: String,
    body: Vec<u8>,
}

impl Recorded {
    fn path(&self) -> &str {
        self.target.split('?').next().unwrap()
    }

    fn query(&self) -> Vec<(String, String)> {
        parse_urlencoded(self.target.split_once('?').map_or("", |(_, query)| query))
    }

    /// The fields of a URL-encoded or multipart body.
    fn fields(&self) -> Vec<(String, String)> {
        if self.content_type.starts_with("multipart/form-data") {
            multipart_parts(self)
                .into_iter()
                .filter(|part| part.file_name.is_none())
                .map(|part| (part.name, String::from_utf8(part.data).unwrap()))
                .collect()
        } else {
            parse_urlencoded(std::str::from_utf8(&self.body).unwrap())
        }
    }
}

type Responder = dyn Fn(&Recorded) -> (u16, String) + Send + Sync;

struct FakeApi {
    url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeApi {
    /// Answers like Pushover does for valid requests.
    fn start() -> Self {
        Self::start_with(pushover)
    }

    fn start_with(responder: impl Fn(&Recorded) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let responder: Arc<Responder> = Arc::new(responder);
        let recorded = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let recorded = Arc::clone(&recorded);
                let responder = Arc::clone(&responder);
                std::thread::spawn(move || serve(stream, &recorded, &*responder));
            }
        });
        Self { url, requests }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

/// Handles one request; every response closes the connection.
fn serve(stream: TcpStream, requests: &Mutex<Vec<Recorded>>, responder: &Responder) {
    stream.set_read_timeout(Some(TIMEOUT)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap().to_owned();
    let target = parts.next().unwrap().to_owned();

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    let body = if let Some(length) = headers.get("content-length") {
        let mut body = vec![0; length.parse().unwrap()];
        reader.read_exact(&mut body).unwrap();
        body
    } else if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        read_chunked(&mut reader)
    } else {
        Vec::new()
    };

    let recorded = Recorded {
        method,
        target,
        content_type: headers.remove("content-type").unwrap_or_default(),
        body,
    };
    let (status, response) = responder(&recorded);
    // Recorded before the answer, so the test sees it with the tool result.
    requests.lock().unwrap().push(recorded);
    let _ = write!(
        writer,
        "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{response}",
        response.len()
    );
}

fn read_chunked(reader: &mut impl BufRead) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let mut size = String::new();
        reader.read_line(&mut size).unwrap();
        let size = usize::from_str_radix(size.trim(), 16).unwrap();
        let mut chunk = vec![0; size + 2];
        reader.read_exact(&mut chunk).unwrap();
        if size == 0 {
            return body;
        }
        body.extend_from_slice(&chunk[..size]);
    }
}

/// What Pushover answers to valid requests.
fn pushover(request: &Recorded) -> (u16, String) {
    let body = match (request.method.as_str(), request.path()) {
        ("POST", "/1/messages.json") => {
            let emergency = request
                .fields()
                .contains(&("priority".to_owned(), "2".to_owned()));
            if emergency {
                json!({"status": 1, "request": "req-emergency", "receipt": RECEIPT})
            } else {
                json!({"status": 1, "request": "req-message"})
            }
        }
        ("POST", "/1/users/validate.json") => json!({
            "status": 1, "group": 0, "devices": ["iphone", "pixel_8"],
            "licenses": ["iOS", "Android"], "request": "req-validate",
        }),
        ("GET", "/1/sounds.json") => json!({
            "sounds": {"pushover": "Pushover (default)", "siren": "Siren", "my_bell": "My Bell"},
            "status": 1, "request": "req-sounds",
        }),
        ("GET", "/1/apps/limits.json") => json!({
            "limit": 10000, "remaining": 7496, "reset": 1_393_653_600_i64,
            "status": 1, "request": "req-limits",
        }),
        ("GET", path) if path == format!("/1/receipts/{RECEIPT}.json") => json!({
            "status": 1, "acknowledged": 1, "acknowledged_at": 1_360_019_238_i64,
            "acknowledged_by": USER, "acknowledged_by_device": "iphone",
            "last_delivered_at": 1_360_019_178_i64, "expired": 0,
            "expires_at": 1_360_022_778_i64, "called_back": 0, "called_back_at": 0,
            "request": "req-receipt",
        }),
        ("POST", path) if path.starts_with("/1/receipts/cancel_by_tag/") => {
            json!({"status": 1, "canceled": 2, "request": "req-cancel-tag"})
        }
        ("POST", path) if path == format!("/1/receipts/{RECEIPT}/cancel.json") => {
            json!({"status": 1, "request": "req-cancel"})
        }
        _ => {
            return (
                404,
                json!({"status": 0, "errors": ["not found"], "request": "req-404"}).to_string(),
            );
        }
    };
    (200, body.to_string())
}

fn parse_urlencoded(input: &str) -> Vec<(String, String)> {
    input
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(name), percent_decode(value))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => decoded.push(b' '),
            b'%' => {
                decoded.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            byte => decoded.push(byte),
        }
        i += 1;
    }
    String::from_utf8(decoded).unwrap()
}

struct Part {
    name: String,
    file_name: Option<String>,
    content_type: Option<String>,
    data: Vec<u8>,
}

fn multipart_parts(request: &Recorded) -> Vec<Part> {
    let boundary = request
        .content_type
        .split("boundary=")
        .nth(1)
        .unwrap()
        .trim_matches('"');
    let delimiter = format!("\r\n--{boundary}").into_bytes();
    // The body starts with the delimiter minus its line break.
    let mut body = b"\r\n".to_vec();
    body.extend_from_slice(&request.body);

    let mut parts = Vec::new();
    let mut rest = &body[..];
    while let Some(start) = find(rest, &delimiter) {
        rest = &rest[start + delimiter.len()..];
        if rest.starts_with(b"--") {
            break;
        }
        let end = find(rest, &delimiter).unwrap();
        let part = &rest[2..end]; // skip the line break after the delimiter
        let header_end = find(part, b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&part[..header_end]).unwrap();
        let attribute = |name: &str| {
            let key = format!("{name}=\"");
            headers
                .split(&key)
                .nth(1)
                .map(|value| value.split('"').next().unwrap().to_owned())
        };
        let content_type = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-type: "))
            .or_else(|| {
                headers
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Type: "))
            })
            .map(str::to_owned);
        parts.push(Part {
            name: attribute("name").unwrap(),
            file_name: attribute("filename"),
            content_type,
            data: part[header_end + 4..].to_vec(),
        });
    }
    parts
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// --- MCP client over stdio ---

struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
}

impl Server {
    /// A server that may send every priority, so all tools are there.
    fn start(config: &Path, api: &FakeApi) -> Self {
        Self::start_with_url(config, &api.url)
    }

    fn start_with_url(config: &Path, api_url: &str) -> Self {
        Self::start_with_args(config, api_url, &["--max-priority", "emergency"])
    }

    fn start_with_args(config: &Path, api_url: &str, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pushover-mcp-rs"))
            .arg("--config")
            .arg(config)
            .arg("--api-url")
            .arg(api_url)
            .args(args)
            .env_clear()
            .env("RUST_LOG", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("server starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        // A reader thread, so a server that stops answering fails the test
        // instead of hanging it.
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 1,
        }
    }

    fn send_line(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn send(&mut self, message: &Value) {
        self.send_line(&message.to_string());
    }

    /// Sends a request and returns the response that carries its id.
    fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(TIMEOUT)
                .unwrap_or_else(|_| panic!("no response to {method}"));
            let message: Value = serde_json::from_str(&line).expect("server writes JSON lines");
            if message["id"] == id {
                return message;
            }
        }
    }

    fn initialize(&mut self, protocol_version: &str) -> Value {
        let response = self.request(
            "initialize",
            &json!({
                "protocolVersion": protocol_version,
                "capabilities": {},
                "clientInfo": {"name": "stdio-test", "version": "0"},
            }),
        );
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        response
    }

    /// A server after the `initialize` handshake.
    fn ready(config: &Path, api: &FakeApi) -> Self {
        let mut server = Self::start(config, api);
        server.initialize("2025-11-25");
        server
    }

    fn tools(&mut self) -> Vec<Value> {
        let response = self.request("tools/list", &json!({}));
        response["result"]["tools"]
            .as_array()
            .expect("tools/list returns tools")
            .clone()
    }

    /// Calls a tool and returns its `CallToolResult`.
    fn call(&mut self, name: &str, arguments: &Value) -> Value {
        self.call_with(&json!({"name": name, "arguments": arguments}))
    }

    fn call_with(&mut self, params: &Value) -> Value {
        let response = self.request("tools/call", params);
        assert!(response.get("error").is_none(), "{params}: {response}");
        response["result"].clone()
    }

    /// Calls a tool that must succeed and returns its structured content,
    /// checked against the tool's output schema.
    fn call_ok(&mut self, name: &str, arguments: &Value) -> Value {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], false, "{name} {arguments}: {result}");
        let structured = result["structuredContent"].clone();

        let schema = self
            .tools()
            .into_iter()
            .find(|tool| tool["name"] == name)
            .unwrap()["outputSchema"]
            .clone();
        let validator = jsonschema::draft202012::new(&schema).expect("output schema compiles");
        let errors: Vec<String> = validator
            .iter_errors(&structured)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:?} in {structured}");

        // Clients without structured output read the same JSON as text.
        let text: Value = serde_json::from_str(text(&result)).expect("text content is JSON");
        assert_eq!(text, structured, "{name}");
        structured
    }

    /// Calls a tool that must fail and returns the error text.
    fn call_err(&mut self, name: &str, arguments: &Value) -> String {
        let result = self.call(name, arguments);
        assert_eq!(result["isError"], true, "{name} {arguments}: {result}");
        let text = text(&result).to_owned();
        assert!(!text.is_empty(), "{name} {arguments}: {result}");
        text
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

fn write_config(dir: &TempDir, config: &Value) -> PathBuf {
    let path = dir.path().join("config.json");
    std::fs::write(&path, config.to_string()).unwrap();
    path
}

/// A config with token and user key.
fn config(dir: &TempDir) -> PathBuf {
    write_config(dir, &json!({"token": TOKEN, "default_user_key": USER}))
}

/// What Pushover's apps do with an end-to-end encrypted field: check the
/// HMAC over IV and ciphertext, decrypt with AES-256-CBC and unpack gzip.
fn decrypt(value: &str) -> String {
    use aws_lc_rs::cipher::{
        AES_256, DecryptionContext, PaddedBlockDecryptingKey, UnboundCipherKey,
    };
    use base64::Engine;

    let key: Vec<u8> = (0..E2EE_KEY.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&E2EE_KEY[i..i + 2], 16).unwrap())
        .collect();
    let sealed = base64::engine::general_purpose::STANDARD
        .decode(value)
        .expect("Base64");
    let (signed, tag) = sealed.split_at(sealed.len() - 32);
    let hmac_key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, &key);
    aws_lc_rs::hmac::verify(&hmac_key, signed, tag).expect("the tag fits");
    let (iv, ciphertext) = signed.split_at(16);
    let iv: [u8; 16] = iv.try_into().unwrap();
    let mut ciphertext = ciphertext.to_vec();
    let cipher =
        PaddedBlockDecryptingKey::cbc_pkcs7(UnboundCipherKey::new(&AES_256, &key).unwrap())
            .unwrap();
    let compressed = cipher
        .decrypt(&mut ciphertext, DecryptionContext::Iv128(iv.into()))
        .expect("decrypts");
    let mut plaintext = String::new();
    flate2::read::GzDecoder::new(&compressed[..])
        .read_to_string(&mut plaintext)
        .expect("gzip");
    plaintext
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}

// --- Protocol ---

#[test]
fn initialize_names_this_server_and_negotiates_the_version() {
    let dir = TempDir::new().unwrap();
    let config = config(&dir);
    let api = FakeApi::start();
    for version in &PROTOCOL_VERSIONS[..4] {
        let mut server = Server::start(&config, &api);
        let result = server.initialize(version)["result"].clone();
        assert_eq!(result["protocolVersion"], *version);
        let info = &result["serverInfo"];
        assert_eq!(info["name"], env!("CARGO_PKG_NAME"));
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["title"], "Pushover");
        assert!(result["capabilities"]["tools"].is_object(), "{result}");
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("send_notification")
        );
    }

    // 2026-07-28 has no initialize handshake, and 1999-01-01 is unknown: both
    // get the newest version that has one.
    for version in ["2026-07-28", "1999-01-01"] {
        let mut server = Server::start(&config, &api);
        let response = server.initialize(version);
        assert_eq!(
            response["result"]["protocolVersion"], "2025-11-25",
            "{version}"
        );
    }
}

/// 2026-07-28 clients skip `initialize`: they discover the server and send
/// the version with every request.
#[test]
fn protocol_2026_07_28_is_served_through_discovery() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::start(&config(&dir), &api);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });

    let response = server.request("server/discover", &json!({"_meta": meta}));
    let result = &response["result"];
    assert_eq!(
        result["supportedVersions"],
        json!(PROTOCOL_VERSIONS),
        "{response}"
    );
    assert!(result["capabilities"]["tools"].is_object(), "{response}");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        env!("CARGO_PKG_NAME")
    );

    let result = server.call_with(&json!({
        "name": "send_notification", "arguments": {"sender": SENDER, "message": "hi"}, "_meta": meta,
    }));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["structuredContent"]["request_id"], "req-message");
}

#[test]
fn tools_are_listed_with_titles_hints_and_schemas() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);
    let tools = server.tools();

    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, TOOLS);
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        assert!(
            !tool["title"].as_str().unwrap_or_default().is_empty(),
            "{name}"
        );
        assert!(
            !tool["description"].as_str().unwrap_or_default().is_empty(),
            "{name}"
        );
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        // Small models lose referenced definitions on the way.
        let input = tool["inputSchema"].to_string();
        assert!(
            !input.contains("$ref") && !input.contains("$defs"),
            "{name}: {input}"
        );
        assert_eq!(tool["outputSchema"]["type"], "object", "{name}");
        let hints = &tool["annotations"];
        match name {
            "send_notification" => {
                assert_eq!(
                    tool["inputSchema"]["required"],
                    json!(["message", "sender"])
                );
                assert_eq!(
                    tool["inputSchema"]["properties"]["priority"]["enum"],
                    json!(["silent", "low", "normal", "high", "emergency"])
                );
                assert_eq!(hints["readOnlyHint"], false);
                assert_eq!(hints["destructiveHint"], false);
                assert_eq!(hints["idempotentHint"], false);
                assert_eq!(hints["openWorldHint"], true);
            }
            "cancel_emergency_notification" => {
                assert_eq!(hints["readOnlyHint"], false);
                assert_eq!(hints["destructiveHint"], true);
                assert_eq!(hints["idempotentHint"], true);
            }
            _ => assert_eq!(hints["readOnlyHint"], true, "{name}"),
        }
    }
    // A request seen with the tool list would cost a message.
    assert!(api.requests().is_empty());
}

/// rmcp 0.9 ended the session at the first line that was not JSON.
#[test]
fn a_line_that_is_not_json_does_not_end_the_session() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);
    server.send_line("this is not json");
    assert_eq!(server.tools().len(), TOOLS.len());
}

// --- Sending ---

#[test]
fn a_notification_carries_every_parameter() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);

    let result = server.call_ok(
        "send_notification",
        &json!({
            "message": "  Deployment finished \u{1F680}\n",
            "sender": " Claude (pushover mcp) ",
            "title": "CI",
            "priority": "high",
            "sound": "my_bell",
            "device": "iphone, pixel_8",
            "url": "https://example.com/build/42?x=1&y=2",
            "url_title": "Build 42",
            "html": true,
            "ttl": 3600,
            "timestamp": 1_331_249_662_u64,
            "retry": 120,
            "tags": ["ignored without emergency"],
        }),
    );
    assert_eq!(result, json!({"request_id": "req-message"}));

    // Sound and devices are checked against the account before sending.
    let requests = api.requests();
    let paths: Vec<&str> = requests.iter().map(Recorded::path).collect();
    assert_eq!(
        paths,
        [
            "/1/sounds.json",
            "/1/users/validate.json",
            "/1/messages.json"
        ]
    );
    let request = &requests[2];
    assert_eq!(request.method, "POST");
    assert!(
        request
            .content_type
            .starts_with("application/x-www-form-urlencoded"),
        "{}",
        request.content_type
    );
    let mut fields = request.fields();
    fields.sort();
    let mut expected: Vec<(String, String)> = [
        ("token", TOKEN),
        ("user", USER),
        ("message", "Deployment finished \u{1F680}"),
        ("title", "Claude (pushover mcp): CI"),
        ("priority", "1"),
        ("sound", "my_bell"),
        ("device", "iphone,pixel_8"),
        ("url", "https://example.com/build/42?x=1&y=2"),
        ("url_title", "Build 42"),
        ("html", "1"),
        ("ttl", "3600"),
        ("timestamp", "1331249662"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    expected.sort();
    assert_eq!(fields, expected);
}

#[test]
fn the_default_device_from_the_config_is_used() {
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &json!({"token": TOKEN, "default_user_key": USER, "default_device": "pixel_8"}),
    );
    let api = FakeApi::start();
    let mut server = Server::ready(&config, &api);

    server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "a"}),
    );
    server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "b", "device": "iphone"}),
    );
    // The default is the user's own choice and goes unchecked.
    let requests = api.requests();
    let paths: Vec<&str> = requests.iter().map(Recorded::path).collect();
    assert_eq!(
        paths,
        [
            "/1/messages.json",
            "/1/users/validate.json",
            "/1/messages.json"
        ]
    );
    let devices: Vec<String> = [&requests[0], &requests[2]]
        .iter()
        .map(|request| field(&request.fields(), "device").unwrap().to_owned())
        .collect();
    assert_eq!(devices, ["pixel_8", "iphone"]);
}

#[test]
fn attachments_are_uploaded_as_multipart() {
    let dir = TempDir::new().unwrap();
    let image = dir.path().join("Screenshot 2026-11-08.png");
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0";
    std::fs::write(&image, png).unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);

    server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "Chart attached", "attachment_path": image}),
    );

    let requests = api.requests();
    let request = &requests[0];
    assert!(
        request.content_type.starts_with("multipart/form-data"),
        "{}",
        request.content_type
    );
    let fields = request.fields();
    assert_eq!(field(&fields, "token"), Some(TOKEN));
    assert_eq!(field(&fields, "user"), Some(USER));
    assert_eq!(field(&fields, "message"), Some("Chart attached"));
    assert_eq!(field(&fields, "title"), Some(SENDER));
    let parts = multipart_parts(request);
    let attachment = parts.iter().find(|part| part.name == "attachment").unwrap();
    // The local file name stays local.
    assert_eq!(attachment.file_name.as_deref(), Some("attachment.png"));
    assert_eq!(attachment.content_type.as_deref(), Some("image/png"));
    assert_eq!(attachment.data, png);
}

#[test]
fn invalid_input_is_a_tool_error_and_reaches_no_api() {
    let dir = TempDir::new().unwrap();
    let not_an_image = dir.path().join("id_ed25519.png");
    std::fs::write(&not_an_image, "-----BEGIN OPENSSH PRIVATE KEY-----").unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);

    let cases = [
        ("send_notification", json!({}), "message"),
        ("send_notification", json!({"message": "x"}), "sender"),
        (
            "send_notification",
            json!({"sender": " ", "message": "x"}),
            "sender must not be empty",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": " "}),
            "empty",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x".repeat(1025)}),
            "1025 characters",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "priority": "urgent"}),
            "priority must be one of silent, low, normal, high, emergency",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "html": true, "monospace": true}),
            "html and monospace",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "url_title": "Open"}),
            "url_title requires url",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "device": "My iPhone"}),
            "list_devices",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "priority": "emergency", "retry": 10}),
            "at least 30",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "attachment_path": "chart.png"}),
            "absolute path",
        ),
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x", "attachment_path": not_an_image}),
            "not a JPEG, PNG, GIF or WebP image",
        ),
        (
            "check_emergency_receipt",
            json!({"receipt": "../users/validate"}),
            "Invalid receipt",
        ),
        (
            "check_emergency_receipt",
            json!({}),
            "No emergency notification was sent yet; give a receipt.",
        ),
        (
            "cancel_emergency_notification",
            json!({}),
            "No emergency notification was sent yet; give a receipt or a tag.",
        ),
        (
            "cancel_emergency_notification",
            json!({"receipt": RECEIPT, "tag": "x"}),
            "either receipt or tag",
        ),
        (
            "cancel_emergency_notification",
            json!({"tag": "a/b"}),
            "Invalid tag",
        ),
    ];
    for (name, arguments, expected) in cases {
        let error = server.call_err(name, &arguments);
        assert!(error.contains(expected), "{name} {arguments}: {error}");
    }
    assert!(api.requests().is_empty(), "{:?}", api.requests());

    // Unknown tools stay protocol errors.
    let response = server.request("tools/call", &json!({"name": "nope", "arguments": {}}));
    assert!(response.get("error").is_some(), "{response}");
}

// --- Emergency notifications ---

#[test]
fn emergency_notifications_can_be_checked_and_canceled() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);

    let result = server.call_ok(
        "send_notification",
        &json!({
            "message": "Server down",
            "sender": SENDER,
            "priority": "emergency",
            "ttl": 60,
            "tags": ["s=mail01", "l=chicago"],
        }),
    );
    assert_eq!(
        result,
        json!({"request_id": "req-emergency", "receipt": RECEIPT})
    );
    let fields = api.requests()[0].fields();
    assert_eq!(field(&fields, "title"), Some(SENDER));
    assert_eq!(field(&fields, "priority"), Some("2"));
    assert_eq!(field(&fields, "retry"), Some("60"));
    assert_eq!(field(&fields, "expire"), Some("3600"));
    assert_eq!(field(&fields, "tags"), Some("s=mail01,l=chicago"));
    // Pushover ignores ttl for emergency priority.
    assert_eq!(field(&fields, "ttl"), None);

    // The receipt names the device; the user key that acknowledged stays out.
    let first_poll = Instant::now();
    let receipt = server.call_ok("check_emergency_receipt", &json!({"receipt": RECEIPT}));
    assert_eq!(
        receipt,
        json!({
            "acknowledged": true,
            "acknowledged_at": "2013-02-04T23:07:18Z",
            "acknowledged_by_device": "iphone",
            "expired": false,
            "expires_at": "2013-02-05T00:06:18Z",
            "last_delivered_at": "2013-02-04T23:06:18Z",
        })
    );
    let request = api.requests().pop().unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), format!("/1/receipts/{RECEIPT}.json"));
    assert_eq!(request.query(), [("token".to_owned(), TOKEN.to_owned())]);

    // Without a receipt, the latest emergency notification is meant: a client
    // that keeps only what was said has lost the receipt by then.
    // Pushover asks to poll no faster than every 5 seconds. The server notes
    // the time of a poll when it starts, after `first_poll`, so the second
    // poll cannot finish within 5 seconds of it.
    server.call_ok("check_emergency_receipt", &json!({}));
    assert!(
        first_poll.elapsed() >= Duration::from_secs(5),
        "{:?}",
        first_poll.elapsed()
    );
    let request = api.requests().pop().unwrap();
    assert_eq!(request.path(), format!("/1/receipts/{RECEIPT}.json"));

    for arguments in [json!({}), json!({"receipt": RECEIPT})] {
        let result = server.call_ok("cancel_emergency_notification", &arguments);
        assert!(
            result["message"].as_str().unwrap().contains(RECEIPT),
            "{result}"
        );
        let request = api.requests().pop().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path(), format!("/1/receipts/{RECEIPT}/cancel.json"));
        assert_eq!(request.fields(), [("token".to_owned(), TOKEN.to_owned())]);
    }

    let result = server.call_ok(
        "cancel_emergency_notification",
        &json!({"tag": " l=chicago "}),
    );
    assert_eq!(
        result["message"],
        "Canceled 2 emergency notification(s) tagged \"l=chicago\""
    );
    let request = api.requests().pop().unwrap();
    assert_eq!(request.path(), "/1/receipts/cancel_by_tag/l=chicago.json");
    assert_eq!(request.fields(), [("token".to_owned(), TOKEN.to_owned())]);
}

#[test]
fn a_fixed_sender_takes_the_parameter_away_from_the_model() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::start_with_args(&config(&dir), &api.url, &["--sender", " Otomo "]);
    let result = server.initialize("2025-11-25")["result"].clone();
    let instructions = result["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("send_notification needs only message;"),
        "{instructions}"
    );
    let tools = server.tools();
    let send = tools
        .iter()
        .find(|tool| tool["name"] == "send_notification")
        .unwrap();
    assert!(
        send["inputSchema"]["properties"].get("sender").is_none(),
        "{send}"
    );
    assert_eq!(send["inputSchema"]["required"], json!(["message"]));
    assert!(
        send["description"]
            .as_str()
            .unwrap()
            .contains("Only message is required."),
        "{send}"
    );

    server.call_ok("send_notification", &json!({"message": "Backup fertig"}));
    // A sender from the model does not replace the fixed one.
    server.call_ok(
        "send_notification",
        &json!({"message": "x", "title": "Backup", "sender": "Ulrike"}),
    );
    let titles: Vec<String> = api
        .requests()
        .iter()
        .map(|request| field(&request.fields(), "title").unwrap().to_owned())
        .collect();
    assert_eq!(titles, ["Otomo", "Otomo: Backup"]);
}

#[test]
fn an_empty_fixed_sender_stops_the_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_pushover-mcp-rs"))
        .args(["--sender", " "])
        .env_clear()
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--sender must not be empty"), "{stderr}");
}

#[test]
fn devices_and_sounds_are_checked_against_the_account() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);

    // Names match without regard to case, sounds also by their display name.
    server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "a", "device": "IPHONE", "sound": "My Bell"}),
    );
    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "device"), Some("iphone"));
    assert_eq!(field(&fields, "sound"), Some("my_bell"));

    // The API would send to all devices and drop an unknown sound without a
    // word; the model gets the account's names instead.
    let error = server.call_err(
        "send_notification",
        &json!({"sender": SENDER, "message": "b", "device": "Laptop"}),
    );
    assert_eq!(
        error,
        "Unknown device 'Laptop'. The user's devices are iphone, pixel_8: send again with the \
         one the user means (a phone, a laptop: tell it by its name), or omit device to reach \
         all of them."
    );
    let error = server.call_err(
        "send_notification",
        &json!({"sender": SENDER, "message": "c", "sound": "Fahrradklingel"}),
    );
    assert_eq!(
        error,
        "Unknown sound 'Fahrradklingel'. The sounds are my_bell (My Bell), pushover (Pushover \
         (default)), siren (Siren). Send again with the name that fits what the user asked for, \
         or omit sound for the user's default."
    );

    // Each list is fetched once, and again for a name it lacks.
    let paths: Vec<String> = api
        .requests()
        .iter()
        .map(|request| request.path().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            "/1/sounds.json",
            "/1/users/validate.json",
            "/1/messages.json",
            "/1/users/validate.json",
            "/1/sounds.json",
        ]
    );

    // A list fetched for this very call is not fetched a second time.
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);
    server.call_err(
        "send_notification",
        &json!({"sender": SENDER, "message": "d", "device": "Laptop"}),
    );
    let paths: Vec<String> = api
        .requests()
        .iter()
        .map(|request| request.path().to_owned())
        .collect();
    assert_eq!(paths, ["/1/users/validate.json"]);
}

#[test]
fn priority_takes_pushover_numbers() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);
    for (priority, sent) in [
        (json!(1), "1"),
        (json!(-2), "-2"),
        (json!("High"), "1"),
        (json!("-1"), "-1"),
    ] {
        server.call_ok(
            "send_notification",
            &json!({"sender": SENDER, "message": "x", "priority": priority}),
        );
        let fields = api.requests().pop().unwrap().fields();
        assert_eq!(field(&fields, "priority"), Some(sent), "{priority}");
    }
}

/// Loud priorities need `--max-priority`: without it a louder request goes out
/// as an ordinary notification, and the result says what to tell the user.
#[test]
fn loud_priorities_need_to_be_allowed() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::start_with_args(&config(&dir), &api.url, &[]);
    server.initialize("2025-11-25");

    // The emergency tools and tags are gone; the priorities stay described.
    let tools = server.tools();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "get_message_limits",
            "list_devices",
            "list_sounds",
            "send_notification"
        ]
    );
    let send = tools
        .iter()
        .find(|tool| tool["name"] == "send_notification")
        .unwrap();
    let properties = &send["inputSchema"]["properties"];
    assert!(properties.get("tags").is_none());
    // Without retry and expire a request for repeated alerts read as one to watch.
    for kept in ["retry", "expire"] {
        assert!(properties.get(kept).is_some(), "{kept}");
    }
    assert_eq!(
        properties["priority"]["enum"],
        json!(["silent", "low", "normal", "high", "emergency"])
    );

    let result = server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "Front door open", "priority": "high"}),
    );
    assert_eq!(
        result["note"],
        "Sent as an ordinary notification, with normal priority instead of high: alarms are \
         switched off for this client (--max-priority normal). Tell the user that it went out as \
         an ordinary notification."
    );
    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "priority"), Some("0"));

    let result = server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "Server down", "priority": 2, "retry": 30,
                "expire": 600, "tags": ["server"]}),
    );
    assert!(result.get("receipt").is_none(), "{result}");
    assert!(
        result["note"].as_str().unwrap().starts_with(
            "Sent as an ordinary notification, with normal priority instead of emergency"
        ),
        "{result}"
    );
    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "priority"), Some("0"));
    for dropped in ["retry", "expire", "tags"] {
        assert_eq!(field(&fields, dropped), None, "{dropped}");
    }

    // Quieter ones go out as asked, without a note.
    let result = server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "leise", "priority": "low"}),
    );
    assert!(result.get("note").is_none(), "{result}");
    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "priority"), Some("-1"));

    // With --max-priority high an alarm goes out once, with high priority.
    let mut server = Server::start_with_args(&config(&dir), &api.url, &["--max-priority", "high"]);
    server.initialize("2025-11-25");
    assert_eq!(server.tools().len(), 4);
    let result = server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "Server down", "priority": "emergency"}),
    );
    assert!(
        result["note"]
            .as_str()
            .unwrap()
            .ends_with("Tell the user that it went out once, with high priority."),
        "{result}"
    );
    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "priority"), Some("1"));
}

#[test]
fn a_quiet_limit_holds_without_a_priority_too() {
    // Pushover takes a missing priority as normal, which would ring.
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    for (limit, number) in [("low", "-1"), ("silent", "-2")] {
        let mut server =
            Server::start_with_args(&config(&dir), &api.url, &["--max-priority", limit]);
        server.initialize("2025-11-25");
        let result = server.call_ok(
            "send_notification",
            &json!({"sender": SENDER, "message": "Backup done"}),
        );
        assert_eq!(
            result["note"],
            format!(
                "Sent as a quiet notification, with {limit} priority instead of normal: this \
                 client may only send quiet notifications (--max-priority {limit}). Tell the \
                 user that it went out as a quiet notification."
            )
        );
        let fields = api.requests().pop().unwrap().fields();
        assert_eq!(field(&fields, "priority"), Some(number), "{limit}");
    }

    // From normal on a missing priority stays missing, without a note.
    let mut server = Server::start_with_args(&config(&dir), &api.url, &[]);
    server.initialize("2025-11-25");
    let result = server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "Backup done"}),
    );
    assert!(result.get("note").is_none(), "{result}");
    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "priority"), None);
}

// --- Account information ---

#[test]
fn devices_sounds_and_limits_are_listed() {
    let dir = TempDir::new().unwrap();
    let api = FakeApi::start();
    let mut server = Server::ready(&config(&dir), &api);

    assert_eq!(
        server.call_ok("list_devices", &json!({})),
        json!({"devices": ["iphone", "pixel_8"], "group": false})
    );
    let request = api.requests().pop().unwrap();
    assert_eq!(request.path(), "/1/users/validate.json");
    let mut fields = request.fields();
    fields.sort();
    assert_eq!(
        fields,
        [
            ("token".to_owned(), TOKEN.to_owned()),
            ("user".to_owned(), USER.to_owned())
        ]
    );

    assert_eq!(
        server.call_ok("list_sounds", &json!({})),
        json!({"sounds": [
            {"name": "my_bell", "description": "My Bell"},
            {"name": "pushover", "description": "Pushover (default)"},
            {"name": "siren", "description": "Siren"},
        ]})
    );
    assert_eq!(
        server.call_ok("get_message_limits", &json!({})),
        json!({"limit": 10000, "remaining": 7496, "reset_at": "2014-03-01T06:00:00Z"})
    );
    for request in &api.requests()[1..] {
        assert_eq!(request.method, "GET");
        assert_eq!(request.query(), [("token".to_owned(), TOKEN.to_owned())]);
    }
}

// --- Failures ---

#[test]
fn api_errors_reach_the_model_without_secrets() {
    let dir = TempDir::new().unwrap();
    let config = config(&dir);
    let cases: [(u16, Value, &str); 3] = [
        (
            400,
            json!({"user": "invalid", "errors": ["user identifier is invalid"],
                   "status": 0, "request": "req-bad"}),
            "Pushover rejected the request (HTTP 400 Bad Request): user identifier is \
             invalid (request req-bad)",
        ),
        (
            429,
            json!({"errors": ["message limit reached"], "status": 0, "request": "req-429"}),
            "get_message_limits",
        ),
        (503, json!({}), "temporary problems"),
    ];
    for (status, body, expected) in cases {
        let api = FakeApi::start_with(move |_| (status, body.to_string()));
        let mut server = Server::ready(&config, &api);
        for (name, arguments) in [
            (
                "send_notification",
                json!({"sender": SENDER, "message": "x"}),
            ),
            ("check_emergency_receipt", json!({"receipt": RECEIPT})),
        ] {
            let error = server.call_err(name, &arguments);
            assert!(error.contains(expected), "{status} {name}: {error}");
        }
    }

    // A port that was just free refuses connections. reqwest names the URL
    // of a failed request, and the receipt URL holds the token.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut server = Server::start_with_url(&config, &format!("http://127.0.0.1:{port}/1"));
    server.initialize("2025-11-25");
    for (name, arguments) in [
        (
            "send_notification",
            json!({"sender": SENDER, "message": "x"}),
        ),
        ("check_emergency_receipt", json!({"receipt": RECEIPT})),
        ("list_devices", json!({})),
    ] {
        let error = server.call_err(name, &arguments);
        assert!(
            error.contains("Request to the Pushover API failed"),
            "{error}"
        );
        assert!(!error.contains(TOKEN), "{name}: {error}");
        assert!(!error.contains(USER), "{name}: {error}");
    }
}

#[test]
fn missing_credentials_name_the_config_file_and_edits_apply_at_once() {
    let dir = TempDir::new().unwrap();
    let path = write_config(&dir, &json!({"token": "", "default_user_key": null}));
    let api = FakeApi::start();
    let mut server = Server::ready(&path, &api);
    let shown = path.display().to_string();

    let error = server.call_err(
        "send_notification",
        &json!({"sender": SENDER, "message": "x"}),
    );
    assert!(error.contains("No Pushover API token"), "{error}");
    assert!(error.contains(&shown), "{error}");

    write_config(&dir, &json!({"token": TOKEN}));
    let error = server.call_err("list_devices", &json!({}));
    assert!(error.contains("default_user_key"), "{error}");
    assert!(error.contains(&shown), "{error}");

    std::fs::write(&path, "{ broken").unwrap();
    let error = server.call_err("list_sounds", &json!({}));
    assert!(error.contains(&shown), "{error}");
    assert!(error.contains("not valid"), "{error}");
    assert!(api.requests().is_empty());

    // The file is read on every call: no restart after an edit.
    config(&dir);
    server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "x"}),
    );
}

#[test]
fn an_encryption_key_encrypts_the_message_fields() {
    let dir = TempDir::new().unwrap();
    let config = write_config(
        &dir,
        &json!({"token": TOKEN, "default_user_key": USER, "encryption_key": E2EE_KEY}),
    );
    let api = FakeApi::start();
    let mut server = Server::ready(&config, &api);

    let result = server.call_ok(
        "send_notification",
        &json!({"sender": SENDER, "message": "Der Tresor ist offen", "title": "Haus",
                "url": "https://example.com/tresor", "url_title": "Kamera", "priority": "high",
                "sound": "siren"}),
    );
    assert_eq!(result, json!({"request_id": "req-message"}));

    let fields = api.requests().pop().unwrap().fields();
    assert_eq!(field(&fields, "encrypted"), Some("1"));
    for (name, plain) in [
        ("message", "Der Tresor ist offen"),
        ("title", "Claude (stdio test): Haus"),
        ("url", "https://example.com/tresor"),
        ("url_title", "Kamera"),
    ] {
        let sent = field(&fields, name).unwrap();
        assert!(
            !sent.contains(plain),
            "{name} went out in the clear: {sent}"
        );
        assert_eq!(decrypt(sent), plain, "{name}");
    }
    // The other fields stay readable for Pushover, which delivers by them.
    assert_eq!(field(&fields, "priority"), Some("1"));
    assert_eq!(field(&fields, "sound"), Some("siren"));
    assert_eq!(field(&fields, "token"), Some(TOKEN));
}

#[test]
fn an_invalid_encryption_key_stops_sending_and_stays_private() {
    let dir = TempDir::new().unwrap();
    let path = write_config(
        &dir,
        &json!({"token": TOKEN, "default_user_key": USER, "encryption_key": "my-secret-passphrase"}),
    );
    let api = FakeApi::start();
    let mut server = Server::ready(&path, &api);

    let error = server.call_err(
        "send_notification",
        &json!({"sender": SENDER, "message": "x"}),
    );
    assert!(error.contains("encryption_key"), "{error}");
    assert!(error.contains(&path.display().to_string()), "{error}");
    assert!(!error.contains("my-secret-passphrase"), "{error}");
    assert!(api.requests().is_empty(), "{:?}", api.requests());
    // Only sending needs the key.
    server.call_ok("list_sounds", &json!({}));
}

#[test]
fn the_first_start_writes_a_private_config_template() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pushover").join("config.json");
    let api = FakeApi::start();
    let mut server = Server::ready(&path, &api);

    let template: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        template,
        json!({"token": "", "default_user_key": null, "default_device": null, "encryption_key": null})
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let error = server.call_err(
        "send_notification",
        &json!({"sender": SENDER, "message": "x"}),
    );
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
fn version_flag_prints_the_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_pushover-mcp-rs"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        concat!(env!("CARGO_PKG_NAME"), " ", env!("CARGO_PKG_VERSION"))
    );
}
