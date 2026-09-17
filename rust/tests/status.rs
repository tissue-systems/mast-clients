mod support;

use std::time::Duration;

use mast::{Error, State};
use support::{channel, error_body, message_id, rate_limited, Stub, KEY};

const ACKED: &str = r#"{
  "id": "mm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "state": "acked",
  "received_at": "2026-09-17T10:00:00Z",
  "acked_at": "2026-09-17T10:06:52Z",
  "acked_by": "Denis's iPhone",
  "dedupe_count": 3
}"#;

#[tokio::test]
async fn reads_one_message() {
    let stub = Stub::answering(200, ACKED);
    let id = message_id('a');
    let status = channel(&stub).status(&id).await.unwrap();

    assert_eq!(
        stub.last().url,
        format!("https://mast.tissue.dev/{KEY}/messages/{id}")
    );
    assert!(stub.last().body.is_none(), "reading a message is a GET");
    assert_eq!(status.state, State::Acked);
    assert_eq!(status.acked_by.as_deref(), Some("Denis's iPhone"));
    assert_eq!(status.dedupe_count, 3);
    assert!(status.acknowledged());
    assert!(status.done());
}

#[tokio::test]
async fn works_out_how_long_the_page_stood_open() {
    let stub = Stub::answering(200, ACKED);
    let status = channel(&stub).status(&message_id('a')).await.unwrap();
    assert_eq!(status.open_for(), Some(Duration::from_secs(412)));
}

#[tokio::test]
async fn measures_a_page_that_ran_over_midnight() {
    let stub = Stub::answering(
        200,
        r#"{"state":"acked","received_at":"2026-02-28T23:58:00Z","acked_at":"2026-03-01T00:02:00Z"}"#,
    );
    let status = channel(&stub).status(&message_id('a')).await.unwrap();
    assert_eq!(status.open_for(), Some(Duration::from_secs(4 * 60)));
}

#[tokio::test]
async fn falls_back_to_the_resolve_stamp() {
    let stub = Stub::answering(
        200,
        r#"{"state":"resolved","received_at":"2026-09-17T10:00:00Z","resolved_at":"2026-09-17T10:01:00.500Z"}"#,
    );
    let status = channel(&stub).status(&message_id('a')).await.unwrap();
    assert_eq!(status.open_for(), Some(Duration::from_secs(60)));
    assert!(status.done());
    assert!(
        !status.acknowledged(),
        "a resolve is not an acknowledgement"
    );
}

#[tokio::test]
async fn a_duration_it_cannot_work_out_is_unknown_not_zero() {
    for body in [
        r#"{"state":"acked","acked_at":"2026-09-17T10:06:52Z"}"#,
        r#"{"state":"acked","received_at":"2026-09-17T10:00:00Z"}"#,
        r#"{"state":"acked","received_at":"2026-09-17T10:00:00Z","acked_at":""}"#,
        r#"{"state":"acked","received_at":"2026-09-17T10:00:00+02:00","acked_at":"2026-09-17T10:06:52+02:00"}"#,
        r#"{"state":"acked","received_at":"yesterday","acked_at":"2026-09-17T10:06:52Z"}"#,
    ] {
        let stub = Stub::answering(200, body);
        let status = channel(&stub).status(&message_id('a')).await.unwrap();
        assert_eq!(status.open_for(), None, "{body}");
    }
}

#[tokio::test]
async fn an_open_page_is_not_done() {
    for (state, expected) in [
        ("queued", State::Queued),
        ("muted", State::Muted),
        ("deduped", State::Deduped),
        ("suppressed", State::Suppressed),
    ] {
        let stub = Stub::answering(200, &format!(r#"{{"state":"{state}"}}"#));
        let status = channel(&stub).status(&message_id('a')).await.unwrap();
        assert_eq!(status.state, expected);
        assert!(!status.done(), "{state}");
    }
}

#[tokio::test]
async fn expired_is_settled_but_unanswered() {
    let stub = Stub::answering(200, r#"{"state":"expired"}"#);
    let status = channel(&stub).status(&message_id('a')).await.unwrap();
    assert!(status.done());
    assert!(!status.acknowledged());
}

#[tokio::test]
async fn carries_a_state_it_has_never_heard_of() {
    let stub = Stub::answering(200, r#"{"state":"snoozed"}"#);
    let status = channel(&stub).status(&message_id('a')).await.unwrap();
    assert_eq!(status.state, State::Other("snoozed".into()));
    assert!(!status.done(), "an unknown state is not treated as settled");
}

#[tokio::test]
async fn refuses_an_id_that_cannot_exist_without_asking() {
    let stub = Stub::answering(200, ACKED);
    let channel = channel(&stub);

    for id in ["", "mm_short", "nope", &message_id('a')[..20]] {
        let err = channel.status(id).await.unwrap_err();
        assert!(matches!(err, Error::Rejected(_)), "{id} gave {err}");
    }
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn separates_an_unknown_message_from_an_unknown_channel() {
    let stub = Stub::answering(404, &error_body("No such message."));
    let err = channel(&stub).status(&message_id('a')).await.unwrap_err();
    assert!(matches!(err, Error::UnknownMessage(_)), "{err}");

    let stub = Stub::answering(404, &error_body("No such channel."));
    let err = channel(&stub).status(&message_id('a')).await.unwrap_err();
    assert!(matches!(err, Error::UnknownChannel(_)), "{err}");
}

#[tokio::test]
async fn a_404_with_no_body_is_read_as_the_channel() {
    // Both 404s look alike from the status line alone, and treating an
    // unreadable one as a bad message would report a dead key as healthy.
    let stub = Stub::answering(404, "");
    let err = channel(&stub).status(&message_id('a')).await.unwrap_err();
    assert!(matches!(err, Error::UnknownChannel(_)), "{err}");
}

#[tokio::test]
async fn checking_a_key_stores_nothing() {
    let stub = Stub::answering(404, &error_body("No such message."));
    channel(&stub).check().await.unwrap();

    let request = stub.last();
    assert_eq!(
        request.url,
        format!("https://mast.tissue.dev/{KEY}/messages/check")
    );
    assert!(request.body.is_none(), "a check must not send anything");
    assert_eq!(stub.count(), 1);
}

#[tokio::test]
async fn checking_a_bad_key_says_so() {
    let stub = Stub::answering(404, &error_body("No such channel."));
    let err = channel(&stub).check().await.unwrap_err();
    assert!(matches!(err, Error::UnknownChannel(_)), "{err}");
}

#[tokio::test]
async fn a_check_that_is_answered_is_not_taken_as_a_pass() {
    let stub = Stub::answering(200, r#"{"state":"queued"}"#);
    let err = channel(&stub).check().await.unwrap_err();
    assert!(matches!(err, Error::Unreachable(_)), "{err}");
}

#[tokio::test]
async fn a_check_does_not_turn_a_rate_limit_into_a_bad_key() {
    let stub = Stub::new();
    stub.queue(rate_limited(30));
    let err = channel(&stub).check().await.unwrap_err();
    assert!(matches!(err, Error::RateLimited { .. }), "{err}");
}
