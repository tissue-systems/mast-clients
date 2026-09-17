mod support;

use std::sync::Arc;
use std::time::Duration;

use mast::{Channel, Error, Poller};
use support::{channel, error_body, message_id, rate_limited, Clock, Stub};

const QUEUED: &str = r#"{"id":"mm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","state":"queued"}"#;
const ACKED: &str =
    r#"{"id":"mm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","state":"acked","acked_by":"iPhone"}"#;

fn watching<'a>(channel: &'a Channel, clock: &Clock) -> Poller<'a> {
    Poller::new(channel)
        .with_clock(clock.reader())
        .with_sleeper(clock.sleeper())
}

#[tokio::test]
async fn asks_often_while_somebody_may_still_be_looking() {
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    poller.watch(&message_id('a')).unwrap();

    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 1, "the first ask is immediate");

    // Under half a minute old: every three seconds.
    clock.advance(1.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 1);
    clock.advance(2.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 2);

    // Past thirty seconds: every ten.
    clock.advance(37.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 3);
    clock.advance(5.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 3);
    clock.advance(5.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 4);

    // Past five minutes, when nobody is coming: every thirty.
    clock.advance(260.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 5);
    clock.advance(10.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 5);
    clock.advance(20.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 6);
}

#[tokio::test]
async fn stops_asking_once_the_page_is_answered() {
    let stub = Stub::new();
    stub.queue(support::json(200, QUEUED));
    stub.queue(support::json(200, ACKED));

    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    let id = message_id('a');
    poller.watch(&id).unwrap();

    assert!(poller.tick().await.unwrap().is_empty());
    assert_eq!(poller.open_count(), 1);

    clock.advance(3.0);
    let settled = poller.tick().await.unwrap();
    assert!(settled[&id].acknowledged());
    assert_eq!(settled[&id].acked_by.as_deref(), Some("iPhone"));
    assert_eq!(poller.open_count(), 0);

    clock.advance(60.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 2, "a settled page is never asked about again");
}

#[tokio::test]
async fn a_failed_poll_is_not_an_answer() {
    let stub = Stub::new();
    stub.queue(support::json(500, ""));
    stub.queue(support::json(200, ACKED));

    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    let id = message_id('a');
    poller.watch(&id).unwrap();

    assert!(poller.tick().await.unwrap().is_empty());
    assert_eq!(poller.open_count(), 1, "the page survives a bad poll");

    clock.advance(3.0);
    assert!(poller.tick().await.unwrap()[&id].acknowledged());
}

#[tokio::test]
async fn gives_up_on_a_message_mast_will_not_talk_about() {
    let stub = Stub::answering(404, &error_body("No such message."));
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    let id = message_id('a');
    poller.watch(&id).unwrap();

    poller.tick().await.unwrap();
    assert_eq!(poller.open_count(), 0);
    assert!(matches!(
        poller.take_error(&id),
        Some(Error::UnknownMessage(_))
    ));
    assert!(poller.take_error(&id).is_none(), "taking it clears it");

    clock.advance(30.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 1);
}

#[tokio::test]
async fn asks_about_the_oldest_page_first_when_the_budget_is_short() {
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock).with_budget(2);

    for tag in ['a', 'b', 'c'] {
        poller.watch(&message_id(tag)).unwrap();
        clock.advance(1.0);
    }

    clock.advance(10.0);
    poller.tick().await.unwrap();

    let asked = stub.urls().join(" ");
    assert_eq!(stub.count(), 2);
    assert!(asked.contains(&message_id('a')), "{asked}");
    assert!(asked.contains(&message_id('b')), "{asked}");
    assert!(!asked.contains(&message_id('c')), "{asked}");
}

#[tokio::test]
async fn the_budget_is_a_sliding_window() {
    // A token bucket would hand back its whole depth the moment it refilled,
    // which is how a limiter set at half the channel's allowance ends up
    // spending nearly all of it in one minute.
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock).with_budget(2);
    poller.watch(&message_id('a')).unwrap();

    poller.tick().await.unwrap();
    clock.advance(3.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 2);

    clock.advance(3.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 2, "the window is full");

    clock.advance(55.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 3, "only the oldest spend has aged out");
}

