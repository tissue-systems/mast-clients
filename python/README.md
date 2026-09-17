# mast-pager

Send to a [Mast](https://tissue.systems/docs/mast/connect/) channel from Python, and wait for
somebody to acknowledge the page.

Mast is a pager for one iPhone:
[Mast Pager](https://apps.apple.com/us/app/mast-pager/id6805232044?pt=129362925&ct=pypi-client&mt=8)
on the App Store. A channel is a URL the app hands you, and this library is the
sending end.

```
pip install mast-pager
```

Nothing outside the standard library, on purpose: this gets installed on the box that runs the
backup script, not on a laptop.

```python
from mast_pager import Channel

mast = Channel("https://mast.tissue.dev/mk_...")   # or the bare key

mast.send(title="db-01", body="replication stopped", priority="loud")

sent = mast.page(title="db-01 unreachable", body="no WAL segment in 11 minutes", key="db-01")
if mast.wait(sent.id, timeout=300).acknowledged:
    print("somebody is on it")

mast.resolve(key="db-01", title="db-01 recovered", body="WAL shipping resumed")
```

`wait()` returns the last state it read. A timeout is not a failure and does not close anything:
the page is still on the phone and can still be answered, which is worth knowing before wiring
this into a deploy gate.

## Checking a key

`check()` proves a key works without sending anything, which is what you want in a setup step or
a config check:

```python
try:
    mast.check()
except UnknownChannel:
    ...
```

## Vitals

A vital is a channel that pages when it stops hearing from you.

```python
mast.ping()                          # proof of life, no card on the phone
mast.ping(body="swept 412 rows")     # same, with something to read
mast.fail(body="pg_dump exit 1")     # flatline now, do not wait out the grace window
```

## Several pages at once

`Poller` watches more than one page on the same channel and keeps inside its request budget: 30 a
minute, half of what the key is allowed, so polling cannot starve the sends. Pages are asked
about oldest first when there is not enough budget to go round, and a 429 backs the whole channel
off rather than the one page.

```python
from mast_pager import Poller

watch = Poller(mast)
watch.watch(first.id)
watch.watch(second.id)
for message_id, status in watch.run(timeout=600).items():
    print(message_id, status.state, status.acked_by)
```

## Errors

Everything raises `MastError` or a subclass: `UnknownChannel` (the key does not resolve, which
covers unknown, revoked and rotated away), `UnknownMessage`, `RateLimited` (carries
`retry_after`), `Rejected` (a field the server or this library will not accept) and
`Unreachable`.

A send that fails with a 5xx is not proof that nothing was stored, so a blind retry can page
twice. Retry with a dedupe `key` or not at all.

## One file, no package

`vendor/mast.py` is the same code as a single file for a project that will not take a dependency.
Copy it in. It is generated, so change `mast_pager/client.py` and run `python tools/vendor.py`.

## Tests

```
python -m unittest discover -s tests -t .
```
