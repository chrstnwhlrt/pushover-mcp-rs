//! The MCP server: tool definitions, input checks and server metadata.
//!
//! The texts the model reads were measured with Gemma 4 E4B in otomo
//! (November 2026) and are kept word for word; see `schema.rs` for the shape
//! of the input schemas.

use std::{
    borrow::Cow,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

use rmcp::{
    ErrorData, Json, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, Implementation, JsonObject, ProtocolVersion,
        ServerCapabilities, ServerConfig,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};

use crate::{
    attachment,
    config::{self, Config, trim_to_option},
    model::{
        CancelEmergencyParams, CancelEmergencyResult, CheckReceiptParams, CheckReceiptResult,
        DEFAULT_EXPIRE_SECONDS, DEFAULT_RETRY_SECONDS, ListDevicesResult, ListSoundsResult,
        MAX_DEVICE_NAME_CHARS, MAX_EXPIRE_SECONDS, MAX_MESSAGE_CHARS, MAX_SENDER_CHARS,
        MAX_TITLE_CHARS, MAX_URL_CHARS, MAX_URL_TITLE_CHARS, MIN_RETRY_SECONDS,
        MessageLimitsResult, Priority, SendNotificationParams, SendNotificationResult, SoundInfo,
    },
    pushover::PushoverClient,
    schema,
};

/// What the server tells the model about itself. Without `alarms` (the
/// client may not send emergency priority) the sentence about the two
/// emergency tools is left out, as they are; the sentence about the
/// priorities stays, a louder request is lowered with a note.
fn instructions(fixed_sender: bool, alarms: bool) -> String {
    let needs = if fixed_sender {
        "message"
    } else {
        "message and sender"
    };
    let receipts = if alarms {
        " check_emergency_receipt and cancel_emergency_notification act on the latest \
         emergency notification unless you give a receipt."
    } else {
        ""
    };
    format!(
        "Sends push notifications to the user's own phone and other devices through Pushover; \
         it reaches nobody else. send_notification needs only {needs}; leave out every other \
         parameter the user does not ask for. Without device a notification reaches all of the \
         user's devices, the phone included. Priority high is for something important or \
         urgent; emergency only for an alarm the user wants repeated until acknowledged; low for \
         no sound and no vibration; silent for no alert at all. For vibration alone use the sound \
         vibrate. A sound the user names that is not among the built-in names of the sound \
         parameter, such as a gong or a sound of their own, is one of the account's own: call \
         list_sounds and take its name.{receipts} Pushover cannot schedule: every notification \
         goes out at once."
    )
}

/// The description of `send_notification`; the tool attribute holds the
/// variant with sender and alarms.
fn send_description(fixed_sender: bool, alarms: bool) -> String {
    let required = if fixed_sender {
        "Only message is required."
    } else {
        "message and sender are required."
    };
    let receipt = if alarms {
        ", and for emergency priority a receipt"
    } else {
        ""
    };
    format!(
        "Send a push notification to the user's own devices now; it cannot reach other people. \
         {required} Returns the request ID{receipt}."
    )
}

const SEND_DESCRIPTION: &str = "Send a push notification to the user's own devices now; it \
                                cannot reach other people. message and sender are required. \
                                Returns the request ID, and for emergency priority a receipt.";

/// What the model is to tell the user when a notification went out quieter
/// than asked for, measured for every limit with Gemma 4 E4B in otomo. It
/// starts with how the notification went out: a note starting with the
/// lowered priority still let the model say now and then that it went out as
/// urgent. Below normal it does not say "louder", over which the model,
/// answering in German, stumbled in 11 of 42 answers ("laut ... laute").
fn downgrade_note(asked: Priority, limit: Priority) -> String {
    let (sent, why, went_out) = match limit {
        Priority::High => (
            "once",
            "alarms that repeat until acknowledged are switched off for this client",
            "once, with high priority",
        ),
        Priority::Normal => (
            "as an ordinary notification",
            "alarms are switched off for this client",
            "as an ordinary notification",
        ),
        _ => (
            "as a quiet notification",
            "this client may only send quiet notifications",
            "as a quiet notification",
        ),
    };
    let (asked, limit) = (asked.name(), limit.name());
    format!(
        "Sent {sent}, with {limit} priority instead of {asked}: {why} (--max-priority {limit}). \
         Tell the user that it went out {went_out}."
    )
}

#[derive(Clone)]
pub struct PushoverServer {
    config_path: Arc<PathBuf>,
    api: Arc<PushoverClient>,
    /// The sender of every notification, from `--sender`
    fixed_sender: Option<Arc<str>>,
    /// The loudest priority this client may send, from `--max-priority`
    max_priority: Priority,
    instructions: Arc<str>,
    /// Receipt of the latest emergency notification this process sent
    latest: Arc<Mutex<Option<String>>>,
    /// The account's devices and sounds, fetched on first use
    devices: Arc<tokio::sync::Mutex<Option<ListDevicesResult>>>,
    sounds: Arc<tokio::sync::Mutex<Option<Vec<SoundInfo>>>>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl PushoverServer {
    /// `fixed_sender` (from `--sender`) is the sender of every notification;
    /// the model then does not see the parameter at all. `max_priority` (from
    /// `--max-priority`) is the loudest priority sent; below emergency, the
    /// two emergency tools and the parameter `tags` are gone.
    pub fn new(
        config_path: PathBuf,
        api: PushoverClient,
        fixed_sender: Option<String>,
        max_priority: Priority,
    ) -> Self {
        let alarms = max_priority == Priority::Emergency;
        let mut tool_router = Self::tool_router();
        if !alarms {
            tool_router.remove_route("check_emergency_receipt");
            tool_router.remove_route("cancel_emergency_notification");
        }
        for route in tool_router.map.values_mut() {
            route.attr.input_schema = Arc::new(schema::flatten(&route.attr.input_schema));
        }
        if let Some(route) = tool_router.map.get_mut("send_notification") {
            let mut input = (*route.attr.input_schema).clone();
            if fixed_sender.is_some() {
                schema::hide_property(&mut input, "sender");
            }
            if !alarms {
                // retry and expire stay: measured with Gemma 4 E4B in otomo,
                // without them a request for an alarm repeated every five
                // minutes read as one to watch (0 of 6 runs, 6 of 6 with
                // them). They fall away when the priority is lowered.
                schema::hide_property(&mut input, "tags");
            }
            route.attr.input_schema = Arc::new(input);
            route.attr.description = Some(send_description(fixed_sender.is_some(), alarms).into());
        }

        Self {
            config_path: Arc::new(config_path),
            api: Arc::new(api),
            instructions: instructions(fixed_sender.is_some(), alarms).into(),
            fixed_sender: fixed_sender.map(Arc::from),
            max_priority,
            latest: Arc::default(),
            devices: Arc::default(),
            sounds: Arc::default(),
            tool_router,
        }
    }

    #[tool(
        description = SEND_DESCRIPTION,
        title = "Send notification",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn send_notification(
        &self,
        Parameters(mut params): Parameters<SendNotificationParams>,
    ) -> Result<Json<SendNotificationResult>, String> {
        // A louder priority than this client may send goes out with the limit;
        // retry, expire and tags then fall away with emergency. Pushover takes
        // a missing priority as normal, so below normal the limit is sent.
        let asked = params.priority.unwrap_or(Priority::Normal);
        let note = if asked > self.max_priority {
            tracing::info!(
                "Priority {} lowered to {} (--max-priority)",
                asked.name(),
                self.max_priority.name()
            );
            params.priority = Some(self.max_priority);
            Some(downgrade_note(asked, self.max_priority))
        } else {
            None
        };
        let config = self.config()?;
        let notification = build_notification(params, config.default_device.clone())?;
        let token = self.token(&config)?;
        let user = self.user_key(&config)?;
        let image = match notification.attachment {
            Some(path) => Some(attachment::load_image(path).await?),
            None => None,
        };
        let mut fields = notification.fields;
        if let Some(sound) = &notification.sound {
            fields.push(("sound", self.checked_sound(token, sound).await?));
        }
        if let Some(devices) = &notification.devices {
            fields.push(("device", self.checked_devices(token, user, devices).await?));
        }

        let response = self.api.send_message(token, user, fields, image).await?;
        if let Some(receipt) = &response.receipt {
            *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(receipt.clone());
        }
        tracing::info!(request = %response.request, "Notification sent");
        Ok(Json(SendNotificationResult {
            request_id: response.request,
            receipt: response.receipt,
            note,
        }))
    }

    /// List the names of the user's active devices, for the device parameter of
    /// `send_notification`. Use only when the user asks for a specific device.
    #[tool(
        title = "List devices",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_devices(&self) -> Result<Json<ListDevicesResult>, String> {
        let config = self.config()?;
        let token = self.token(&config)?;
        let user = self.user_key(&config)?;
        let response = self.api.validate_user(token, user).await?;
        Ok(Json(response.into()))
    }

    /// List the notification sounds: the built-in ones and the custom sounds of the
    /// Pushover account, with the names `send_notification` takes.
    #[tool(
        title = "List sounds",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_sounds(&self) -> Result<Json<ListSoundsResult>, String> {
        let config = self.config()?;
        let response = self.api.sounds(self.token(&config)?).await?;
        Ok(Json(response.into()))
    }

    /// Get the monthly message quota of the Pushover account: limit, messages left and
    /// when the count starts over.
    #[tool(
        title = "Get message limits",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_message_limits(&self) -> Result<Json<MessageLimitsResult>, String> {
        let config = self.config()?;
        let response = self.api.limits(self.token(&config)?).await?;
        Ok(Json(response.into()))
    }

    #[tool(
        description = "Check whether an emergency notification was acknowledged or has expired: \
                       the latest one sent, or the one of receipt.",
        title = "Check emergency receipt",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_emergency_receipt(
        &self,
        Parameters(params): Parameters<CheckReceiptParams>,
    ) -> Result<Json<CheckReceiptResult>, String> {
        let receipt = match trim_to_option(params.receipt) {
            Some(receipt) => check_receipt(&receipt)?.to_owned(),
            None => self.latest_receipt().ok_or_else(|| {
                "No emergency notification was sent yet; give a receipt.".to_owned()
            })?,
        };
        let config = self.config()?;
        let response = self.api.receipt(self.token(&config)?, &receipt).await?;
        Ok(Json(response.into()))
    }

    #[tool(
        description = "Stop the repeated alerts of an emergency notification: the latest one \
                       sent, the one of receipt, or all sent with tag.",
        title = "Cancel emergency notification",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn cancel_emergency_notification(
        &self,
        Parameters(params): Parameters<CancelEmergencyParams>,
    ) -> Result<Json<CancelEmergencyResult>, String> {
        let target = match (trim_to_option(params.receipt), trim_to_option(params.tag)) {
            (Some(receipt), None) => CancelTarget::Receipt(check_receipt(&receipt)?.to_owned()),
            (None, Some(tag)) => {
                check_tag(&tag)?;
                CancelTarget::Tag(tag)
            }
            (None, None) => CancelTarget::Receipt(self.latest_receipt().ok_or_else(|| {
                "No emergency notification was sent yet; give a receipt or a tag.".to_owned()
            })?),
            (Some(_), Some(_)) => return Err("Give either receipt or tag, not both".to_owned()),
        };
        let config = self.config()?;
        let token = self.token(&config)?;

        let message = match target {
            CancelTarget::Receipt(receipt) => {
                self.api.cancel_receipt(token, &receipt).await?;
                format!("Canceled the emergency notification with receipt {receipt}")
            }
            CancelTarget::Tag(tag) => {
                let response = self.api.cancel_by_tag(token, &tag).await?;
                match response.canceled {
                    Some(count) => {
                        format!("Canceled {count} emergency notification(s) tagged {tag:?}")
                    }
                    None => format!("Canceled the emergency notifications tagged {tag:?}"),
                }
            }
        };
        Ok(Json(CancelEmergencyResult { message }))
    }
}

impl PushoverServer {
    /// The config file as it is now; it is read on every call.
    fn config(&self) -> Result<Config, String> {
        config::load(&self.config_path).map_err(|e| format!("{e:#}"))
    }

    fn token<'a>(&self, config: &'a Config) -> Result<&'a str, String> {
        if config.token.is_empty() {
            Err(format!(
                "No Pushover API token configured. The user has to add the token of a Pushover \
                 application (https://pushover.net/apps) as \"token\" to {}.",
                self.config_path.display()
            ))
        } else {
            Ok(&config.token)
        }
    }

    fn user_key<'a>(&self, config: &'a Config) -> Result<&'a str, String> {
        config.default_user_key.as_deref().ok_or_else(|| {
            format!(
                "No Pushover user key configured. The user has to add their user key (shown \
                 on https://pushover.net after login) as \"default_user_key\" to {}.",
                self.config_path.display()
            )
        })
    }

    fn latest_receipt(&self) -> Option<String> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The devices the call names, each spelled as the account spells it.
    ///
    /// The API sends a notification for an unknown device to all devices, so
    /// a wrong name would go unnoticed; an error with the right names lets the
    /// model correct it. The list is fetched once and again when a name is
    /// missing, in case the user added the device since.
    async fn checked_devices(
        &self,
        token: &str,
        user: &str,
        names: &[String],
    ) -> Result<String, String> {
        let mut list = self.devices.lock().await;
        let mut fetched = false;
        if list.is_none() {
            *list = Some(self.api.validate_user(token, user).await?.into());
            fetched = true;
        }
        loop {
            let outcome = {
                let account = list.as_ref().expect("fetched above");
                if account.group {
                    // A delivery group picks its members' devices itself.
                    return Ok(names.join(","));
                }
                match_devices(names, &account.devices)
                    .map_err(|unknown| unknown_device(&unknown, &account.devices))
            };
            match outcome {
                Ok(devices) => return Ok(devices.join(",")),
                Err(error) if fetched => return Err(error),
                Err(_) => {
                    *list = Some(self.api.validate_user(token, user).await?.into());
                    fetched = true;
                }
            }
        }
    }

    /// The sound's name, also for its display name. The API drops an unknown
    /// sound without an error, so the model learns of a wrong one only here.
    async fn checked_sound(&self, token: &str, sound: &str) -> Result<String, String> {
        let mut list = self.sounds.lock().await;
        let mut fetched = false;
        if list.is_none() {
            *list = Some(ListSoundsResult::from(self.api.sounds(token).await?).sounds);
            fetched = true;
        }
        loop {
            let outcome = {
                let sounds = list.as_ref().expect("fetched above");
                match_sound(sound, sounds)
                    .map(str::to_owned)
                    .ok_or_else(|| unknown_sound(sound, sounds))
            };
            match outcome {
                Ok(name) => return Ok(name),
                Err(error) if fetched => return Err(error),
                Err(_) => {
                    *list = Some(ListSoundsResult::from(self.api.sounds(token).await?).sounds);
                    fetched = true;
                }
            }
        }
    }
}

