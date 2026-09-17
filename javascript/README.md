# mast-pager

A client for one [Mast](https://tissue.systems/docs/mast/connect/) channel: send a
page, then wait for somebody to acknowledge it.

Mast is a pager for one iPhone:
[Mast Pager](https://apps.apple.com/us/app/mast-pager/id6805232044?pt=129362925&ct=npm-client&mt=8)
on the App Store. A channel is a URL the app hands you, and this library is the
sending end.

No dependencies, one file, nothing but `fetch`, so it runs on Node 18 and later
and inside a Cell.

```js
import { Channel } from "mast-pager";

const mast = new Channel(process.env.MAST_URL);

const sent = await mast.page({
  title: "s10 disk",
  body: "92% and climbing",
  url: "https://grafana.example/d/disk",
  key: "disk-s10",
  retry: 60,
  expire: 3600,
});

const status = await mast.wait(sent.id, { timeout: 300000 });
if (status.acknowledged) {
  console.log(`${status.ackedBy} answered after ${status.openFor}s`);
}
```

The channel URL is the whole credential. Take it from the environment or a
vault binding; anyone holding it can page the phone.

## Sending

`send(message)` takes the ingest fields, camel-cased where the wire uses
underscores (`urlTitle` is `url_title`):

| Field | What it does |
|---|---|
| `title`, `body` | 250 and 4096 characters |
| `priority` | `quiet`, `normal`, `loud`, `page` |
| `sound` | `default`, `bleep`, `chirp`, `klaxon`, `pager`, `rising` |
| `url`, `urlTitle` | A link on the card: the dashboard, the runbook |
| `key` | Dedupe key: a second send under an open one folds into it |
| `retry`, `expire` | Seconds; `0` turns the channel's default off |
| `ack` | Keep repeating until somebody answers |
| `silent` | Deliver without a sound; cannot be combined with `ack` or a page |

Anything the server would refuse is refused here first, so a doomed send does
not spend one of the channel's 60 requests a minute.

`page()` is `send()` at the pager tier. `resolve({ key })` closes whatever that
key has open, and `resolve({ messageId })` closes one specific card.

## Vitals

A vital is a channel that expects to hear from you. `ping()` with no arguments
is a heartbeat, which stores no card and returns no id. `fail()` flatlines it
now instead of waiting out the grace window.

```js
await mast.ping();
await mast.fail({ body: "backup exited 1" });
```

## Checking a key

`check()` proves a key works without sending anything and without a phone going
off. It leans on the status route resolving the key before it validates the
message id, so an id that cannot exist separates "No such channel." from "No
such message."

```js
if (await mast.check()) console.log("key is good");
```

## Waiting on more than one page

`wait()` is a `Poller` with one page on it. Use the poller directly when
several are open at once:

```js
import { Poller } from "mast-pager";

const poller = new Poller(mast);
poller.watch(first.id);
poller.watch(second.id);

for (const [id, status] of await poller.run({ timeout: 600000 })) {
  console.log(id, status.state, status.ackedBy);
}
```

It asks about a page every 3 seconds for the first half minute, every 10 up to
five minutes, then every 30, and spends at most 30 requests a minute so that
half the channel's budget is always left for sending. When several pages are
open and the budget runs short, the oldest is asked about first. A 429 backs off
the whole channel, because the limit belongs to the key.

## Errors

Everything thrown extends `MastError`.

| Error | Means |
|---|---|
| `Unreachable` | Mast did not answer, or answered a redirect |
| `UnknownChannel` | The key does not resolve: wrong, revoked, or rotated |
| `UnknownMessage` | The key is good, the message id is not one of its own |
| `RateLimited` | Over the budget; `retryAfter` says for how long |
| `Rejected` | The send was refused and the field is named |

A 5xx stays a bare `MastError` on purpose: it does not prove nothing was
stored, so retrying blind can page twice. Retry with a dedupe key, or not at
all.

## Tests

```sh
node --test
```