#[tokio::test]
async fn a_rate_limit_backs_off_every_page_on_the_channel() {
    let stub = Stub::new();
    stub.queue(rate_limited(30));
    stub.queue(support::json(200, QUEUED));

    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    poller.watch(&message_id('a')).unwrap();
    clock.advance(1.0);
    poller.watch(&message_id('b')).unwrap();

    poller.tick().await.unwrap();
    assert_eq!(
        stub.count(),
        1,
        "the limit belongs to the key, not the page"
    );
    assert_eq!(poller.open_count(), 2);

    clock.advance(10.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 1, "still waiting out the retry-after");

    clock.advance(25.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 3);
}

#[tokio::test]
async fn watching_one_page_twice_watches_one_page() {
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    let id = message_id('a');

    poller.watch(&id).unwrap();
    poller.tick().await.unwrap();

    clock.advance(1.0);
    poller.watch(&id).unwrap();
    assert_eq!(poller.open_count(), 1);

    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 1, "re-watching does not restart the ladder");
}

#[tokio::test]
async fn refuses_to_watch_something_that_is_not_a_message() {
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);

    assert!(matches!(poller.watch("mm_nope"), Err(Error::Rejected(_))));
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn an_idle_poller_asks_nothing() {
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);

    poller.tick().await.unwrap();
    clock.advance(600.0);
    poller.tick().await.unwrap();
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn run_returns_when_the_last_page_settles() {
    let stub = Stub::new();
    stub.queue(support::json(200, QUEUED));
    stub.queue(support::json(200, ACKED));

    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    let id = message_id('a');
    poller.watch(&id).unwrap();

    let settled = poller.run(None).await.unwrap();
    assert!(settled[&id].acknowledged());
    assert_eq!(poller.open_count(), 0);
    assert!(clock.now() >= 3.0, "it waited out the first rung");
}

#[tokio::test]
async fn a_timeout_leaves_the_page_watched() {
    let stub = Stub::answering(200, QUEUED);
    let channel = channel(&stub);
    let clock = Clock::new();
    let mut poller = watching(&channel, &clock);
    let id = message_id('a');
    poller.watch(&id).unwrap();

    let settled = poller.run(Some(Duration::from_secs(5))).await.unwrap();
    assert!(settled.is_empty());
    assert_eq!(poller.open_count(), 1);
    assert!(poller.last(&id).is_some(), "the last read is kept");

    let before = stub.count();
    poller.run(Some(Duration::from_secs(5))).await.unwrap();
    assert!(
        stub.count() > before,
        "and the watch picks up where it left off"
    );
}

#[tokio::test]
async fn wait_hands_back_the_acknowledgement() {
    let stub = Stub::answering(200, ACKED);
    let status = channel(&stub)
        .wait(&message_id('a'), Some(Duration::from_secs(1)))
        .await
        .unwrap();
    assert!(status.acknowledged());
}

#[tokio::test]
async fn wait_hands_back_the_last_status_when_the_time_runs_out() {
    let stub = Stub::answering(200, QUEUED);
    let status = channel(&stub)
        .wait(&message_id('a'), Some(Duration::ZERO))
        .await
        .unwrap();
    assert!(!status.acknowledged());
    assert!(!status.done(), "the page is still open on the phone");
}

#[tokio::test]
async fn wait_reports_a_message_that_does_not_exist() {
    let stub = Stub::answering(404, &error_body("No such message."));
    let err = channel(&stub)
        .wait(&message_id('a'), Some(Duration::ZERO))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::UnknownMessage(_)), "{err}");
}

#[tokio::test]
async fn wait_refuses_an_id_it_could_never_read() {
    let stub = Stub::answering(200, ACKED);
    let err = channel(&stub).wait("mm_nope", None).await.unwrap_err();
    assert!(matches!(err, Error::Rejected(_)), "{err}");
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn wait_says_so_when_it_never_got_an_answer() {
    let stub: Arc<Stub> = Stub::new();
    let err = channel(&stub)
        .wait(&message_id('a'), Some(Duration::ZERO))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Unreachable(_)), "{err}");
}
