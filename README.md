# mast-clients

Client libraries for the [Mast](https://tissue.systems/docs/mast/connect/) ingest
API: send to a channel, and wait for the acknowledgement.

Mast is a pager for one iPhone:
[Mast Pager](https://apps.apple.com/us/app/mast-pager/id6805232044?pt=129362925&ct=github-clients&mt=8)
on the App Store. A channel is a URL the app hands you, and these libraries
are the sending end.

| | Directory | Install | Tests |
|---|---|---|---|
| Python | [`python/`](python) | `pip install mast-pager`, or vendor [one file](python/vendor/mast.py) | `cd python && python3 -m unittest discover -s tests -t .` |
| JavaScript | [`javascript/`](javascript) | `npm install mast-pager` | `cd javascript && node --test` |
| Go | [`go/`](go) | `go get github.com/tissue-systems/mast-clients/go` | `cd go && go test ./...` |
| Rust | [`rust/`](rust) | `cargo add mast-pager` | `cd rust && cargo test` |

The wire format is form-encoded POSTs and JSON replies, and the channel key in
the URL is the whole credential, so there is nothing to sign and no SDK to drag
in. Python, JavaScript and Go are standard library only. Rust has neither HTTP
nor JSON in its standard library, so the crate takes `serde_json` and bundles a
`reqwest` transport that can be switched off for one of your own.

## What they are actually for

The HTTP is four lines in any language. What is worth sharing is the handful of
things that are wrong the first time everybody writes them:

- **The channel URL comes in several shapes.** The short form the app copies,
  the `/m/` form the management API hands out, a host with no scheme, a bare
  key. The edge rewrites `/` onto `/m/`, a self-hosted one in front of the
  management API may not, so whichever prefix was pasted is kept rather than
  normalised.
- **The error body is nested.** It is `{"error": {"code", "message"}}` at every
  status. Reading `payload["message"]` finds nothing and falls back to whatever
  default the caller supplied, which turns a good key's "No such message." into
  "No such channel." and fails setup for every valid key.
- **A key can be checked without sending anything.** The status route resolves
  the key before it validates the message id, so asking about an id that cannot
  exist tells the two 404s apart. Nothing is stored and no phone goes off, which
  is what makes it usable in a config dialog.
- **The acknowledgement is polled, not pushed.** Mast stores a `callback=` on a
  message and does not fire it, and most senders are not reachable from the
  internet anyway. Polling costs requests against the channel's 60 a minute,
  shared with sends, so each client spends at most half of that, slows the ladder
  down as a page ages (3s, then 10s, then 30s), asks about the oldest page first
  when the budget runs short, and backs off the whole channel on a 429 because
  the limit belongs to the key.

One more thing every client does the same way: a 5xx is never reported as a
refused send. It does not prove the message was not stored, so retrying blind
can page somebody twice. Retry with a dedupe key, or not at all.

## Wire contract

`POST /m/{key}` sends, `POST /m/{key}/fail` flatlines a vital, and
`GET /m/{key}/messages/{id}` reads one message's state. Fields, caps, states and
the incident lifecycle are documented at
<https://tissue.systems/docs/mast/connect/>.

## One repo, four packages

Each directory is a package that stands alone and releases on its own tag,
prefixed with its directory:

```
python/v0.1.0      PyPI       mast-pager
javascript/v0.1.0  npm        mast-pager
go/v0.1.0          the module path itself
rust/v0.1.0        crates.io  mast-pager
```

Go is the one that needs the prefix: a module in a subdirectory is fetched as
`github.com/tissue-systems/mast-clients/go` and versioned by tags named
`go/vX.Y.Z`. PyPI, npm and crates.io all publish from a subdirectory with
nothing special. CI is split the same way, one workflow per language filtered on
its own paths, so a change to the Python client does not run the Go tests.

They share a repo because they share the four traps above, and a fix to the poll
ladder or the error body is one pull request instead of four. Splitting one out
later is cheap for Python, JavaScript and Rust, whose package names would not
change, and costs Go an import path, so that is the one to move early if it ever
moves.
