use std::fmt;
use std::time::Duration;

/// Everything that can go wrong on the way to a phone.
#[derive(Debug)]
pub enum Error {
    /// The channel URL could not be read as one.
    InvalidUrl(String),

    /// Mast could not be reached, did not answer in time, or answered a
    /// redirect, which is never followed: the key is in the URL, so following
    /// one would hand it to whoever answered.
    Unreachable(String),

    /// The key does not resolve. Unknown, malformed, revoked and rotated away
    /// past the grace window all answer alike, so that a key cannot be probed,
    /// and a client cannot tell them apart either.
    UnknownChannel(String),

    /// The key is good, the message id is not one of its own.
    UnknownMessage(String),

    /// Over 60 requests a minute on the key, or 600 across the owner's
    /// channels.
    RateLimited { retry_after: Duration },

    /// The send was refused, either by Mast or here on its behalf. The text
    /// names the field.
    Rejected(String),

    /// A 5xx. It does not prove the message was not stored: the failure can
    /// land on either side of the durability line, so retrying blind can page
    /// somebody twice. Retry with a dedupe key, or not at all.
    Unavailable { status: u16, reason: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidUrl(reason) => write!(f, "{reason}"),
            Error::Unreachable(reason) => write!(f, "could not reach Mast: {reason}"),
            Error::UnknownChannel(reason) => write!(f, "no such channel: {reason}"),
            Error::UnknownMessage(reason) => write!(f, "no such message: {reason}"),
            Error::RateLimited { retry_after } => {
                write!(f, "rate limited, retry in {}s", retry_after.as_secs())
            }
            Error::Rejected(reason) => write!(f, "the send was refused: {reason}"),
            Error::Unavailable { status, reason } => {
                write!(f, "Mast answered {status}: {reason}")
            }
        }
    }
}

impl std::error::Error for Error {}