// The router is named explicitly: the macro's default builds a new router on
// every request. The `list_tools` it generates is async without awaiting.
#[tool_handler(router = self.tool_router)]
#[allow(clippy::unused_async_trait_impl)]
impl ServerHandler for PushoverServer {
    /// Routes a tool call; with `--sender`, `send_notification` gets the
    /// fixed sender, which its schema no longer shows.
    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Some(sender) = &self.fixed_sender
            && request.name == "send_notification"
        {
            request
                .arguments
                .get_or_insert_with(JsonObject::new)
                .insert("sender".to_owned(), sender.as_ref().into());
        }
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    fn get_info(&self) -> ServerConfig {
        // The protocol version stays at the SDK default; `initialize` answers
        // a client with the version it asks for, or with the newest version
        // that has an initialize handshake.
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                    .with_title("Pushover")
                    .with_description(env!("CARGO_PKG_DESCRIPTION"))
                    .with_website_url(env!("CARGO_PKG_REPOSITORY")),
            )
            .with_instructions(self.instructions.to_string())
    }

    /// The protocol versions this server is tested with, pinned so that an
    /// rmcp update cannot widen the advertised range unnoticed. 2026-07-28
    /// has no `initialize`; clients reach it through `server/discover`.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(ProtocolVersion::known_up_to(&ProtocolVersion::V_2026_07_28))
    }
}

