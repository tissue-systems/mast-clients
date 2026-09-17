# mast

A client for one [Mast](https://tissue.systems/docs/mast/connect/) channel: send
a page, then wait for somebody to acknowledge it.

Mast is a pager for one iPhone:
[Mast Pager](https://apps.apple.com/us/app/mast-pager/id6805232044?pt=129362925&ct=godoc-client&mt=8)
on the App Store. A channel is a URL the app hands you, and this library is the
sending end.

Standard library only.

```go
import mast "github.com/tissue-systems/mast-clients/go"
```

The module path ends in `go` and the package is `mast`, so name it in the
import.

```go
channel, err := mast.Open(os.Getenv("MAST_URL"))
if err != nil {
	return err
}

sent, err := channel.Page(ctx, mast.Message{
	Title:  "s10 disk",
	Body:   "92% and climbing",
	URL:    "https://grafana.example/d/disk",
	Key:    "disk-s10",
	Retry:  time.Minute,
	Expire: time.Hour,
})
if err != nil {
	return err
}

ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
defer cancel()

status, err := channel.Wait(ctx, sent.ID)
if err != nil && !errors.Is(err, context.DeadlineExceeded) {
	return err
}
if status.Acknowledged() {
	open, _ := status.OpenFor()
	log.Printf("%s answered after %s", status.AckedBy, open)
}
```

The channel URL is the whole credential. Read it from the environment or a
secret store; anyone holding it can page the phone.

## Sending

Every field of `Message` is optional:

| Field | What it does |
|---|---|
| `Title`, `Body` | 250 and 4096 characters |
| `Priority` | `quiet`, `normal`, `loud`, `page` |
| `Sound` | `default`, `bleep`, `chirp`, `klaxon`, `pager`, `rising` |
| `URL`, `URLTitle` | A link on the card: the dashboard, the runbook |
| `Key` | Dedupe key: a second send under an open one folds into it |
| `Retry`, `Expire` | Durations; `DisableRetry` and `DisableExpire` turn the channel's defaults off |
| `Ack` | Keep repeating until somebody answers |
| `Silent` | Deliver without a sound; cannot be combined with `Ack` or a page |

Anything the server would refuse is refused here first, so a doomed send does
not spend one of the channel's 60 requests a minute.

`Page` sends at the pager tier. `ResolveKey` closes whatever that dedupe key
has open, and `ResolveMessage` closes one specific card.

## Vitals

A vital is a channel that expects to hear from you. `Ping` with an empty
`Message` is a heartbeat, which stores no card and returns no id. `Fail`
flatlines it now instead of waiting out the grace window.

```go
if _, err := channel.Ping(ctx, mast.Message{}); err != nil {
	log.Println("heartbeat did not land:", err)
}

if _, err := channel.Fail(ctx, mast.Message{Body: "backup exited 1"}); err != nil {
	log.Println("flatline did not land:", err)
}
```

## Checking a key

`Check` proves a key works without sending anything and without a phone going
off. It leans on the status route resolving the key before it validates the
message id, so an id that cannot exist separates "No such channel." from "No
such message."

```go
if err := channel.Check(ctx); err != nil {
	return fmt.Errorf("that channel URL does not work: %w", err)
}
```

## Waiting on more than one page

`Wait` is a `Poller` with one page on it. Use the poller directly when several
are open at once:

```go
poller := mast.NewPoller(channel)
poller.Watch(first.ID)
poller.Watch(second.ID)

settled, err := poller.Run(ctx)
for id, status := range settled {
	log.Println(id, status.State, status.AckedBy)
}
```

It asks about a page every 3 seconds for the first half minute, every 10 up to
five minutes, then every 30, and spends at most 30 requests a minute so half the
channel's budget is always left for sending. When several pages are open and the
budget runs short, the oldest is asked about first. A 429 backs off the whole
channel, because the limit belongs to the key.

## Errors

```go
errors.Is(err, mast.ErrUnreachable)     // no answer, or an answer that redirected
errors.Is(err, mast.ErrUnknownChannel)  // the key does not resolve
errors.Is(err, mast.ErrUnknownMessage)  // the key is good, the id is not its own

var limited *mast.RateLimited           // limited.RetryAfter says how long to wait
var rejected *mast.Rejected             // the send was refused, and the field is named
var down *mast.Unavailable              // a 5xx
errors.As(err, &limited)
```

An `Unavailable` is deliberately not a `Rejected`: a 5xx does not prove the
message was not stored, so retrying blind can page somebody twice. Retry with a
dedupe key, or not at all.

## Tests

```sh
go test ./...
```
