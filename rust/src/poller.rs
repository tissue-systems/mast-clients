use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::transport::BoxFuture;
use crate::{Channel, Error, Status, POLL_BUDGET_PER_MINUTE};

/// Age of an open page against how often to ask about it. The first rung the
/// page has not aged past wins: fast while somebody may still be looking at it,
/// slow once it is clear nobody is.
const POLL_LADDER: [(f64, f64); 3] = [(30.0, 3.0), (300.0, 10.0), (f64::INFINITY, 30.0)];

/// How long [`run`](Poller::run) waits between looks at the ladder.
#[cfg(feature = "tokio")]
const TICK: Duration = Duration::from_secs(1);

type Clock = Box<dyn Fn() -> f64 + Send + Sync>;
type Sleeper = Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

struct Watched {
    started: f64,
    last_poll: f64,
}

impl Watched {
    fn due(&self, at: f64) -> bool {
        let age = at - self.started;
        for (until, every) in POLL_LADDER {
            if age < until {
                return at - self.last_poll >= every;
            }
        }
        false
    }
}

/// Watches open pages on one channel until they settle.
///
/// Several at once share the channel's request budget, so the oldest is asked
/// about first when there is not enough to go round: it is the one somebody is
/// still standing over. A 429 backs off the whole channel rather than the one
/// page, because the limit belongs to the key.
pub struct Poller<'a> {
    channel: &'a Channel,
    budget: usize,
    open: HashMap<String, Watched>,
    last: HashMap<String, Status>,
    dropped: HashMap<String, Error>,
    spent: VecDeque<f64>,
    paused_until: f64,
    clock: Clock,
    sleeper: Option<Sleeper>,
}

impl<'a> Poller<'a> {
    pub fn new(channel: &'a Channel) -> Poller<'a> {
        let started = Instant::now();
        Poller {
            channel,
            budget: POLL_BUDGET_PER_MINUTE,
            open: HashMap::new(),
            last: HashMap::new(),
            dropped: HashMap::new(),
            spent: VecDeque::new(),
            paused_until: 0.0,
            clock: Box::new(move || started.elapsed().as_secs_f64()),
            sleeper: None,
        }
    }

    /// How many requests a minute polling may spend. The rest of the channel's
    /// allowance is left for sending.
    pub fn with_budget(mut self, per_minute: usize) -> Self {
        self.budget = per_minute;
        self
    }

    /// Monotonic seconds from any source, for a caller that already keeps time
    /// or a test that would rather not wait.
    pub fn with_clock(mut self, clock: impl Fn() -> f64 + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// What [`run`](Poller::run) waits with between ticks.
    pub fn with_sleeper(
        mut self,
        sleeper: impl Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync + 'static,
    ) -> Self {
        self.sleeper = Some(Box::new(sleeper));
        self
    }

    /// Start following a message. Watching one twice watches one page.
    pub fn watch(&mut self, message_id: &str) -> Result<(), Error> {
        if !crate::url::is_message_id(message_id) {
            return Err(Error::Rejected(format!(
                "{message_id:?} is not a message id"
            )));
        }
        let at = (self.clock)();
        self.open.entry(message_id.to_string()).or_insert(Watched {
            started: at,
            last_poll: f64::NEG_INFINITY,
        });
        Ok(())
    }

    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// The most recent status read for a message.
    pub fn last(&self, message_id: &str) -> Option<&Status> {
        self.last.get(message_id)
    }

    /// Why a message stopped being watched without settling. Taking it clears
    /// it, so a caller that watches the same id again starts clean.
    pub fn take_error(&mut self, message_id: &str) -> Option<Error> {
        self.dropped.remove(message_id)
    }

    /// Ask about whatever is due and affordable, and report what settled.
    pub async fn tick(&mut self) -> Result<HashMap<String, Status>, Error> {
        let mut settled = HashMap::new();
        let at = (self.clock)();
        if at < self.paused_until {
            return Ok(settled);
        }

        let mut due: Vec<(String, f64)> = self
            .open
            .iter()
            .filter(|(_, page)| page.due(at))
            .map(|(id, page)| (id.clone(), page.started))
            .collect();
        due.sort_by(|a, b| a.1.total_cmp(&b.1));

        for (message_id, _) in due {
            if !self.take(at) {
                break;
            }
            if let Some(page) = self.open.get_mut(&message_id) {
                page.last_poll = at;
            }

            match self.channel.status(&message_id).await {
                Ok(status) => {
                    let done = status.done();
                    self.last.insert(message_id.clone(), status.clone());
                    if done {
                        self.open.remove(&message_id);
                        settled.insert(message_id, status);
                    }
                }
                Err(Error::RateLimited { retry_after }) => {
                    self.paused_until = (self.clock)() + retry_after.as_secs_f64();
                    break;
                }
                Err(err @ (Error::UnknownMessage(_) | Error::UnknownChannel(_))) => {
                    // Nothing more is going to happen to a message the channel
                    // will not talk about.
                    self.open.remove(&message_id);
                    self.dropped.insert(message_id, err);
                }
                Err(_) => {
                    // A blip is not an answer: leave the page open for the next
                    // tick.
                }
            }
        }
        Ok(settled)
    }

    /// Tick until nothing is open or the timeout runs out. A timeout is not a
    /// failed page: anything still open stays watched, so calling this again
    /// picks up where it left off.
    #[cfg(feature = "tokio")]
    pub async fn run(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<HashMap<String, Status>, Error> {
        let deadline = timeout.map(|limit| (self.clock)() + limit.as_secs_f64());
        let mut settled = HashMap::new();

        while !self.open.is_empty() {
            settled.extend(self.tick().await?);
            if self.open.is_empty() {
                break;
            }
            if deadline.is_some_and(|deadline| (self.clock)() >= deadline) {
                break;
            }
            match &self.sleeper {
                Some(sleeper) => sleeper(TICK).await,
                None => tokio::time::sleep(TICK).await,
            }
        }
        Ok(settled)
    }

    /// Spend one request if the last sixty seconds leave room for it.
    ///
    /// A token bucket would be the usual answer, but a full one lets through
    /// its own depth on top of the refill, which is how a limiter sized at half
    /// the channel's allowance ends up spending nearly all of it.
    fn take(&mut self, at: f64) -> bool {
        while self.spent.front().is_some_and(|spent| at - spent >= 60.0) {
            self.spent.pop_front();
        }
        if self.spent.len() >= self.budget {
            return false;
        }
        self.spent.push_back(at);
        true
    }
}
