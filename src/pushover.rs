//! Client for the Pushover REST API (<https://pushover.net/api>).
//!
//! It follows Pushover's rules for API clients: at most two concurrent
//! connections and receipt polls no faster than every five seconds. Error
//! texts reach the model, so they never contain the token or the user key.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::{StatusCode, Url, multipart};
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, Semaphore};

use crate::{
    attachment::Image,
    model::{
        ApiStatus, CancelResponse, LimitsResponse, MessageResponse, ReceiptResponse,
        SoundsResponse, ValidateResponse,
    },
};

/// Base URL of the Pushover API.
pub const DEFAULT_API_URL: &str = "https://api.pushover.net/1";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// An attachment of up to 5 MB on a slow uplink needs longer.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// "Do not create more than 2 concurrent HTTP requests (TCP connections) to
/// our API" (Pushover API documentation).
const MAX_CONCURRENT_REQUESTS: usize = 2;
/// "Submit a GET request (no faster than once every 5 seconds)" (Pushover
/// receipts API documentation).
const RECEIPT_POLL_INTERVAL: Duration = Duration::from_secs(5);

pub struct PushoverClient {
    http: reqwest::Client,
    base: Url,
    slots: Semaphore,
    last_receipt_poll: Mutex<Option<Instant>>,
}

impl PushoverClient {
    /// A client for the API at `api_url`, normally [`DEFAULT_API_URL`].
    pub fn new(api_url: &str, user_agent: &str) -> Result<Self> {
        let base = Url::parse(api_url).with_context(|| format!("Invalid API URL {api_url}"))?;
        if !matches!(base.scheme(), "http" | "https") || base.cannot_be_a_base() {
            anyhow::bail!("Invalid API URL {api_url}: expected an http or https URL");
        }

        let mut builder = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT);
        if base.scheme() == "http" {
            // A plain-HTTP API (a local test server) needs no certificates,
            // so the system's CA store is not loaded; the Nix build sandbox
            // has none, and loading fails there.
            builder = builder.tls_certs_only(std::iter::empty());
        }
        let http = builder
            .build()
            .context("Could not set up the HTTPS client")?;

