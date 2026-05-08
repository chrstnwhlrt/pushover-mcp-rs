use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use rmcp::{
    Json, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use crate::{
    config::{self, Config},
    model::{
        AvailableSound, CancelEmergencyParams, CancelEmergencyResponse, CheckReceiptParams,
        CheckReceiptResponse, ListDevicesParams, ListDevicesResponse, ListSoundsParams,
        ListSoundsResponse, MAX_EXPIRE_SECONDS, MAX_MESSAGE_LENGTH, MAX_TITLE_LENGTH,
        MAX_URL_LENGTH, MAX_URL_TITLE_LENGTH, MIN_RETRY_SECONDS, PushoverApiResponse,
        PushoverCancelResponse, PushoverReceiptResponse, PushoverSoundsResponse,
        PushoverValidateResponse, SendNotificationParams, SendNotificationResponse,
    },
};

const PUSHOVER_API_URL: &str = "https://api.pushover.net/1/messages.json";
const PUSHOVER_SOUNDS_URL: &str = "https://api.pushover.net/1/sounds.json";
const PUSHOVER_VALIDATE_URL: &str = "https://api.pushover.net/1/users/validate.json";
const PUSHOVER_RECEIPTS_URL: &str = "https://api.pushover.net/1/receipts";
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 10;
const HTTP_TIMEOUT_SECS: u64 = 30;
const SOUNDS_HTTP_TIMEOUT_SECS: u64 = 5;

const NO_TOKEN_ERROR: &str =
    "No Pushover API token configured. The user must add their token to the config file.";

/// Trim an optional string, returning None if empty after trimming.
#[inline]
fn trim_optional(s: Option<String>) -> Option<String> {
    s.and_then(|s| {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == s.len() {
            // No trimming needed, return original to avoid allocation
            Some(s)
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Format API errors into a single string, handling empty/None cases.
#[inline]
fn format_api_errors(errors: Option<Vec<String>>) -> String {
    errors
        .filter(|e| !e.is_empty())
        .map_or_else(|| "Unknown error".to_string(), |e| e.join(", "))
}

/// Validate input parameters against Pushover API limits.
fn validate_params(params: &SendNotificationParams) -> Result<(), String> {
    // Validate trimmed message since we trim before sending
    let message_trimmed = params.message.trim();

    // Message is required and cannot be empty
    if message_trimmed.is_empty() {
        return Err("Message cannot be empty".to_string());
    }

    // Use chars().count() for proper UTF-8 character counting (not byte length)
    let message_chars = message_trimmed.chars().count();
    if message_chars > MAX_MESSAGE_LENGTH {
        return Err(format!(
            "Message exceeds maximum length of {MAX_MESSAGE_LENGTH} characters (got {message_chars})"
        ));
    }

    // Validate trimmed lengths since we trim before sending to API
    if let Some(ref title) = params.title {
        let title_trimmed = title.trim();
        let title_chars = title_trimmed.chars().count();
        if title_chars > MAX_TITLE_LENGTH {
            return Err(format!(
                "Title exceeds maximum length of {MAX_TITLE_LENGTH} characters (got {title_chars})"
            ));
        }
    }

    // URL limits are in bytes (per HTTP spec), not characters
    // Validate trimmed length
    if let Some(ref url) = params.url {
        let url_trimmed = url.trim();
        if url_trimmed.len() > MAX_URL_LENGTH {
            return Err(format!(
                "URL exceeds maximum length of {} bytes (got {})",
                MAX_URL_LENGTH,
                url_trimmed.len()
            ));
        }
    }

    if let Some(ref url_title) = params.url_title {
        let url_title_trimmed = url_title.trim();
        let url_title_chars = url_title_trimmed.chars().count();
        if url_title_chars > MAX_URL_TITLE_LENGTH {
            return Err(format!(
                "URL title exceeds maximum length of {MAX_URL_TITLE_LENGTH} characters (got {url_title_chars})"
            ));
        }
    }

    // Emergency priority validation for retry/expire
    if let Some(priority) = params.priority
        && priority.is_emergency()
    {
        if let Some(retry) = params.retry
            && retry < MIN_RETRY_SECONDS
        {
            return Err(format!(
                "Retry interval must be at least {MIN_RETRY_SECONDS} seconds for emergency priority (got {retry})"
            ));
        }
        if let Some(expire) = params.expire
            && expire > MAX_EXPIRE_SECONDS
        {
            return Err(format!(
                "Expire duration cannot exceed {MAX_EXPIRE_SECONDS} seconds for emergency priority (got {expire})"
            ));
        }
    }

    if params.html == Some(true) && params.monospace == Some(true) {
        return Err("Cannot use both html and monospace formatting".to_string());
    }

    // url_title requires url
    if params.url_title.is_some() && params.url.as_ref().is_none_or(|u| u.trim().is_empty()) {
        return Err("url_title requires url to be set".to_string());
    }

    Ok(())
}

#[derive(Clone)]
pub struct PushoverServer {
    name: &'static str,
    version: &'static str,
    config: Arc<Config>,
    http_client: reqwest::Client,
    sounds_cache: Arc<RwLock<Option<Vec<AvailableSound>>>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl PushoverServer {
    /// Send a push notification via Pushover.
    #[tool(
        description = "Send a push notification. Only message is required. Returns {success: bool, message: string, request_id: string, receipt?: string}. Receipt is only present for emergency priority."
    )]
    async fn send_notification(
        &self,
        Parameters(params): Parameters<SendNotificationParams>,
    ) -> Result<Json<SendNotificationResponse>, String> {
        // Validate input parameters
        validate_params(&params)?;

        // Check token is configured
        if !config::is_token_configured(&self.config) {
            return Err(NO_TOKEN_ERROR.to_string());
        }

        // Get user key from config
        let user_key = self
            .config
            .default_user_key
            .clone()
            .ok_or("No default_user_key configured. Add your user key to the config file.")?;

        let device = trim_optional(params.device).or_else(|| self.config.default_device.clone());

        // Pre-allocate form vector with estimated capacity
        let mut form = Vec::with_capacity(16);
        form.push(("token", self.config.token.clone()));
        form.push(("user", user_key));
        // Trim message for consistency with other string fields
        form.push(("message", params.message.trim().to_string()));

        // Trim optional string fields - treat empty strings as None
        if let Some(title) = trim_optional(params.title) {
            form.push(("title", title));
        }

        if let Some(device) = device {
            form.push(("device", device));
        }

        if let Some(priority) = params.priority {
            form.push(("priority", priority.to_api_value().to_string()));

            // Emergency priority requires retry and expire
            if priority.is_emergency() {
                let retry = params.retry.unwrap_or(60);
                let expire = params.expire.unwrap_or(3600);
                form.push(("retry", retry.to_string()));
                form.push(("expire", expire.to_string()));
            }
        }

        if let Some(sound) = trim_optional(params.sound) {
            form.push(("sound", sound));
        }

        if let Some(url) = trim_optional(params.url) {
            form.push(("url", url));
        }

        if let Some(url_title) = trim_optional(params.url_title) {
            form.push(("url_title", url_title));
        }

        if params.html == Some(true) {
            form.push(("html", "1".to_string()));
        }

        if params.monospace == Some(true) {
            form.push(("monospace", "1".to_string()));
        }

        if let Some(ttl) = params.ttl {
            form.push(("ttl", ttl.to_string()));
        }

        if let Some(timestamp) = params.timestamp {
            form.push(("timestamp", timestamp.to_string()));
        }

        // Send the request
        let response = self
            .http_client
            .post(PUSHOVER_API_URL)
            .form(&form)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status();

        // Handle non-success HTTP status codes
        if !status.is_success() && !status.is_client_error() {
            return Err(format!("Pushover API returned HTTP {status}"));
        }

        let api_response: PushoverApiResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Pushover response: {e}"))?;

        if api_response.status == 1 {
            Ok(Json(SendNotificationResponse {
                success: true,
                message: "Notification sent successfully".to_string(),
                request_id: Some(api_response.request),
                receipt: api_response.receipt,
            }))
        } else {
            Ok(Json(SendNotificationResponse {
                success: false,
                message: format!(
                    "Pushover API error: {}",
                    format_api_errors(api_response.errors)
                ),
                request_id: Some(api_response.request),
                receipt: None,
            }))
        }
    }

    /// List supported notification sounds.
    #[tool(
        description = "List notification sounds supported by the configured Pushover application. Includes built-in and account custom sounds. Returns {success: bool, message: string, sounds?: [{name: string, description: string}]}. Use name as send_notification.sound. Set refresh=true after changing custom sounds."
    )]
    async fn list_sounds(
        &self,
        Parameters(params): Parameters<ListSoundsParams>,
    ) -> Result<Json<ListSoundsResponse>, String> {
        // Check token is configured
        if !config::is_token_configured(&self.config) {
            return Err(NO_TOKEN_ERROR.to_string());
        }

        let use_cache = params.refresh != Some(true);
        if use_cache && let Some(sounds) = self.cached_sounds()? {
            return Ok(Json(ListSoundsResponse {
                success: true,
                message: format!("Loaded {} sounds from startup cache", sounds.len()),
                sounds: Some(sounds),
            }));
        }

        match self.refresh_sounds_cache().await {
            Ok(sounds) => Ok(Json(ListSoundsResponse {
                success: true,
                message: format!("Loaded {} sounds from Pushover", sounds.len()),
                sounds: Some(sounds),
            })),
            Err(e) => Ok(Json(ListSoundsResponse {
                success: false,
                message: e,
                sounds: None,
            })),
        }
    }

    /// List available devices for a user.
    #[tool(
        description = "List available device names. Returns {success: bool, message: string, devices?: string[]}. Only use if user explicitly asks for a specific device."
    )]
    async fn list_devices(
        &self,
        Parameters(_params): Parameters<ListDevicesParams>,
    ) -> Result<Json<ListDevicesResponse>, String> {
        // Check token is configured
        if !config::is_token_configured(&self.config) {
            return Err(NO_TOKEN_ERROR.to_string());
        }

        // Get user key from config
        let user_key = self
            .config
            .default_user_key
            .clone()
            .ok_or("No default_user_key configured. Add your user key to the config file.")?;

        // Call validation endpoint
        let response = self
            .http_client
            .post(PUSHOVER_VALIDATE_URL)
            .form(&[
                ("token", self.config.token.as_str()),
                ("user", user_key.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() && !status.is_client_error() {
            return Err(format!("Pushover API returned HTTP {status}"));
        }

        let api_response: PushoverValidateResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Pushover response: {e}"))?;

        if api_response.status == 1 {
            Ok(Json(ListDevicesResponse {
                success: true,
                message: "User validated successfully".to_string(),
                devices: api_response.devices,
            }))
        } else {
            Ok(Json(ListDevicesResponse {
                success: false,
                message: format!(
                    "Validation failed: {}",
                    format_api_errors(api_response.errors)
                ),
                devices: None,
            }))
        }
    }

    /// Check the status of an emergency notification.
    #[tool(
        description = "Check if an emergency notification was acknowledged. Returns {success: bool, message: string, acknowledged?: bool, acknowledged_by?: string, expired?: bool}. Use the receipt from send_notification."
    )]
    async fn check_emergency_receipt(
        &self,
        Parameters(params): Parameters<CheckReceiptParams>,
    ) -> Result<Json<CheckReceiptResponse>, String> {
        // Check token is configured
        if !config::is_token_configured(&self.config) {
            return Err(NO_TOKEN_ERROR.to_string());
        }

        let receipt = params.receipt.trim();
        if receipt.is_empty() {
            return Err("Receipt ID cannot be empty".to_string());
        }

        // GET request to receipts endpoint
        let token = &self.config.token;
        let url = format!("{PUSHOVER_RECEIPTS_URL}/{receipt}.json?token={token}");
        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() && !status.is_client_error() {
            return Err(format!("Pushover API returned HTTP {status}"));
        }

        let api_response: PushoverReceiptResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Pushover response: {e}"))?;

        if api_response.status == 1 {
            let acknowledged = api_response.acknowledged == Some(1);
            let expired = api_response.expired == Some(1);

            let message = if acknowledged {
                format!(
                    "Notification was acknowledged{}",
                    api_response
                        .acknowledged_by
                        .as_ref()
                        .map(|d| format!(" by device '{d}'"))
                        .unwrap_or_default()
                )
            } else if expired {
                "Notification expired without acknowledgment".to_string()
            } else {
                "Notification is still waiting for acknowledgment".to_string()
            };

            Ok(Json(CheckReceiptResponse {
                success: true,
                message,
                acknowledged: Some(acknowledged),
                acknowledged_by: api_response.acknowledged_by,
                expired: Some(expired),
            }))
        } else {
            Ok(Json(CheckReceiptResponse {
                success: false,
                message: format!(
                    "Receipt check failed: {}",
                    format_api_errors(api_response.errors)
                ),
                acknowledged: None,
                acknowledged_by: None,
                expired: None,
            }))
        }
    }

    /// Cancel an emergency notification.
    #[tool(
        description = "Cancel an emergency notification that is still retrying. Returns {success: bool, message: string}. Use the receipt from send_notification."
    )]
    async fn cancel_emergency_notification(
        &self,
        Parameters(params): Parameters<CancelEmergencyParams>,
    ) -> Result<Json<CancelEmergencyResponse>, String> {
        // Check token is configured
        if !config::is_token_configured(&self.config) {
            return Err(NO_TOKEN_ERROR.to_string());
        }

        let receipt = params.receipt.trim();
        if receipt.is_empty() {
            return Err("Receipt ID cannot be empty".to_string());
        }

        // POST request to cancel endpoint
        let url = format!("{PUSHOVER_RECEIPTS_URL}/{receipt}/cancel.json");
        let response = self
            .http_client
            .post(&url)
            .form(&[("token", self.config.token.as_str())])
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() && !status.is_client_error() {
            return Err(format!("Pushover API returned HTTP {status}"));
        }

        let api_response: PushoverCancelResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Pushover response: {e}"))?;

        if api_response.status == 1 {
            Ok(Json(CancelEmergencyResponse {
                success: true,
                message: "Emergency notification canceled successfully".to_string(),
            }))
        } else {
            Ok(Json(CancelEmergencyResponse {
                success: false,
                message: format!("Cancel failed: {}", format_api_errors(api_response.errors)),
            }))
        }
    }
}