enum CancelTarget {
    Receipt(String),
    Tag(String),
}

/// A checked notification: the form fields of `messages.json` without token
/// and user, and what still needs the account's lists or a file.
#[derive(Debug, PartialEq, Eq)]
struct Notification {
    fields: Vec<(&'static str, String)>,
    /// Devices the call names, checked against the account's before sending
    devices: Option<Vec<String>>,
    /// Sound the call names, checked against the account's before sending
    sound: Option<String>,
    attachment: Option<PathBuf>,
}

/// Checks the parameters against the API's limits and turns them into form
/// fields. Strings are trimmed; blank optional strings count as unset.
fn build_notification(
    params: SendNotificationParams,
    default_device: Option<String>,
) -> Result<Notification, String> {
    let SendNotificationParams {
        message,
        sender,
        title,
        priority,
        sound,
        device,
        url,
        url_title,
        html,
        monospace,
        attachment_path,
        ttl,
        timestamp,
        retry,
        expire,
        tags,
    } = params;

    let message = message.trim();
    if message.is_empty() {
        return Err("message must not be empty".to_owned());
    }
    check_length("message", message, MAX_MESSAGE_CHARS)?;
    let mut fields = vec![("message", message.to_owned())];

    fields.push(("title", title_with_sender(&sender, title)?));

    let url = trim_to_option(url);
    let url_title = trim_to_option(url_title);
    if url_title.is_some() && url.is_none() {
        return Err("url_title requires url".to_owned());
    }
    if let Some(url) = url {
        check_length("url", &url, MAX_URL_CHARS)?;
        fields.push(("url", url));
    }
    if let Some(url_title) = url_title {
        check_length("url_title", &url_title, MAX_URL_TITLE_CHARS)?;
        fields.push(("url_title", url_title));
    }

    match (html == Some(true), monospace == Some(true)) {
        (true, true) => return Err("html and monospace cannot be combined".to_owned()),
        (true, false) => fields.push(("html", "1".to_owned())),
        (false, true) => fields.push(("monospace", "1".to_owned())),
        (false, false) => {}
    }

    // Devices of the call are checked against the account before sending;
    // the default from the config is the user's own choice and goes as it is.
    let devices = trim_to_option(device)
        .map(|device| device_names(&device))
        .transpose()?;
    if devices.is_none()
        && let Some(device) = default_device
    {
        let names =
            device_names(&device).map_err(|e| format!("default_device in the config file: {e}"))?;
        fields.push(("device", names.join(",")));
    }
    if let Some(timestamp) = timestamp {
        fields.push(("timestamp", timestamp.to_string()));
    }

    if let Some(priority) = priority {
        fields.push(("priority", priority.to_api_value().to_string()));
    }
    if priority == Some(Priority::Emergency) {
        fields.extend(emergency_fields(retry, expire, tags.as_deref())?);
    } else if let Some(ttl) = ttl {
        // Pushover ignores ttl for emergency priority, so it is only checked
        // and sent for the others.
        if ttl == 0 {
            return Err("ttl must be at least 1 second".to_owned());
        }
        fields.push(("ttl", ttl.to_string()));
    }

    let attachment = trim_to_option(attachment_path).map(PathBuf::from);
    if let Some(path) = &attachment
        && !path.is_absolute()
    {
        return Err(format!(
            "attachment_path must be an absolute path (got {})",
            path.display()
        ));
    }

    Ok(Notification {
        fields,
        devices,
        sound: trim_to_option(sound),
        attachment,
    })
}

/// The title: the sender, followed by the title if there is one. The sender
/// makes every notification show which session it comes from.
fn title_with_sender(sender: &str, title: Option<String>) -> Result<String, String> {
    let sender = sender.trim();
    if sender.is_empty() {
        return Err(
            "sender must not be empty: your own name, with the session or project you work in, \
             e.g. \"Claude (shop backend)\""
                .to_owned(),
        );
    }
    check_length("sender", sender, MAX_SENDER_CHARS)?;
    let Some(title) = trim_to_option(title) else {
        return Ok(sender.to_owned());
    };
    let full = format!("{sender}: {title}");
    let chars = full.chars().count();
    if chars > MAX_TITLE_CHARS {
        return Err(format!(
            "sender and title together have {chars} characters (as \"sender: title\"); \
             at most {MAX_TITLE_CHARS} are allowed"
        ));
    }
    Ok(full)
}

/// Retry interval, retry duration (with defaults) and tags of an emergency
/// notification.
fn emergency_fields(
    retry: Option<u32>,
    expire: Option<u32>,
    tags: Option<&[String]>,
) -> Result<Vec<(&'static str, String)>, String> {
    let retry = retry.unwrap_or(DEFAULT_RETRY_SECONDS);
    if retry < MIN_RETRY_SECONDS {
        return Err(format!(
            "retry must be at least {MIN_RETRY_SECONDS} seconds (got {retry})"
        ));
    }
    let expire = expire.unwrap_or(DEFAULT_EXPIRE_SECONDS);
    if !(1..=MAX_EXPIRE_SECONDS).contains(&expire) {
        return Err(format!(
            "expire must be between 1 and {MAX_EXPIRE_SECONDS} seconds (got {expire})"
        ));
    }
    let mut fields = vec![("retry", retry.to_string()), ("expire", expire.to_string())];
    let tags = tag_list(tags.unwrap_or_default())?;
    if !tags.is_empty() {
        fields.push(("tags", tags));
    }
    Ok(fields)
}

/// Lengths count characters, as the API does.
fn check_length(name: &str, value: &str, max: usize) -> Result<(), String> {
    let chars = value.chars().count();
    if chars > max {
        Err(format!(
            "{name} has {chars} characters; at most {max} are allowed"
        ))
    } else {
        Ok(())
    }
}

/// Splits a comma-separated device list and checks each name
/// (`[A-Za-z0-9_-]`, at most 25 characters).
fn device_names(devices: &str) -> Result<Vec<String>, String> {
    devices
        .split(',')
        .map(str::trim)
        .map(|name| {
            let valid = !name.is_empty()
                && name.chars().count() <= MAX_DEVICE_NAME_CHARS
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if valid {
                Ok(name.to_owned())
            } else {
                Err(format!(
                    "Invalid device name {name:?}: device names have 1 to {MAX_DEVICE_NAME_CHARS} \
                     letters, digits, _ or -; several are joined with commas. list_devices shows \
                     the names."
                ))
            }
        })
        .collect()
}

/// Each requested device as the account spells it, matched without regard to
/// case; the first unknown name otherwise.
fn match_devices(requested: &[String], devices: &[String]) -> Result<Vec<String>, String> {
    requested
        .iter()
        .map(|name| {
            devices
                .iter()
                .find(|device| *device == name)
                .or_else(|| {
                    devices
                        .iter()
                        .find(|device| device.eq_ignore_ascii_case(name))
                })
                .cloned()
                .ok_or_else(|| name.clone())
        })
        .collect()
}

fn unknown_device(name: &str, devices: &[String]) -> String {
    format!(
        "Unknown device '{name}'. The user's devices are {}: send again with the one the user \
         means (a phone, a laptop: tell it by its name), or omit device to reach all of them.",
        devices.join(", ")
    )
}

/// The name of the sound the user means: its name, or a sound whose name or
/// display name matches without regard to case ("Küchengong" for kuechengong).
fn match_sound<'a>(requested: &str, sounds: &'a [SoundInfo]) -> Option<&'a str> {
    let wanted = requested.to_lowercase();
    sounds
        .iter()
        .find(|sound| sound.name == requested)
        .or_else(|| {
            sounds.iter().find(|sound| {
                sound.name.to_lowercase() == wanted || sound.description.to_lowercase() == wanted
            })
        })
        .map(|sound| sound.name.as_str())
}

