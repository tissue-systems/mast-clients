#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mast::{BoxFuture, Channel, Error, Method, Request, Response, Transport};

pub const KEY: &str = "mk_0123456789abcdef0123456789abcdef01234567";
pub const URL: &str = "https://mast.tissue.dev/mk_0123456789abcdef0123456789abcdef01234567";

/// A message id made of one repeated hex digit, so tests can tell pages apart
/// at a glance.
pub fn message_id(tag: char) -> String {
    format!("mm_{}", tag.to_string().repeat(32))
}

/// A transport that answers from a queue and remembers what it was asked.
///
/// The last reply queued repeats, which is what a poller wants: queue the
/// interesting answers, and the steady state after them.
pub struct Stub {
    replies: Mutex<VecDeque<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Stub {
    pub fn new() -> Arc<Stub> {
        Arc::new(Stub {
            replies: Mutex::new(VecDeque::new()),
            seen: Mutex::new(Vec::new()),
        })
    }

    /// A stub that answers everything with one reply.
    pub fn answering(status: u16, body: &str) -> Arc<Stub> {
        let stub = Stub::new();
        stub.queue(json(status, body));
        stub
    }

    pub fn queue(&self, reply: Response) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    pub fn last(&self) -> Request {
        self.seen
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("nothing was sent")
    }

    pub fn last_body(&self) -> String {
        self.last().body.unwrap_or_default()
    }

    pub fn urls(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.url).collect()
    }
}

impl Transport for Stub {
    fn execute(&self, request: Request) -> BoxFuture<'_, Result<Response, Error>> {
        let reply = {
            self.seen.lock().unwrap().push(request);
            let mut replies = self.replies.lock().unwrap();
            if replies.len() > 1 {
                replies.pop_front()
            } else {
                replies.front().cloned()
            }
        };
        Box::pin(async move {
            reply.ok_or_else(|| Error::Unreachable("the stub had no reply queued".into()))
        })
    }
}

pub fn json(status: u16, body: &str) -> Response {
    Response {
        status,
        retry_after: None,
        body: body.as_bytes().to_vec(),
    }
}

pub fn rate_limited(retry_after: u64) -> Response {
    Response {
        status: 429,
        retry_after: Some(Duration::from_secs(retry_after)),
        body: br#"{"error":{"code":"rate_limited","message":"Too many requests."}}"#.to_vec(),
    }
}

pub fn error_body(message: &str) -> String {
    format!(r#"{{"error":{{"code":"bad_request","message":"{message}"}}}}"#)
}

pub fn open(raw: &str) -> Result<Channel, Error> {
    Channel::with_transport(raw, Stub::new())
}

pub fn channel(stub: &Arc<Stub>) -> Channel {
    Channel::with_transport(URL, stub.clone()).expect("the test URL parses")
}

pub fn is_post(request: &Request) -> bool {
    request.method == Method::Post
}

/// A clock a test drives by hand, so the poll ladder can be walked in a
/// millisecond instead of ten minutes.
#[derive(Clone)]
pub struct Clock(Arc<Mutex<f64>>);

impl Clock {
    pub fn new() -> Clock {
        Clock(Arc::new(Mutex::new(0.0)))
    }

    pub fn now(&self) -> f64 {
        *self.0.lock().unwrap()
    }

    pub fn advance(&self, seconds: f64) {
        *self.0.lock().unwrap() += seconds;
    }

    pub fn reader(&self) -> impl Fn() -> f64 + Send + Sync + 'static {
        let held = self.0.clone();
        move || *held.lock().unwrap()
    }

    /// A sleeper that moves the clock rather than the wall.
    pub fn sleeper(&self) -> impl Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync + 'static {
        let held = self.0.clone();
        move |wait| {
            *held.lock().unwrap() += wait.as_secs_f64();
            Box::pin(async {})
        }
    }
}
