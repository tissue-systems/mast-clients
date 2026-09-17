mod support;

use std::time::Duration;

use mast::{Channel, Error, Message, Priority, Sound, State};
use support::{channel, error_body, message_id, rate_limited, Stub, KEY};

const ACCEPTED: &str = r#"{"id":"mm_0123456789abcdef0123456789abcdef","state":"queued"}"#;

async fn sent_form(message: Message) -> String {
    let stub = Stub::answering(202, ACCEPTED);
    channel(&stub).send(message).await.expect("accepted");
    stub.last_body()
}

fn refusal(result: Result<mast::Sent, Error>) -> String {
    match result {
        Err(Error::Rejected(reason)) => reason,
        Err(other) => panic!("expected a refusal, got {other}"),
        Ok(_) => panic!("expected a refusal, the send went through"),
    }
}

#[tokio::test]
async fn posts_the_message_to_the_channel_url() {
    let stub = Stub::answering(202, ACCEPTED);
    let sent = channel(&stub)
        .send(Message::new().title("s10 disk").body("92% and climbing"))
        .await
        .unwrap();

    let request = stub.last();
    assert!(support::is_post(&request));
    assert_eq!(request.url, format!("https://mast.tissue.dev/{KEY}"));
    assert_eq!(
        request.body.unwrap(),
        "title=s10+disk&body=92%25+and+climbing"
    );
    assert_eq!(
        sent.id.as_deref(),
        Some("mm_0123456789abcdef0123456789abcdef")
    );
    assert_eq!(sent.state, State::Queued);
    assert!(!sent.duplicate);
}

#[tokio::test]
async fn encodes_what_a_log_line_is_full_of() {
    let form = sent_form(Message::new().title("db=1 & rows>0").body("100% / 90%")).await;
    assert_eq!(form, "title=db%3D1+%26+rows%3E0&body=100%25+%2F+90%25");
}

#[tokio::test]
async fn sends_utf8_as_bytes() {
    let form = sent_form(Message::new().title("café")).await;
    assert_eq!(form, "title=caf%C3%A9");
}

#[tokio::test]
async fn names_the_tier_and_the_tone() {
    let form = sent_form(Message::new().priority(Priority::Loud).sound(Sound::Klaxon)).await;
    assert_eq!(form, "priority=loud&sound=klaxon");
}

#[tokio::test]
async fn page_sends_at_the_pager_tier() {
    let stub = Stub::answering(202, ACCEPTED);
    channel(&stub)
        .page(Message::new().title("s10 down"))
        .await
        .unwrap();
    assert!(stub.last_body().contains("priority=page"));
}

#[tokio::test]
async fn a_tier_set_by_hand_survives_send() {
    let form = sent_form(Message::new().priority(Priority::Quiet)).await;
    assert!(form.contains("priority=quiet"));
}

#[tokio::test]
async fn carries_the_dedupe_key_as_key() {
    let form = sent_form(Message::new().dedupe_key("disk-s10")).await;
    assert_eq!(form, "key=disk-s10");
}

#[tokio::test]
async fn writes_retry_and_expire_in_whole_seconds() {
    let form = sent_form(
        Message::new()
            .retry(Duration::from_secs(60))
            .expire(Duration::from_secs(3600)),
    )
    .await;
    assert_eq!(form, "retry=60&expire=3600");
}

#[tokio::test]
async fn turning_retry_off_is_a_zero_not_a_missing_field() {
    let form = sent_form(Message::new().no_retry().no_expire()).await;
    assert_eq!(form, "retry=0&expire=0");
}

#[tokio::test]
async fn refuses_a_retry_outside_what_mast_accepts() {
    let stub = Stub::answering(202, ACCEPTED);
    let channel = channel(&stub);

    for message in [
        Message::new().retry(Duration::from_secs(29)),
        Message::new().retry(Duration::from_secs(86_401)),
        Message::new().expire(Duration::from_secs(59)),
        Message::new().expire(Duration::from_secs(604_801)),
    ] {
        refusal(channel.send(message).await);
    }
    assert_eq!(stub.count(), 0, "a refused send must not cost a request");
}