        Ok(Self {
            http,
            base,
            slots: Semaphore::new(MAX_CONCURRENT_REQUESTS),
            last_receipt_poll: Mutex::new(None),
        })
    }

    /// `POST messages.json`. `fields` are the message parameters without
    /// token and user.
    pub async fn send_message(
        &self,
        token: &str,
        user: &str,
        fields: Vec<(&'static str, String)>,
        image: Option<Image>,
    ) -> Result<MessageResponse, String> {
        let url = self.endpoint(&["messages.json"]);
        let mut form = Vec::with_capacity(fields.len() + 2);
        form.push(("token", token.to_owned()));
        form.push(("user", user.to_owned()));
        form.extend(fields);

        let request = match image {
            None => self.http.post(url).form(&form),
            Some(image) => {
                let attachment = multipart::Part::bytes(image.data)
                    .file_name(image.file_name)
                    .mime_str(image.mime_type)
                    .map_err(|e| format!("Invalid attachment type: {e}"))?;
                let multipart = form
                    .into_iter()
                    .fold(multipart::Form::new(), |multipart, (name, value)| {
                        multipart.text(name, value)
                    })
                    .part("attachment", attachment);
                self.http
                    .post(url)
                    .multipart(multipart)
                    .timeout(UPLOAD_TIMEOUT)
            }
        };
        self.execute(request, &[token, user]).await
    }

    /// `POST users/validate.json`: the active devices of a user or group.
    pub async fn validate_user(&self, token: &str, user: &str) -> Result<ValidateResponse, String> {
        let request = self
            .http
            .post(self.endpoint(&["users", "validate.json"]))
            .form(&[("token", token), ("user", user)]);
        self.execute(request, &[token, user]).await
    }

    /// `GET sounds.json`: built-in and custom sounds.
    pub async fn sounds(&self, token: &str) -> Result<SoundsResponse, String> {
        let request = self
            .http
            .get(self.endpoint_with_token(&["sounds.json"], token));
        self.execute(request, &[token]).await
    }

    /// `GET apps/limits.json`: the monthly message quota of the account.
    pub async fn limits(&self, token: &str) -> Result<LimitsResponse, String> {
        let request = self
            .http
            .get(self.endpoint_with_token(&["apps", "limits.json"], token));
        self.execute(request, &[token]).await
    }

    /// `GET receipts/<receipt>.json`, at least five seconds after the
    /// previous poll.
    pub async fn receipt(&self, token: &str, receipt: &str) -> Result<ReceiptResponse, String> {
        self.wait_for_receipt_poll().await;
        let file = format!("{receipt}.json");
        let request = self
            .http
            .get(self.endpoint_with_token(&["receipts", &file], token));
        self.execute(request, &[token]).await
    }

    /// `POST receipts/<receipt>/cancel.json`.
    pub async fn cancel_receipt(
        &self,
        token: &str,
        receipt: &str,
    ) -> Result<CancelResponse, String> {
        let request = self
            .http
            .post(self.endpoint(&["receipts", receipt, "cancel.json"]))
            .form(&[("token", token)]);
        self.execute(request, &[token]).await
    }

    /// `POST receipts/cancel_by_tag/<tag>.json`.
    pub async fn cancel_by_tag(&self, token: &str, tag: &str) -> Result<CancelResponse, String> {
        let file = format!("{tag}.json");
        let request = self
            .http
            .post(self.endpoint(&["receipts", "cancel_by_tag", &file]))
            .form(&[("token", token)]);
        self.execute(request, &[token]).await
    }

    /// The URL of an API call; each segment is percent-encoded.
    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.base.clone();
        url.path_segments_mut()
            .expect("new() accepts only URLs with a path")
            .pop_if_empty()
            .extend(segments);
        url
    }

    /// The URL of a GET call, which takes the token as query parameter.
    fn endpoint_with_token(&self, segments: &[&str], token: &str) -> Url {
        let mut url = self.endpoint(segments);
        url.query_pairs_mut().append_pair("token", token);
        url
    }

    async fn wait_for_receipt_poll(&self) {
        let mut last = self.last_receipt_poll.lock().await;
        if let Some(previous) = *last {
            let wait = RECEIPT_POLL_INTERVAL.saturating_sub(previous.elapsed());
            if !wait.is_zero() {
                tracing::debug!("Waiting {wait:?} before the next receipt poll");
                tokio::time::sleep(wait).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Sends a request within the connection limit and reads the response.
    async fn execute<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        secrets: &[&str],
    ) -> Result<T, String> {
        let result = async {
            let _slot = self
                .slots
                .acquire()
                .await
                .map_err(|_| "The HTTP client is shut down".to_owned())?;
            let response = request.send().await.map_err(transport_error)?;
            let status = response.status();
            let body = response.bytes().await.map_err(transport_error)?;
            parse_response(status, &body)
        }
        .await;
        result.map_err(|error| {
            // Redacted before logging, too: MCP clients keep stderr in log files.
            let error = redact(error, secrets);
            tracing::warn!("{error}");
            error
        })
    }
}

/// Maps an HTTP response to the call's result, following "Being Friendly to
/// our API": 5xx is temporary, 4xx or a status other than 1 is a rejection
/// that repeating does not fix.
fn parse_response<T: DeserializeOwned>(status: StatusCode, body: &[u8]) -> Result<T, String> {
    if status.is_server_error() {
        return Err(format!(
            "The Pushover API has temporary problems (HTTP {status}). Try again in a few \
             seconds, not sooner than 5."
        ));
    }
    match serde_json::from_slice::<ApiStatus>(body) {
        Ok(envelope) if status.is_success() && envelope.status == 1 => serde_json::from_slice(body)
            .map_err(|e| format!("Unexpected response from the Pushover API: {e}")),
        Ok(envelope) => Err(rejection(status, envelope)),
        Err(e) if status.is_success() => {
            Err(format!("Unexpected response from the Pushover API: {e}"))
        }
        Err(_) => Err(format!("The Pushover API answered HTTP {status}")),
    }
}

fn rejection(status: StatusCode, envelope: ApiStatus) -> String {
    let reasons = envelope
        .errors
        .filter(|errors| !errors.is_empty())
        .map_or_else(|| "no reason given".to_owned(), |errors| errors.join("; "));
    let text = if status == StatusCode::TOO_MANY_REQUESTS {
        format!(
            "Pushover refused the request (HTTP {status}, usually because the monthly \
             message limit is used up, see get_message_limits): {reasons}"
        )
    } else {
        format!("Pushover rejected the request (HTTP {status}): {reasons}")
    };
    match envelope.request {
        Some(request) => format!("{text} (request {request})"),
        None => text,
    }
}

/// Describes a failed request with its causes. reqwest names the URL, which
/// for GET calls holds the token, so it is dropped.
fn transport_error(error: reqwest::Error) -> String {
    let error = error.without_url();
    if error.is_timeout() {
        return "The Pushover API did not answer in time".to_owned();
    }
    let mut text = format!("Request to the Pushover API failed: {error}");
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Replaces each secret in `text`, a last guard before text reaches the model.
fn redact(mut text: String, secrets: &[&str]) -> String {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        if text.contains(secret) {
            text = text.replace(secret, "[redacted]");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(api_url: &str) -> PushoverClient {
        PushoverClient::new(api_url, "test").unwrap()
    }

    #[test]
    fn endpoints_encode_each_segment() {
        let api = client("http://127.0.0.1:9/1");
        assert_eq!(
            api.endpoint(&["receipts", "cancel_by_tag", "l=chicago rack/2?.json"])
                .as_str(),
            "http://127.0.0.1:9/1/receipts/cancel_by_tag/l=chicago%20rack%2F2%3F.json"
        );
        assert_eq!(
            api.endpoint_with_token(&["sounds.json"], "a&b=c").as_str(),
            "http://127.0.0.1:9/1/sounds.json?token=a%26b%3Dc"
        );
        // A trailing slash on the base does not double the separator.
        assert_eq!(
            client("http://127.0.0.1:9/1/")
                .endpoint(&["messages.json"])
                .as_str(),
            "http://127.0.0.1:9/1/messages.json"
        );
    }

    #[test]
    fn api_urls_must_be_http() {
        for url in ["ftp://example.com/1", "mailto:a@example.com", "not a url"] {
            assert!(PushoverClient::new(url, "test").is_err(), "{url}");
        }
    }

    #[test]
    fn responses_map_to_results_and_errors() {
        let ok: MessageResponse =
            parse_response(StatusCode::OK, br#"{"status":1,"request":"r1"}"#).unwrap();
        assert_eq!(ok.request, "r1");

        let error = parse_response::<MessageResponse>(
            StatusCode::BAD_REQUEST,
            br#"{"user":"invalid","errors":["user identifier is invalid"],"status":0,"request":"r2"}"#,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Pushover rejected the request (HTTP 400 Bad Request): user identifier is invalid (request r2)"
        );

        let error = parse_response::<MessageResponse>(
            StatusCode::TOO_MANY_REQUESTS,
            br#"{"status":0,"errors":["message limit reached"]}"#,
        )
        .unwrap_err();
        assert!(error.contains("get_message_limits"), "{error}");

        // A 200 with a status other than 1 is a rejection, too.
        let error =
            parse_response::<MessageResponse>(StatusCode::OK, br#"{"status":0}"#).unwrap_err();
        assert!(error.contains("no reason given"), "{error}");

        let error =
            parse_response::<MessageResponse>(StatusCode::BAD_GATEWAY, b"<html>bad gateway</html>")
                .unwrap_err();
        assert!(error.contains("temporary problems"), "{error}");

        let error =
            parse_response::<MessageResponse>(StatusCode::FORBIDDEN, b"<html>").unwrap_err();
        assert_eq!(error, "The Pushover API answered HTTP 403 Forbidden");

        let error =
            parse_response::<MessageResponse>(StatusCode::OK, b"{\"status\":1}").unwrap_err();
        assert!(error.starts_with("Unexpected response"), "{error}");
    }

    #[test]
    fn secrets_are_redacted() {
        assert_eq!(
            redact(
                "token abc and user xyz, abc again".into(),
                &["abc", "xyz", ""]
            ),
            "token [redacted] and user [redacted], [redacted] again"
        );
    }

    #[tokio::test]
    async fn transport_errors_do_not_name_the_url() {
        // A port that was just free refuses connections.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let api = client(&format!("http://127.0.0.1:{port}/1"));
        let error = api.sounds("tokentokentoken").await.unwrap_err();
        assert!(
            error.starts_with("Request to the Pushover API failed"),
            "{error}"
        );
        assert!(!error.contains("tokentokentoken"), "{error}");
        assert!(!error.contains("sounds.json"), "{error}");
    }
}
