//! Send to a Mast channel and wait for the acknowledgement.
//!
//! The channel key in the URL is the whole credential, so there is nothing to
//! configure and nothing to sign. There is also nothing inbound: Mast stores a
//! callback on a message but does not fire it, and most senders are not
//! reachable from the internet anyway, so an acknowledgement is polled.
//!
//! ```no_run
//! # async fn example() -> Result<(), mast::Error> {
//! use mast::{Channel, Message};
//! use std::time::Duration;
//!
//! let channel = Channel::open(&std::env::var("MAST_URL").unwrap())?;
//!
//! let sent = channel
//!     .page(
//!         Message::new()
//!             .title("s10 disk")
//!             .body("92% and climbing")
//!             .dedupe_key("disk-s10")
//!             .retry(Duration::from_secs(60))
//!             .expire(Duration::from_secs(3600)),
//!     )
//!     .await?;
//!
//! let status = channel
//!     .wait(sent.id.as_deref().unwrap(), Some(Duration::from_secs(300)))
//!     .await?;
//! if status.acknowledged() {
//!     println!("{:?} answered", status.acked_by);
//! }
//! # Ok(())
//! # }
//! ```

mod channel;
mod error;
mod message;
mod poller;
mod status;
mod transport;
mod url;

pub use channel::Channel;
pub use error::Error;
pub use message::{Message, Priority, Sound};
pub use poller::Poller;
pub use status::{Sent, State, Status};
pub use transport::{BoxFuture, Method, Request, Response, Transport};

#[cfg(feature = "reqwest")]
pub use transport::ReqwestTransport;

/// Requests a minute Mast allows on one key, shared between sends and polls.
pub const RATE_LIMIT_PER_MINUTE: usize = 60;

/// How many of those a poller will spend. The rest is left for sending: a
/// client that polls itself out of a budget cannot report the next outage.
pub const POLL_BUDGET_PER_MINUTE: usize = 30;

/// Where a bare key is assumed to live.
pub const DEFAULT_HOST: &str = "https://mast.tissue.dev";
