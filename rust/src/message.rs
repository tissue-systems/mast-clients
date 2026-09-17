use std::time::Duration;

use crate::url::encode;
use crate::Error;

/// How loudly a message arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Delivered, no sound, no banner.
    Quiet,
    Normal,
    /// Breaks through a silenced phone.
    Loud,
    /// Repeats until somebody acknowledges it. The one tier storm control will
    /// not fold, so give a page a dedupe key if its source can flap.
    Page,
}

impl Priority {
    fn as_str(self) -> &'static str {
        match self {
            Priority::Quiet => "quiet",
            Priority::Normal => "normal",
            Priority::Loud => "loud",
            Priority::Page => "page",
        }
    }
}

/// The alert tones the app ships. A name it does not have is not an error on
/// the phone: iOS plays its own tone and reports nothing, so a typo would be a
/// page that silently loses its alarm. Here it cannot compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Default,
    Bleep,
    Chirp,
    Klaxon,
    Pager,
    Rising,
}

impl Sound {
    fn as_str(self) -> &'static str {
        match self {
            Sound::Default => "default",
            Sound::Bleep => "bleep",
            Sound::Chirp => "chirp",
            Sound::Klaxon => "klaxon",
            Sound::Pager => "pager",
            Sound::Rising => "rising",
        }
    }
}

// The server's own caps, mirrored here so a send that cannot be accepted costs
// none of the channel's 60 requests a minute.
const MAX_TITLE_CHARS: usize = 250;
const MAX_BODY_CHARS: usize = 4096;
const MAX_URL_CHARS: usize = 512;
const MAX_DEDUPE_KEY_CHARS: usize = 120;

const MIN_RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(86_400);
const MIN_EXPIRE: Duration = Duration::from_secs(60);
const MAX_EXPIRE: Duration = Duration::from_secs(604_800);

/// One send. Everything is optional: a `Message::new()` with nothing on it is a
/// vital's heartbeat.
#[derive(Debug, Clone, Default)]
pub struct Message {
    title: Option<String>,
    body: Option<String>,
    priority: Option<Priority>,
    sound: Option<Sound>,
    url: Option<String>,
    url_title: Option<String>,
    callback: Option<String>,
    dedupe_key: Option<String>,
    retry: Option<Option<Duration>>,
    expire: Option<Option<Duration>>,
    ack: bool,
    silent: bool,
    timestamp: Option<i64>,
    resolve: Option<String>,
}