#[tokio::test]
async fn refuses_a_retry_that_is_not_whole_seconds() {
    let stub = Stub::answering(202, ACCEPTED);
    let reason = refusal(
        channel(&stub)
            .send(Message::new().retry(Duration::from_millis(60_500)))
            .await,
    );
    assert!(reason.contains("whole number of seconds"), "{reason}");
}

#[tokio::test]
async fn ack_is_spelled_required() {
    let form = sent_form(Message::new().ack()).await;
    assert_eq!(form, "ack=required");
}

#[tokio::test]
async fn silent_cannot_be_asked_for_alongside_a_noise() {
    let stub = Stub::answering(202, ACCEPTED);
    let channel = channel(&stub);

    let reason = refusal(channel.send(Message::new().silent().ack()).await);
    assert!(reason.contains("silent"), "{reason}");
    refusal(
        channel
            .send(Message::new().silent().priority(Priority::Page))
            .await,
    );
    refusal(channel.page(Message::new().silent()).await);
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn silent_on_its_own_is_fine() {
    let form = sent_form(Message::new().silent()).await;
    assert_eq!(form, "silent=1");
}

#[tokio::test]
async fn checks_the_scheme_on_a_link_and_a_callback() {
    let stub = Stub::answering(202, ACCEPTED);
    let channel = channel(&stub);

    refusal(channel.send(Message::new().url("mast.tissue.dev")).await);
    refusal(
        channel
            .send(Message::new().callback("http://alerts.example/ack"))
            .await,
    );
    assert_eq!(stub.count(), 0);

    let form = sent_form(
        Message::new()
            .url("https://grafana.example/d/s10")
            .url_title("Dashboard")
            .callback("https://alerts.example/ack"),
    )
    .await;
    assert!(
        form.contains("url=https%3A%2F%2Fgrafana.example%2Fd%2Fs10"),
        "{form}"
    );
    assert!(form.contains("url_title=Dashboard"), "{form}");
    assert!(
        form.contains("callback=https%3A%2F%2Falerts.example%2Fack"),
        "{form}"
    );
}

#[tokio::test]
async fn measures_the_caps_in_characters() {
    let stub = Stub::answering(202, ACCEPTED);
    let channel = channel(&stub);

    // 250 accented characters are 500 bytes, and the server counts characters.
    channel
        .send(Message::new().title("é".repeat(250)))
        .await
        .unwrap();

    let reason = refusal(channel.send(Message::new().title("x".repeat(251))).await);
    assert!(reason.contains("250"), "{reason}");
    refusal(channel.send(Message::new().body("x".repeat(4097))).await);
    refusal(
        channel
            .send(Message::new().dedupe_key("x".repeat(121)))
            .await,
    );
    refusal(
        channel
            .send(Message::new().url(format!("https://e.example/{}", "x".repeat(512))))
            .await,
    );

    assert_eq!(
        stub.count(),
        1,
        "only the accepted send should have gone out"
    );
}

#[tokio::test]
async fn a_heartbeat_carries_nothing() {
    let stub = Stub::answering(202, r#"{"state":"alive"}"#);
    let sent = channel(&stub).ping(Message::new()).await.unwrap();

    assert_eq!(stub.last_body(), "");
    assert_eq!(sent.state, State::Alive);
    assert!(sent.id.is_none(), "a heartbeat stores no card to follow");
}

#[tokio::test]
async fn flatlining_a_vital_has_its_own_route() {
    let stub = Stub::answering(202, ACCEPTED);
    channel(&stub)
        .fail(Message::new().title("no heartbeat"))
        .await
        .unwrap();
    assert_eq!(
        stub.last().url,
        format!("https://mast.tissue.dev/{KEY}/fail")
    );
}

#[tokio::test]
async fn resolving_by_key_closes_that_card() {
    let stub = Stub::answering(200, r#"{"state":"resolved","open_for":412}"#);
    let sent = channel(&stub)
        .resolve_key("disk-s10", Message::new().title("recovered"))
        .await
        .unwrap();

    let form = stub.last_body();
    assert!(form.contains("key=disk-s10"), "{form}");
    assert!(form.contains("resolve=1"), "{form}");
    assert_eq!(sent.state, State::Resolved);
    assert_eq!(sent.open_for, Some(Duration::from_secs(412)));
}

#[tokio::test]
async fn a_resolve_needs_something_to_resolve() {
    let stub = Stub::answering(200, ACCEPTED);
    let channel = channel(&stub);

    refusal(channel.resolve_key("", Message::new()).await);
    refusal(channel.resolve_message("mm_nope", Message::new()).await);
    assert_eq!(stub.count(), 0);
}

#[tokio::test]
async fn resolving_by_id_names_the_message() {
    let stub = Stub::answering(200, r#"{"state":"resolved"}"#);
    let id = message_id('a');
    channel(&stub)
        .resolve_message(&id, Message::new())
        .await
        .unwrap();
    assert_eq!(stub.last_body(), format!("resolve={id}"));
}

#[tokio::test]
async fn reports_a_dedupe_fold() {
    let stub = Stub::answering(
        200,
        r#"{"id":"mm_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","state":"deduped","duplicate":true}"#,
    );
    let sent = channel(&stub)
        .send(Message::new().dedupe_key("k"))
        .await
        .unwrap();
    assert!(sent.duplicate);
    assert_eq!(sent.state, State::Deduped);
}

#[tokio::test]
async fn reads_the_reason_out_of_the_nested_error() {
    let stub = Stub::answering(400, &error_body("title is too long."));
    let err = channel(&stub)
        .send(Message::new().title("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Rejected(_)), "{err}");
    assert!(err.to_string().contains("title is too long."), "{err}");
}

#[tokio::test]
async fn a_body_over_the_wire_limit_says_so() {
    let stub = Stub::answering(413, "");
    let err = channel(&stub)
        .send(Message::new().body("x"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("16 KiB"), "{err}");
}

#[tokio::test]
async fn a_rate_limit_carries_how_long_to_wait() {
    let stub = Stub::new();
    stub.queue(rate_limited(12));
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    match err {
        Error::RateLimited { retry_after } => assert_eq!(retry_after, Duration::from_secs(12)),
        other => panic!("expected a rate limit, got {other}"),
    }
}

#[tokio::test]
async fn a_rate_limit_without_a_header_still_waits() {
    let stub = Stub::answering(429, "");
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    match err {
        Error::RateLimited { retry_after } => assert!(retry_after >= Duration::from_secs(1)),
        other => panic!("expected a rate limit, got {other}"),
    }
}

#[tokio::test]
async fn a_bad_key_is_not_a_refused_send() {
    let stub = Stub::answering(404, &error_body("No such channel."));
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    assert!(matches!(err, Error::UnknownChannel(_)), "{err}");
}

#[tokio::test]
async fn a_5xx_is_reported_as_unavailable_rather_than_a_rejection() {
    // Whether the message was stored is unknowable from here, and a caller that
    // reads this as a rejection and retries can page somebody twice.
    let stub = Stub::answering(503, &error_body("upstream unavailable"));
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    match err {
        Error::Unavailable { status, ref reason } => {
            assert_eq!(status, 503);
            assert_eq!(reason, "upstream unavailable");
        }
        other => panic!("expected unavailable, got {other}"),
    }
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let stub = Stub::answering(302, "");
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    assert!(matches!(err, Error::Unreachable(_)), "{err}");
    assert!(err.to_string().contains("redirect"), "{err}");
}

#[tokio::test]
async fn survives_an_answer_that_is_not_json() {
    let stub = Stub::answering(502, "<html>502 Bad Gateway</html>");
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    assert!(
        matches!(err, Error::Unavailable { status: 502, .. }),
        "{err}"
    );
}

#[tokio::test]
async fn passes_a_transport_failure_through_untouched() {
    let stub = Stub::new();
    let err = channel(&stub).send(Message::new()).await.unwrap_err();
    assert!(matches!(err, Error::Unreachable(_)), "{err}");
}

#[tokio::test]
async fn a_message_can_be_built_before_the_channel_is() {
    let message = Message::new()
        .title("s10 disk")
        .timestamp(1_760_000_000)
        .sound(Sound::Pager);
    let stub = Stub::answering(202, ACCEPTED);
    let channel: Channel = channel(&stub);
    channel.send(message).await.unwrap();
    assert!(stub.last_body().contains("timestamp=1760000000"));
}