fn unknown_sound(name: &str, sounds: &[SoundInfo]) -> String {
    let named: Vec<String> = sounds
        .iter()
        .map(|sound| format!("{} ({})", sound.name, sound.description))
        .collect();
    format!(
        "Unknown sound '{name}'. The sounds are {}. Send again with the name that fits what the \
         user asked for, or omit sound for the user's default.",
        named.join(", ")
    )
}

/// Joins tags with commas, the API's list separator; blank tags are dropped.
fn tag_list(tags: &[String]) -> Result<String, String> {
    let mut list = Vec::with_capacity(tags.len());
    for tag in tags {
        let tag = tag.trim();
        if !tag.is_empty() {
            check_tag(tag)?;
            list.push(tag);
        }
    }
    Ok(list.join(","))
}

/// A tag is a path segment when canceling, and the API separates tags with
/// commas, so neither may appear in one.
fn check_tag(tag: &str) -> Result<(), String> {
    if tag.contains([',', '/']) || tag.chars().any(char::is_control) {
        Err(format!(
            "Invalid tag {tag:?}: tags must not contain commas, slashes or control characters"
        ))
    } else {
        Ok(())
    }
}

/// Receipts are 30 letters and digits; anything else would end up in the URL path.
fn check_receipt(receipt: &str) -> Result<&str, String> {
    let receipt = receipt.trim();
    if receipt.is_empty() || !receipt.chars().all(|c| c.is_ascii_alphanumeric()) {
        Err(format!(
            "Invalid receipt {receipt:?}: use the receipt send_notification returned for an \
             emergency notification"
        ))
    } else {
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(message: &str) -> SendNotificationParams {
        SendNotificationParams {
            message: message.to_owned(),
            sender: "Claude (tests)".to_owned(),
            ..Default::default()
        }
    }

    fn fields(notification: &Notification) -> Vec<(&str, &str)> {
        notification
            .fields
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect()
    }

    fn sounds() -> Vec<SoundInfo> {
        [
            ("bike", "Bike"),
            ("kuechengong", "Küchengong"),
            ("pushover", "Pushover (default)"),
        ]
        .into_iter()
        .map(|(name, description)| SoundInfo {
            name: name.to_owned(),
            description: description.to_owned(),
        })
        .collect()
    }

    #[test]
    fn message_and_sender_are_enough() {
        let notification = build_notification(
            SendNotificationParams {
                sender: "  Claude (pushover mcp) ".into(),
                ..params("  Build finished \n")
            },
            None,
        )
        .unwrap();
        assert_eq!(
            fields(&notification),
            [
                ("message", "Build finished"),
                ("title", "Claude (pushover mcp)")
            ]
        );
        assert_eq!(notification.devices, None);
        assert_eq!(notification.sound, None);
        assert_eq!(notification.attachment, None);
    }

    #[test]
    fn all_parameters_become_fields() {
        let notification = build_notification(
            SendNotificationParams {
                title: Some(" Deploy ".into()),
                priority: Some(Priority::High),
                sound: Some(" siren ".into()),
                device: Some("iphone, pixel_8".into()),
                url: Some("https://example.com".into()),
                url_title: Some("Open".into()),
                html: Some(true),
                monospace: Some(false),
                attachment_path: Some("/tmp/chart.png".into()),
                ttl: Some(600),
                timestamp: Some(1_331_249_662),
                retry: Some(5),
                expire: Some(99_999),
                tags: Some(vec!["ignored".into()]),
                ..params("done")
            },
            Some("ignored-default".into()),
        )
        .unwrap();
        assert_eq!(
            fields(&notification),
            [
                ("message", "done"),
                ("title", "Claude (tests): Deploy"),
                ("url", "https://example.com"),
                ("url_title", "Open"),
                ("html", "1"),
                ("timestamp", "1331249662"),
                ("priority", "1"),
                ("ttl", "600"),
            ]
        );
        // Sound and devices wait for the check against the account.
        assert_eq!(notification.sound.as_deref(), Some("siren"));
        assert_eq!(
            notification.devices,
            Some(vec!["iphone".to_owned(), "pixel_8".to_owned()])
        );
        assert_eq!(
            notification.attachment,
            Some(PathBuf::from("/tmp/chart.png"))
        );
    }

    #[test]
    fn emergency_gets_retry_expire_and_tags_but_no_ttl() {
        let notification = build_notification(
            SendNotificationParams {
                priority: Some(Priority::Emergency),
                ttl: Some(60),
                tags: Some(vec![" s=mail01 ".into(), String::new(), "l=chicago".into()]),
                ..params("Server down")
            },
            None,
        )
        .unwrap();
        assert_eq!(
            fields(&notification),
            [
                ("message", "Server down"),
                ("title", "Claude (tests)"),
                ("priority", "2"),
                ("retry", "60"),
                ("expire", "3600"),
                ("tags", "s=mail01,l=chicago"),
            ]
        );
    }

    #[test]
    fn the_default_device_goes_unchecked_when_none_is_given() {
        let notification = build_notification(params("hi"), Some("pixel".into())).unwrap();
        assert_eq!(
            fields(&notification),
            [
                ("message", "hi"),
                ("title", "Claude (tests)"),
                ("device", "pixel")
            ]
        );
        assert_eq!(notification.devices, None);
        let notification = build_notification(
            SendNotificationParams {
                device: Some(" ".into()),
                ..params("hi")
            },
            Some("pixel".into()),
        )
        .unwrap();
        assert_eq!(
            fields(&notification),
            [
                ("message", "hi"),
                ("title", "Claude (tests)"),
                ("device", "pixel")
            ]
        );
    }

    #[test]
    fn limits_count_characters() {
        let at_limit = "ä".repeat(MAX_MESSAGE_CHARS);
        assert!(build_notification(params(&at_limit), None).is_ok());
        let error = build_notification(params(&format!("{at_limit}x")), None).unwrap_err();
        assert_eq!(
            error,
            "message has 1025 characters; at most 1024 are allowed"
        );

        // "Claude (tests): " has 16 characters.
        let title_at_limit = SendNotificationParams {
            title: Some("t".repeat(MAX_TITLE_CHARS - 16)),
            ..params("x")
        };
        assert!(build_notification(title_at_limit, None).is_ok());
    }

    fn assert_rejected(cases: Vec<(SendNotificationParams, &str)>) {
        for (params, expected) in cases {
            let error = build_notification(params, None).unwrap_err();
            assert!(error.starts_with(expected), "{error:?} vs {expected:?}");
        }
    }

    #[test]
    fn invalid_texts_are_rejected() {
        assert_rejected(vec![
            (params("   "), "message must not be empty"),
            (
                SendNotificationParams {
                    sender: " ".into(),
                    ..params("x")
                },
                "sender must not be empty",
            ),
            (
                SendNotificationParams {
                    sender: "s".repeat(65),
                    ..params("x")
                },
                "sender has 65 characters; at most 64 are allowed",
            ),
            (
                SendNotificationParams {
                    title: Some("t".repeat(235)),
                    ..params("x")
                },
                "sender and title together have 251 characters",
            ),
            (
                SendNotificationParams {
                    url: Some(format!("https://{}", "a".repeat(510))),
                    ..params("x")
                },
                "url has 518 characters",
            ),
            (
                SendNotificationParams {
                    url_title: Some("Open".into()),
                    ..params("x")
                },
                "url_title requires url",
            ),
            (
                SendNotificationParams {
                    url: Some("https://example.com".into()),
                    url_title: Some("u".repeat(101)),
                    ..params("x")
                },
                "url_title has 101 characters",
            ),
            (
                SendNotificationParams {
                    html: Some(true),
                    monospace: Some(true),
                    ..params("x")
                },
                "html and monospace cannot be combined",
            ),
            (
                SendNotificationParams {
                    attachment_path: Some("chart.png".into()),
                    ..params("x")
                },
                "attachment_path must be an absolute path (got chart.png)",
            ),
        ]);
    }

    #[test]
    fn invalid_devices_tags_and_timings_are_rejected() {
        assert_rejected(vec![
            (
                SendNotificationParams {
                    device: Some("Christian's iPhone".into()),
                    ..params("x")
                },
                "Invalid device name \"Christian's iPhone\"",
            ),
            (
                SendNotificationParams {
                    device: Some("iphone,,pixel".into()),
                    ..params("x")
                },
                "Invalid device name \"\"",
            ),
            (
                SendNotificationParams {
                    device: Some("a".repeat(26)),
                    ..params("x")
                },
                "Invalid device name",
            ),
            (
                SendNotificationParams {
                    priority: Some(Priority::Emergency),
                    retry: Some(29),
                    ..params("x")
                },
                "retry must be at least 30 seconds (got 29)",
            ),
            (
                SendNotificationParams {
                    priority: Some(Priority::Emergency),
                    expire: Some(10_801),
                    ..params("x")
                },
                "expire must be between 1 and 10800 seconds (got 10801)",
            ),
            (
                SendNotificationParams {
                    priority: Some(Priority::Emergency),
                    expire: Some(0),
                    ..params("x")
                },
                "expire must be between 1 and 10800 seconds (got 0)",
            ),
            (
                SendNotificationParams {
                    priority: Some(Priority::Emergency),
                    tags: Some(vec!["a,b".into()]),
                    ..params("x")
                },
                "Invalid tag \"a,b\"",
            ),
            (
                SendNotificationParams {
                    ttl: Some(0),
                    ..params("x")
                },
                "ttl must be at least 1 second",
            ),
        ]);
        // The default device from the config is checked, too.
        let error = build_notification(params("x"), Some("my phone".into())).unwrap_err();
        assert!(
            error.starts_with("default_device in the config file: Invalid device name"),
            "{error}"
        );
    }

    #[test]
    fn devices_are_matched_to_the_accounts_spelling() {
        let devices = ["pixel9".to_owned(), "thinkpad".to_owned()];
        assert_eq!(
            match_devices(&["Pixel9".to_owned(), "thinkpad".to_owned()], &devices),
            Ok(vec!["pixel9".to_owned(), "thinkpad".to_owned()])
        );
        assert_eq!(
            match_devices(&["thinkpad".to_owned(), "Laptop".to_owned()], &devices),
            Err("Laptop".to_owned())
        );
        assert_eq!(
            unknown_device("Laptop", &devices),
            "Unknown device 'Laptop'. The user's devices are pixel9, thinkpad: send again with \
             the one the user means (a phone, a laptop: tell it by its name), or omit device to \
             reach all of them."
        );
    }

    #[test]
    fn sounds_are_matched_by_name_and_display_name() {
        let sounds = sounds();
        assert_eq!(match_sound("bike", &sounds), Some("bike"));
        assert_eq!(match_sound("Bike", &sounds), Some("bike"));
        assert_eq!(match_sound("küchengong", &sounds), Some("kuechengong"));
        assert_eq!(match_sound("KÜCHENGONG", &sounds), Some("kuechengong"));
        assert_eq!(match_sound("Fahrradklingel", &sounds), None);
        assert_eq!(
            unknown_sound("Fahrradklingel", &sounds),
            "Unknown sound 'Fahrradklingel'. The sounds are bike (Bike), kuechengong \
             (Küchengong), pushover (Pushover (default)). Send again with the name that fits \
             what the user asked for, or omit sound for the user's default."
        );
    }

    #[test]
    fn receipts_and_tags_cannot_leave_their_path_segment() {
        assert_eq!(
            check_receipt(" KAWxmB7eX5spZ1aR3H4hQNa5jxzq5j "),
            Ok("KAWxmB7eX5spZ1aR3H4hQNa5jxzq5j")
        );
        for receipt in ["", "../users/validate", "abc?token=x", "abc def"] {
            assert!(check_receipt(receipt).is_err(), "{receipt}");
        }
        assert!(check_tag("l=chicago").is_ok());
        for tag in ["a/b", "a,b", "a\nb"] {
            assert!(check_tag(tag).is_err(), "{tag:?}");
        }
    }

    #[test]
    fn the_texts_follow_sender_and_alarms() {
        // The full variant is the one in the tool attribute.
        assert_eq!(send_description(false, true), SEND_DESCRIPTION);
        assert_eq!(
            send_description(true, false),
            "Send a push notification to the user's own devices now; it cannot reach other \
             people. Only message is required. Returns the request ID."
        );
        let full = instructions(false, true);
        assert!(full.contains("send_notification needs only message and sender;"));
        assert!(full.contains(
            "take its name. check_emergency_receipt and cancel_emergency_notification act on the \
             latest emergency notification unless you give a receipt. Pushover cannot schedule"
        ));
        let otomo = instructions(true, false);
        assert!(otomo.contains("send_notification needs only message;"));
        assert!(otomo.contains("take its name. Pushover cannot schedule"));
        // The priorities stay described either way.
        assert!(otomo.contains("emergency only for an alarm"));
    }

    #[test]
    fn a_lowered_priority_tells_what_to_say() {
        assert_eq!(
            downgrade_note(Priority::High, Priority::Normal),
            "Sent as an ordinary notification, with normal priority instead of high: alarms are \
             switched off for this client (--max-priority normal). Tell the user that it went out \
             as an ordinary notification."
        );
        assert_eq!(
            downgrade_note(Priority::Emergency, Priority::High),
            "Sent once, with high priority instead of emergency: alarms that repeat until \
             acknowledged are switched off for this client (--max-priority high). Tell the user \
             that it went out once, with high priority."
        );
        assert_eq!(
            downgrade_note(Priority::Normal, Priority::Low),
            "Sent as a quiet notification, with low priority instead of normal: this client may \
             only send quiet notifications (--max-priority low). Tell the user that it went out \
             as a quiet notification."
        );
    }
}