impl Message {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = Some(priority);
        self
    }

    pub fn sound(mut self, sound: Sound) -> Self {
        self.sound = Some(sound);
        self
    }

    /// A link on the card: the dashboard, the runbook.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn url_title(mut self, title: impl Into<String>) -> Self {
        self.url_title = Some(title.into());
        self
    }

    /// Stored on the message and, for now, never fired. Poll instead.
    pub fn callback(mut self, url: impl Into<String>) -> Self {
        self.callback = Some(url.into());
        self
    }

    /// Folds a second send into the card an earlier one opened, and is what
    /// [`Channel::resolve_key`](crate::Channel::resolve_key) closes later.
    pub fn dedupe_key(mut self, key: impl Into<String>) -> Self {
        self.dedupe_key = Some(key.into());
        self
    }

    /// How often a page repeats until it is answered.
    pub fn retry(mut self, every: Duration) -> Self {
        self.retry = Some(Some(every));
        self
    }

    /// Turn the channel's own retry off, which the wire spells as a literal 0.
    pub fn no_retry(mut self) -> Self {
        self.retry = Some(None);
        self
    }

    /// How long a page keeps repeating before it gives up.
    pub fn expire(mut self, after: Duration) -> Self {
        self.expire = Some(Some(after));
        self
    }

    /// Turn the channel's own expiry off.
    pub fn no_expire(mut self) -> Self {
        self.expire = Some(None);
        self
    }

    /// Keep repeating until somebody answers.
    pub fn ack(mut self) -> Self {
        self.ack = true;
        self
    }

    /// Deliver without a sound. Cannot be combined with `ack` or the page tier,
    /// which exist to make a noise.
    pub fn silent(mut self) -> Self {
        self.silent = true;
        self
    }

    /// When the event happened, if that is not now.
    pub fn timestamp(mut self, unix_seconds: i64) -> Self {
        self.timestamp = Some(unix_seconds);
        self
    }

    pub(crate) fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = Some(priority);
        self
    }

    pub(crate) fn with_resolve(mut self, resolve: impl Into<String>) -> Self {
        self.resolve = Some(resolve.into());
        self
    }

    pub(crate) fn into_form(self) -> Result<String, Error> {
        let mut fields: Vec<(&str, String)> = Vec::new();

        capped(&mut fields, "title", self.title.as_deref(), MAX_TITLE_CHARS)?;
        capped(&mut fields, "body", self.body.as_deref(), MAX_BODY_CHARS)?;
        capped(
            &mut fields,
            "url_title",
            self.url_title.as_deref(),
            MAX_TITLE_CHARS,
        )?;
        capped(
            &mut fields,
            "key",
            self.dedupe_key.as_deref(),
            MAX_DEDUPE_KEY_CHARS,
        )?;

        if let Some(priority) = self.priority {
            fields.push(("priority", priority.as_str().to_string()));
        }
        if let Some(sound) = self.sound {
            fields.push(("sound", sound.as_str().to_string()));
        }

        if let Some(url) = self.url.as_deref() {
            capped(&mut fields, "url", Some(url), MAX_URL_CHARS)?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(Error::Rejected(
                    "url has to start with http:// or https://".into(),
                ));
            }
        }
        if let Some(callback) = self.callback.as_deref() {
            capped(&mut fields, "callback", Some(callback), MAX_URL_CHARS)?;
            if !callback.starts_with("https://") {
                return Err(Error::Rejected(
                    "callback has to start with https://".into(),
                ));
            }
        }

        if let Some(retry) = self.retry {
            let seconds = whole_seconds("retry", retry, MIN_RETRY, MAX_RETRY)?;
            fields.push(("retry", seconds.to_string()));
        }
        if let Some(expire) = self.expire {
            let seconds = whole_seconds("expire", expire, MIN_EXPIRE, MAX_EXPIRE)?;
            fields.push(("expire", seconds.to_string()));
        }

        if self.ack {
            fields.push(("ack", "required".to_string()));
        }
        if self.silent {
            if self.ack || self.priority == Some(Priority::Page) {
                return Err(Error::Rejected(
                    "silent cannot be combined with ack or the page tier".into(),
                ));
            }
            fields.push(("silent", "1".to_string()));
        }

        if let Some(timestamp) = self.timestamp {
            fields.push(("timestamp", timestamp.to_string()));
        }
        if let Some(resolve) = self.resolve {
            fields.push(("resolve", resolve));
        }

        Ok(fields
            .iter()
            .map(|(name, value)| format!("{name}={}", encode(value)))
            .collect::<Vec<_>>()
            .join("&"))
    }
}

/// Counts characters rather than bytes, which is how the server counts them.
fn capped(
    fields: &mut Vec<(&str, String)>,
    name: &'static str,
    value: Option<&str>,
    limit: usize,
) -> Result<(), Error> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.chars().count() > limit {
        return Err(Error::Rejected(format!(
            "{name} is longer than {limit} characters"
        )));
    }
    fields.push((name, value.to_string()));
    Ok(())
}

fn whole_seconds(
    name: &str,
    value: Option<Duration>,
    low: Duration,
    high: Duration,
) -> Result<u64, Error> {
    // None is the caller asking for the channel's own default to be turned off,
    // which every layer spells as 0.
    let Some(value) = value else {
        return Ok(0);
    };
    if value.subsec_nanos() != 0 {
        return Err(Error::Rejected(format!(
            "{name} has to be a whole number of seconds"
        )));
    }
    if value < low || value > high {
        return Err(Error::Rejected(format!(
            "{name} has to be between {}s and {}s, or turned off",
            low.as_secs(),
            high.as_secs()
        )));
    }
    Ok(value.as_secs())
}
