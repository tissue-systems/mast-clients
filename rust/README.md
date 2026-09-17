# mast

A client for one [Mast](https://tissue.systems/docs/mast/connect/) channel: send
a page, then wait for somebody to acknowledge it.

```toml
[dependencies]
mast-pager = "0.1"
```

The crate is `mast-pager` and the library is `mast`.

```rust
use mast::{Channel, Message};
use std::time::Duration;

let channel = Channel::open(&std::env::var("MAST_URL")?)?;

let sent = channel
    .page(
        Message::new()
            .title("s10 disk")
            .body("92% and climbing")
            .url("https://grafana.example/d/disk")
            .dedupe_key("disk-s10")
            .retry(Duration::from_secs(60))
            .expire(Duration::from_secs(3600)),
    )
    .await?;

// Only a heartbeat comes back without an id.
let status = channel
    .wait(sent.id.as_deref().unwrap(), Some(Duration::from_secs(300)))
    .await?;
if status.acknowledged() {
    println!("{:?} answered after {:?}", status.acked_by, status.open_for());
}
```

The channel URL is the whole credential. Read it from the environment or a
secret store; anyone holding it can page the phone. `Display` on a `Channel`
prints only the first few characters of the key, so logging one is safe.

## Sending

Every field is optional and set on a builder:

| Method | What it does |
|---|---|
| `title`, `body` | 250 and 4096 characters |
| `priority` | `Priority::Quiet`, `Normal`, `Loud`, `Page` |
| `sound` | `Sound::Default`, `Bleep`, `Chirp`, `Klaxon`, `Pager`, `Rising` |
| `url`, `url_title` | A link on the card: the dashboard, the runbook |
| `dedupe_key` | A second send under an open one folds into it |
| `retry`, `expire` | `Duration`s; `no_retry` and `no_expire` turn the channel's defaults off |
| `ack` | Keep repeating until somebody answers |
| `silent` | Deliver without a sound; cannot be combined with `ack` or a page |

Priority and sound are enums rather than strings. A sound name the app does not
have is not an error on the phone, which plays its own tone and reports nothing,
so a typo would be a page that quietly lost its alarm; here it does not compile.

Anything the server would refuse is refused here first, so a doomed send does
not spend one of the channel's 60 requests a minute.

`page` sends at the pager tier. `resolve_key` closes whatever that dedupe key
has open, and `resolve_message` closes one specific card.

## Vitals

A vital is a channel that expects to hear from you. `ping` with an empty
`Message` is a heartbeat, which stores no card and returns no id. `fail`
flatlines it now instead of waiting out the grace window.

```rust
channel.ping(Message::new()).await?;
channel.fail(Message::new().body("backup exited 1")).await?;
```

## Checking a key

`check` proves a key works without sending anything and without a phone going
off. It leans on the status route resolving the key before it validates the
message id, so an id that cannot exist separates "No such channel." from "No
such message."

```rust
channel.check().await?;
```

## Waiting on more than one page

`wait` is a `Poller` with one page on it. Use the poller directly when several
are open at once:

```rust
let mut poller = Poller::new(&channel);
poller.watch(&first)?;
poller.watch(&second)?;

for (id, status) in poller.run(Some(Duration::from_secs(600))).await? {
    println!("{id} {:?} {:?}", status.state, status.acked_by);
}
```

It asks about a page every 3 seconds for the first half minute, every 10 up to
five minutes, then every 30, and spends at most 30 requests a minute so half the
channel's budget is always left for sending. When several pages are open and the
budget runs short, the oldest is asked about first. A 429 backs off the whole
channel, because the limit belongs to the key.

A timeout is not a failed page: whatever is still open stays watched, so calling
`run` again picks up where it left off.

## Errors

One `Error` enum, matched on rather than downcast:

```rust
match channel.send(message).await {
    Ok(sent) => println!("stored as {:?}", sent.id),
    Err(Error::UnknownChannel(reason)) => return Err(reason.into()),
    Err(Error::RateLimited { retry_after }) => tokio::time::sleep(retry_after).await,
    Err(Error::Rejected(reason)) => eprintln!("that send cannot be accepted: {reason}"),
    Err(Error::Unavailable { status, .. }) => eprintln!("Mast answered {status}"),
    Err(err) => eprintln!("{err}"),
}
```

`UnknownChannel` covers unknown, revoked and rotated away: Mast answers all of
them alike so that a key cannot be probed.

`Unavailable` is deliberately not `Rejected`: a 5xx does not prove the message
was not stored, so retrying blind can page somebody twice. Retry with a dedupe
key, or not at all.

## Transports

Rust has no HTTP client in the standard library. The crate ships one built on
`reqwest` and enabled by default, and takes one of yours instead:

```toml
mast-pager = { version = "0.1", default-features = false }
```

Without the default feature the crate is `serde_json` and nothing else: the wire
rules, the poller and the error mapping, driven by a `Transport` you implement
against the client your service already holds. That is also how the tests run,
which is why they open no sockets.

Leave redirects off whichever client you use. The key is in the URL, so
following one would hand it to whoever answered; the bundled transport refuses,
and a redirect that reaches the crate is reported as `Unreachable` rather than
chased.

## Tests

```sh
cargo test
```