impl PushoverServer {
    pub async fn new(name: &'static str, version: &'static str, config: Config) -> Self {
        let user_agent = format!("{name}/{version}");
        let http_client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .expect("Failed to create HTTP client");

        let server = Self {
            name,
            version,
            config: Arc::new(config),
            http_client,
            sounds_cache: Arc::new(RwLock::new(None)),
            tool_router: Self::tool_router(),
        };

        if config::is_token_configured(&server.config) {
            match server.refresh_sounds_cache().await {
                Ok(sounds) => tracing::info!("Loaded {} Pushover sounds", sounds.len()),
                Err(e) => tracing::warn!("Failed to load Pushover sounds during startup: {}", e),
            }
        }

        server
    }

    async fn fetch_sounds(&self) -> Result<Vec<AvailableSound>, String> {
        let response = self
            .http_client
            .get(PUSHOVER_SOUNDS_URL)
            .query(&[("token", self.config.token.as_str())])
            .timeout(Duration::from_secs(SOUNDS_HTTP_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        let status = response.status();
        if !status.is_success() && !status.is_client_error() {
            return Err(format!("Pushover API returned HTTP {status}"));
        }

        let api_response: PushoverSoundsResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Pushover sounds response: {e}"))?;

        if api_response.status == 1 {
            Ok(api_response
                .sounds
                .unwrap_or_default()
                .into_iter()
                .map(|(name, description)| AvailableSound { name, description })
                .collect())
        } else {
            Err(format!(
                "Sounds lookup failed: {}",
                format_api_errors(api_response.errors)
            ))
        }
    }

    async fn refresh_sounds_cache(&self) -> Result<Vec<AvailableSound>, String> {
        let sounds = self.fetch_sounds().await?;
        *self
            .sounds_cache
            .write()
            .map_err(|_| "Sounds cache lock poisoned".to_string())? = Some(sounds.clone());
        Ok(sounds)
    }

    fn cached_sounds(&self) -> Result<Option<Vec<AvailableSound>>, String> {
        self.sounds_cache
            .read()
            .map(|cache| cache.clone())
            .map_err(|_| "Sounds cache lock poisoned".to_string())
    }

    fn sound_instructions(&self) -> String {
        let Some(sounds) = self
            .sounds_cache
            .read()
            .ok()
            .and_then(|cache| cache.clone())
        else {
            return "Use list_sounds to discover built-in and custom notification sound IDs before setting send_notification.sound.".to_string();
        };

        if sounds.is_empty() {
            return "Use list_sounds to discover built-in and custom notification sound IDs before setting send_notification.sound.".to_string();
        }

        let sound_ids = sounds
            .iter()
            .map(|sound| sound.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "Notification sound IDs loaded at startup: {sound_ids}. Use list_sounds with refresh=true if the account's custom sounds changed."
        )
    }
}

#[tool_handler]
impl ServerHandler for PushoverServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: self.name.to_string(),
                version: self.version.to_string(),
                ..Default::default()
            },
            instructions: Some(format!(
                "Send push notifications to the user's devices. \
                 For most notifications, just call send_notification with a message - all other parameters are optional. \
                 Do NOT use device parameter or list_devices unless the user explicitly asks for a specific device. \
                 For urgent notifications requiring acknowledgment, use priority=\"emergency\" which returns a receipt ID \
                 that can be used with check_emergency_receipt or cancel_emergency_notification. \
                 {}",
                self.sound_instructions()
            )),
        }
    }
}
