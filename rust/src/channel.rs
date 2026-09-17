use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::message::{Message, Priority};
use crate::status::{Sent, Status};
use crate::transport::{Method, Request, Response, Transport};
use crate::url::{is_message_id, parse_channel_url};
use crate::Error;
#[cfg(feature = "tokio")]
use crate::Poller;

/// One channel: its URL, and the way to reach it.
pub struct Channel {
    origin: String,
    prefix: String,
    key: String,
    transport: Arc<dyn Transport>,
}

impl Channel {
    /// Parse a channel URL and use the bundled transport.
    #[cfg(feature = "reqwest")]
    pub fn open(raw: &str) -> Result<Channel, Error> {
        Channel::with_transport(raw, Arc::new(crate::ReqwestTransport::new()?))
    }

    /// Parse a channel URL and reach it through a transport of your own.
    pub fn with_transport(raw: &str, transport: Arc<dyn Transport>) -> Result<Channel, Error> {
        let (origin, prefix, key) = parse_channel_url(raw)?;
        Ok(Channel {
            origin,
            prefix,
            key,
            transport,
        })
    }

    pub fn url(&self) -> String {
        format!("{}{}/{}", self.origin, self.prefix, self.key)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The whole credential. Keep it out of logs: [`Display`](fmt::Display)
    /// shows only enough of it to tell two channels apart.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Post one message. This returns once Mast has stored it, which is before
    /// the push leaves for Apple: it is not a promise that a phone made a
    /// noise.
    pub async fn send(&self, message: Message) -> Result<Sent, Error> {
        let form = message.into_form()?;
        self.post(self.url(), form).await
    }

    /// Send at the pager tier, which repeats until somebody acknowledges it.
    pub async fn page(&self, message: Message) -> Result<Sent, Error> {
        self.send(message.with_priority(Priority::Page)).await
    }

    /// Close whatever that dedupe key has open: the card turns green and stops
    /// repeating. A resolve that finds nothing open is not an error; it is
    /// stored quietly.
    pub async fn resolve_key(&self, key: &str, message: Message) -> Result<Sent, Error> {
        if key.is_empty() {
            return Err(Error::Rejected("a resolve needs a dedupe key".into()));
        }
        self.send(message.dedupe_key(key).with_resolve("1")).await
    }

    /// Close one specific card by the id its own send returned.
    pub async fn resolve_message(&self, message_id: &str, message: Message) -> Result<Sent, Error> {
        if !is_message_id(message_id) {
            return Err(Error::Rejected(format!(
                "{message_id:?} is not a message id"
            )));
        }
        self.send(message.with_resolve(message_id)).await
    }

    /// A heartbeat on a vital. An empty message is proof of life and no card.
    pub async fn ping(&self, message: Message) -> Result<Sent, Error> {
        self.send(message).await
    }

    /// Flatline a vital now rather than waiting out its grace window. Only a
    /// vital has this route; a plain channel answers the 404 an unknown key
    /// gets.
    pub async fn fail(&self, message: Message) -> Result<Sent, Error> {
        let form = message.into_form()?;
        self.post(format!("{}/fail", self.url()), form).await
    }

    /// Read one message.
    pub async fn status(&self, message_id: &str) -> Result<Status, Error> {
        if !is_message_id(message_id) {
            return Err(Error::Rejected(format!(
                "{message_id:?} is not a message id"
            )));
        }
        let payload = self.get(self.message_url(message_id)).await?;
        Ok(Status::from_json(&payload))
    }

    /// Prove the key works without sending anything.
    ///
    /// The status route resolves the key before it validates the message id, so
    /// an id that cannot exist separates the two 404s: a bad key answers "No
    /// such channel." and a good one "No such message." Nothing is stored and
    /// no phone goes off, which is what makes this usable in a setup dialog.
    pub async fn check(&self) -> Result<(), Error> {
        match self.get(self.message_url("check")).await {
            Err(Error::UnknownMessage(_)) => Ok(()),
            Err(err) => Err(err),
            Ok(_) => Err(Error::Unreachable(
                "the key check was answered in a way this version does not expect".into(),
            )),
        }
    }

    /// Poll one message until it settles or the timeout runs out. A timeout
    /// hands back the last status read: the page is still open on the phone,
    /// where it can still be answered.
    #[cfg(feature = "tokio")]
    pub async fn wait(&self, message_id: &str, timeout: Option<Duration>) -> Result<Status, Error> {
        let mut poller = Poller::new(self);
        poller.watch(message_id)?;
        let settled = poller.run(timeout).await?;
        if let Some(status) = settled.get(message_id) {
            return Ok(status.clone());
        }
        if let Some(err) = poller.take_error(message_id) {
            return Err(err);
        }
        poller
            .last(message_id)
            .cloned()
            .ok_or_else(|| Error::Unreachable("the message was never read".into()))
    }

    fn message_url(&self, message_id: &str) -> String {
        format!("{}/messages/{}", self.url(), message_id)
    }

    async fn post(&self, url: String, form: String) -> Result<Sent, Error> {
        let (status, payload) = self
            .request(Request {
                method: Method::Post,
                url,
                body: Some(form),
            })
            .await?;

        match status {
            400 => Err(Error::Rejected(message(&payload, "the send was refused"))),
            413 => Err(Error::Rejected(
                "the request is over Mast's 16 KiB limit".into(),
            )),
            status if status >= 500 => Err(Error::Unavailable {
                status,
                reason: message(&payload, "no reason given"),
            }),
            status if status >= 400 => Err(Error::Unreachable(format!(
                "Mast answered {status}: {}",
                message(&payload, "no reason given")
            ))),
            _ => Ok(Sent::from_json(&payload)),
        }
    }

    async fn get(&self, url: String) -> Result<Value, Error> {
        let (status, payload) = self
            .request(Request {
                method: Method::Get,
                url,
                body: None,
            })
            .await?;
        if status >= 400 {
            return Err(Error::Unreachable(format!(
                "Mast answered {status}: {}",
                message(&payload, "no reason given")
            )));
        }
        Ok(payload)
    }

    async fn request(&self, request: Request) -> Result<(u16, Value), Error> {
        let Response {
            status,
            retry_after,
            body,
        } = self.transport.execute(request).await?;

        if (300..400).contains(&status) {
            return Err(Error::Unreachable(
                "Mast answered a redirect, which is not followed".into(),
            ));
        }

        // A proxy in front of a self-hosted edge can answer with something that
        // is not JSON at all, and that is not worth a separate error.
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

        match status {
            429 => Err(Error::RateLimited {
                retry_after: retry_after.unwrap_or(Duration::from_secs(1)),
            }),
            404 => {
                let reason = message(&payload, "No such channel.");
                if reason.contains("No such message") {
                    Err(Error::UnknownMessage(reason))
                } else {
                    Err(Error::UnknownChannel(reason))
                }
            }
            _ => Ok((status, payload)),
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "mast channel {}{}/{}...",
            self.origin,
            self.prefix,
            &self.key[..7]
        )
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Channel({self})")
    }
}

/// Digs out the human half of an error body. Mast nests it:
/// `{"error": {"code": ..., "message": ...}}`. Reading `payload["message"]`
/// finds nothing and falls back to whatever default the caller passed, which
/// turns a good key's "No such message." into "No such channel." and fails
/// setup for every valid key.
fn message(payload: &Value, fallback: &str) -> String {
    payload
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or(error.as_str())
        })
        .or_else(|| payload.get("message").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .unwrap_or(fallback)
        .to_string()
}
