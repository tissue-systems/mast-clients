use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::Error;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// One request to Mast. The body, when there is one, is already
/// form-encoded.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: Method,
    pub url: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub retry_after: Option<Duration>,
    pub body: Vec<u8>,
}

/// How a [`Channel`](crate::Channel) reaches Mast.
///
/// Rust has no HTTP client in the standard library, so this crate ships one and
/// lets you supply your own instead: an agent that already holds a client, or a
/// test that would rather not open a socket, implements this and keeps
/// everything else.
///
/// An implementation returns [`Error::Unreachable`] for a connection that
/// failed or timed out, and the response otherwise, whatever its status.
pub trait Transport: Send + Sync {
    fn execute(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>>;
}

#[cfg(feature = "reqwest")]
pub use bundled::ReqwestTransport;

#[cfg(feature = "reqwest")]
mod bundled {
    use super::{BoxFuture, Method, Request, Response, Transport};
    use crate::Error;
    use std::time::Duration;

    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

    /// The bundled transport.
    pub struct ReqwestTransport {
        client: reqwest::Client,
    }

    impl ReqwestTransport {
        pub fn new() -> Result<Self, Error> {
            let client = reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                // The key is in the URL, so following a redirect would hand it
                // to whoever answered.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|err| Error::Unreachable(err.to_string()))?;
            Ok(Self { client })
        }

        /// Use a client you have already built, with your own timeouts, proxy
        /// or connection pool. Leave redirects off.
        pub fn with_client(client: reqwest::Client) -> Self {
            Self { client }
        }
    }

    impl Transport for ReqwestTransport {
        fn execute(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>> {
            Box::pin(async move {
                let mut builder = match request.method {
                    Method::Get => self.client.get(&request.url),
                    Method::Post => self.client.post(&request.url),
                };
                if let Some(body) = request.body {
                    builder = builder
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body(body);
                }

                let response = builder
                    .send()
                    .await
                    .map_err(|err| Error::Unreachable(err.to_string()))?;

                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .map(Duration::from_secs);
                let body = response
                    .bytes()
                    .await
                    .map_err(|err| Error::Unreachable(err.to_string()))?
                    .to_vec();

                Ok(Response {
                    status,
                    retry_after,
                    body,
                })
            })
        }
    }
}
