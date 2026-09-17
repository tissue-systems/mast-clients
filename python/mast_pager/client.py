"""A client for one Mast channel.

The channel key is the whole credential. No account, no token, and nothing
inbound, which is also why an acknowledgement is polled rather than pushed: the
boxes that send pages (a cron host, a home server, a CI runner) are usually not
reachable from the internet, so the only way to learn that somebody answered is
to ask.

Nothing outside the standard library is imported here, so this module can be
copied into a project that does not want a dependency. See tools/vendor.py.
"""

import collections
import datetime
import json
import re
import time
import urllib.error
import urllib.parse
import urllib.request

__all__ = [
    "Channel",
    "Poller",
    "Sent",
    "Status",
    "MastError",
    "Unreachable",
    "UnknownChannel",
    "UnknownMessage",
    "RateLimited",
    "Rejected",
    "parse_channel_url",
    "is_message_id",
]

DEFAULT_HOST = "https://mast.tissue.dev"

_KEY = re.compile(r"mk_[0-9a-f]{40}\Z")
_MESSAGE_ID = re.compile(r"mm_[0-9a-f]{32}\Z")

PRIORITIES = ("quiet", "normal", "loud", "page")
SOUNDS = ("default", "bleep", "chirp", "klaxon", "pager", "rising")

# States a message never leaves. Anything else is still in play and worth
# another poll.
TERMINAL_STATES = frozenset(("acked", "resolved", "expired"))

# The server's own caps, so a send that cannot be accepted costs nothing.
MAX_TITLE_CHARS = 250
MAX_BODY_CHARS = 4096
MAX_URL_CHARS = 512
MAX_DEDUPE_KEY_CHARS = 120
MIN_RETRY_SECS, MAX_RETRY_SECS = 30, 86_400
MIN_EXPIRE_SECS, MAX_EXPIRE_SECS = 60, 604_800

# 60 requests a minute per key, shared between sends and polls. Half is left for
# sends, because a client that polls itself out of a send budget cannot report
# the next outage.
RATE_LIMIT_PER_MIN = 60
POLL_BUDGET_PER_MIN = 30

# Age of an open page in seconds -> how often to ask about it. Read in order;
# the first rung whose bound the page has not passed wins.
POLL_LADDER = ((30, 3.0), (300, 10.0), (None, 30.0))

_TIMEOUT = 15


class MastError(Exception):
    """Anything that went wrong talking to Mast."""


class Unreachable(MastError):
    """Mast could not be reached, or did not answer in time."""


class UnknownChannel(MastError):
    """The key does not resolve.

    Unknown, malformed, revoked, or rotated away past its grace: Mast answers
    all four the same way so that a key cannot be probed, and a client cannot
    tell them apart either.
    """


class UnknownMessage(MastError):
    """The key is good but the message id is not one of its own."""


class RateLimited(MastError):
    """Over 60 a minute on this key, or 600 a minute across the owner."""

    def __init__(self, retry_after=1.0):
        MastError.__init__(self, "Rate limited by Mast")
        self.retry_after = retry_after


class Rejected(MastError):
    """The send was refused and the offending field was named."""


class Sent:
    """What came back from an accepted send."""

    def __init__(self, payload):
        self.id = payload.get("id")
        self.state = payload.get("state") or "queued"
        self.duplicate = bool(payload.get("duplicate"))
        self.open_for = payload.get("open_for")
        self.payload = payload

    def __repr__(self):
        return "Sent(id=%r, state=%r)" % (self.id, self.state)


class Status:
    """One message's lifecycle, as GET .../messages/<id> reports it."""

    def __init__(self, payload):
        self.id = payload.get("id")
        self.state = payload.get("state") or ""
        self.received_at = payload.get("received_at")
        self.dedupe_count = payload.get("dedupe_count") or 0
        self.acked_at = payload.get("acked_at")
        self.acked_by = payload.get("acked_by")
        self.resolved_at = payload.get("resolved_at")
        self.expires_at = payload.get("expires_at")
        self.payload = payload

    @property
    def acknowledged(self):
        return self.state == "acked"

    @property
    def done(self):
        return self.state in TERMINAL_STATES

    @property
    def open_for(self):
        """Whole seconds from arrival to the ack, or None if either is missing.

        Mast leaves a stamp out rather than zeroing it, so absence means "not
        known" and not "instant".
        """
        end = self.acked_at or self.resolved_at
        start = _parse_stamp(self.received_at)
        finish = _parse_stamp(end)
        if start is None or finish is None:
            return None
        return max(0, int(finish - start))

    def __repr__(self):
        return "Status(id=%r, state=%r)" % (self.id, self.state)


def is_message_id(value):
    return bool(_MESSAGE_ID.match(value or ""))


def parse_channel_url(raw):
    """Split a channel URL into (origin, prefix, key).

    Takes the short form people copy out of the app
    (https://mast.tissue.dev/mk_...), the explicit ingest path the management
    API serves (.../m/mk_...), a host with no scheme, and a bare key. The
    prefix that was pasted is kept rather than normalised, so a self-hosted
    edge without the "/" to "/m/" rewrite still works.
    """
    raw = (raw or "").strip()
    if not raw:
        raise ValueError("No channel URL given")

    if _KEY.match(raw):
        return DEFAULT_HOST, "", raw

    if "://" not in raw:
        raw = "https://" + raw

    parts = urllib.parse.urlsplit(raw)
    if parts.scheme not in ("http", "https") or not parts.netloc:
        raise ValueError("A channel URL has to be an http(s) URL")

    segments = [s for s in parts.path.split("/") if s]
    prefix = ""
    if segments and segments[0] == "m":
        prefix = "/m"
        segments = segments[1:]
    if not segments:
        raise ValueError("There is no key in that URL")

    key = segments[0]
    if not _KEY.match(key):
        raise ValueError("%r is not a channel key: expected mk_ and 40 hex characters" % key)

    return "%s://%s" % (parts.scheme, parts.netloc), prefix, key


class Channel:
    """One channel's worth of Mast."""

    def __init__(self, url, timeout=_TIMEOUT, opener=None):
        self.origin, self.prefix, self.key = parse_channel_url(url)
        self.timeout = timeout
        self._opener = opener or urllib.request.build_opener()

    @property
    def url(self):
        return "%s%s/%s" % (self.origin, self.prefix, self.key)

    def __repr__(self):
        # The key is a credential, so never the whole of it.
        return "Channel(%s%s/%s...)" % (self.origin, self.prefix, self.key[:7])

    def send(self, body=None, title=None, priority=None, url=None, url_title=None,
             key=None, sound=None, retry=None, expire=None, ack=None, silent=False,
             callback=None, timestamp=None, resolve=None):
        """Post one message. `key` is the dedupe key, as on the wire.

        Returns a Sent. A send answers once the message is stored, before the
        push leaves for Apple, so this is not a promise that a phone made a
        noise.
        """
        fields = _build(body=body, title=title, priority=priority, url=url,
                        url_title=url_title, key=key, sound=sound, retry=retry,
                        expire=expire, ack=ack, silent=silent, callback=callback,
                        timestamp=timestamp, resolve=resolve)
        return Sent(self._post(self.url, fields))

    def page(self, body=None, title=None, **kw):
        """A send at the pager tier: repeats until somebody acknowledges it.

        A page is the one tier storm control does not fold, so give it a `key`
        if the thing sending it can flap.
        """
        kw["priority"] = "page"
        return self.send(body=body, title=title, **kw)

    def resolve(self, key=None, message_id=None, body=None, title=None, **kw):
        """Close an open incident. The card turns green and stops repeating.

        Either the dedupe key the page carried, or the message id from its own
        send. A resolve that finds nothing open is not an error: it is stored
        quietly and wakes nobody.
        """
        if message_id is not None:
            if not is_message_id(message_id):
                raise Rejected("%r is not a message id" % message_id)
            kw["resolve"] = message_id
        elif key:
            kw["resolve"] = "1"
            kw["key"] = key
        else:
            raise Rejected("A resolve needs a key or a message id")
        return self.send(body=body, title=title, **kw)

    def ping(self, body=None, title=None, **kw):
        """A heartbeat on a vital. With no text it is proof of life and no card.

        The answering state is "alive" and there is no id to follow, so nothing
        to poll.
        """
        return self.send(body=body, title=title, **kw)

    def fail(self, body=None, title=None, **kw):
        """Flatline a vital now, instead of waiting out its grace window.

        Only a vital has this route. On a plain channel it answers the same 404
        an unknown key gets, because saying otherwise would describe the
        channel to whoever is holding the key.
        """
        fields = _build(body=body, title=title, **kw)
        return Sent(self._post(self.url + "/fail", fields))

    def status(self, message_id):
        """Read one message back, acknowledgement included."""
        if not is_message_id(message_id):
            raise Rejected("%r is not a message id" % message_id)
        return Status(self._get(self._message_url(message_id)))

    def check(self):
        """Prove the key works without sending anything.

        The status route resolves the key before it looks at the message id, so
        asking about an id that cannot exist separates the two 404s: a bad key
        says "No such channel." and a good one says "No such message." Nothing
        is stored and no phone goes off, which is what makes this safe to run
        during setup.
        """
        try:
            self._get(self._message_url("check"))
        except UnknownMessage:
            return True
        raise MastError("Mast answered a key check in a way this version does not expect")

    def wait(self, message_id, timeout=None, clock=time.monotonic, sleep=time.sleep):
        """Poll one message until it settles, or until the timeout runs out.

        Returns the last Status read. A timeout leaves the page open on the
        phone: the person who was paged can still answer it, and the automation
        that gave up waiting is usually not the only one that cares.
        """
        poller = Poller(self, clock=clock, sleep=sleep)
        poller.watch(message_id)
        settled = poller.run(timeout=timeout)
        return settled.get(message_id) or poller.last_status(message_id)

    def _message_url(self, message_id):
        return "%s/messages/%s" % (self.url, message_id)

    def _post(self, url, fields):
        data = urllib.parse.urlencode(fields).encode("utf-8")
        headers = {"Content-Type": "application/x-www-form-urlencoded"}
        status, payload = self._request(urllib.request.Request(url, data=data, headers=headers))
        if status == 400:
            raise Rejected(_message(payload, "Mast refused the send"))
        if status == 413:
            raise Rejected("The request is over Mast's 16 KiB limit")
        if status >= 500:
            # A 502 does not prove nothing was stored. The failure can land on
            # either side of the durability line, so a blind retry can page
            # twice; retry with a dedupe key or not at all.
            raise MastError(_message(payload, "Mast answered %d" % status))
        return payload

    def _get(self, url):
        status, payload = self._request(urllib.request.Request(url))
        if status >= 400:
            raise MastError(_message(payload, "Mast answered %d" % status))
        return payload

    def _request(self, request):
        try:
            response = self._opener.open(request, timeout=self.timeout)
            with response:
                return response.getcode(), _body(response)
        except urllib.error.HTTPError as err:
            with err:
                payload = _body(err)
            if err.code == 429:
                raise RateLimited(_retry_after(err.headers.get("Retry-After")))
            if err.code == 404:
                text = _message(payload, "No such channel.")
                if "No such message" in text:
                    raise UnknownMessage(text)
                raise UnknownChannel(text)
            return err.code, payload
        except urllib.error.URLError as err:
            raise Unreachable("Could not reach Mast: %s" % err.reason)
        except OSError as err:
            raise Unreachable("Could not reach Mast: %s" % err)


class _Budget:
    """At most per_minute requests in any sixty seconds.

    A token bucket would be the usual answer, but a full one lets through its
    own depth on top of the refill, which is how a limiter sized at half the
    allowance ends up spending nearly all of it.
    """

    def __init__(self, per_minute, clock):
        self.per_minute = per_minute
        self._clock = clock
        self._spent = collections.deque()

    def take(self):
        now = self._clock()
        while self._spent and now - self._spent[0] >= 60.0:
            self._spent.popleft()
        if len(self._spent) >= self.per_minute:
            return False
        self._spent.append(now)
        return True


class _Watched:
    def __init__(self, message_id, at):
        self.message_id = message_id
        self.started = at
        self.last_poll = 0.0
        self.status = None

    def due(self, now):
        age = now - self.started
        for bound, every in POLL_LADDER:
            if bound is None or age < bound:
                return now - self.last_poll >= every
        return False


class Poller:
    """Watches open pages on one channel and stops when they settle.

    Several pages at once share the channel's poll budget, so the oldest is
    asked about first when there is not enough of it to go round: the page that
    has been waiting longest is the one somebody is still waiting on. A 429
    backs the whole channel off rather than the one page, because the limit
    belongs to the key.
    """

    def __init__(self, channel, budget_per_min=POLL_BUDGET_PER_MIN,
                 clock=time.monotonic, sleep=time.sleep):
        self.channel = channel
        self._clock = clock
        self._sleep = sleep
        self._budget = _Budget(budget_per_min, clock)
        self._open = {}
        self._seen = {}
        self._paused_until = 0.0

    def watch(self, message_id):
        if not is_message_id(message_id):
            raise Rejected("%r is not a message id" % message_id)
        if message_id not in self._open:
            self._open[message_id] = _Watched(message_id, self._clock())
        return self._open[message_id]

    @property
    def open_count(self):
        return len(self._open)

    def last_status(self, message_id):
        return self._seen.get(message_id)

    def tick(self):
        """Ask about whatever is due and affordable. Returns what settled."""
        now = self._clock()
        settled = {}
        if now < self._paused_until:
            return settled

        due = [w for w in self._open.values() if w.due(now)]
        due.sort(key=lambda w: w.started)
        for watched in due:
            if not self._budget.take():
                break
            watched.last_poll = now
            try:
                status = self.channel.status(watched.message_id)
            except RateLimited as err:
                self._paused_until = self._clock() + err.retry_after
                break
            except (UnknownMessage, UnknownChannel):
                # Nothing further is going to happen to a message the channel
                # will not talk about, so stop asking.
                del self._open[watched.message_id]
                continue
            except MastError:
                # A blip is not an answer. Leave the page open and try again on
                # the next tick.
                continue

            self._seen[watched.message_id] = status
            watched.status = status
            if status.done:
                del self._open[watched.message_id]
                settled[watched.message_id] = status
        return settled

    def run(self, timeout=None, interval=1.0):
        """Tick until nothing is open, or the timeout runs out.

        Returns the statuses that settled. A timeout is not a failure and
        anything still open stays watched, so calling run() again picks up
        where this one left off.
        """
        deadline = None if timeout is None else self._clock() + timeout
        settled = {}
        while self._open:
            settled.update(self.tick())
            if not self._open:
                break
            if deadline is not None and self._clock() >= deadline:
                break
            self._sleep(interval)
        return settled


def _build(body=None, title=None, priority=None, url=None, url_title=None, key=None,
           sound=None, retry=None, expire=None, ack=None, silent=False, callback=None,
           timestamp=None, resolve=None):
    fields = {}
    if title is not None:
        fields["title"] = _capped("title", title, MAX_TITLE_CHARS)
    if body is not None:
        fields["body"] = _capped("body", body, MAX_BODY_CHARS)

    if priority is not None:
        if priority not in PRIORITIES:
            raise Rejected("Field 'priority' must be one of %s" % ", ".join(PRIORITIES))
        fields["priority"] = priority

    if url is not None:
        fields["url"] = _capped("url", url, MAX_URL_CHARS)
        if not url.startswith(("http://", "https://")):
            raise Rejected("Field 'url' must start with http:// or https://")
    if url_title is not None:
        fields["url_title"] = _capped("url_title", url_title, MAX_TITLE_CHARS)
    if callback is not None:
        fields["callback"] = _capped("callback", callback, MAX_URL_CHARS)
        if not callback.startswith("https://"):
            raise Rejected("Field 'callback' must start with https://")

    if key is not None:
        fields["key"] = _capped("key", key, MAX_DEDUPE_KEY_CHARS)
    if sound is not None:
        name = sound[:-4] if sound.endswith(".caf") else sound
        if name not in SOUNDS:
            # iOS plays the default tone for a name it does not have and reports
            # nothing, so a typo here is a page that silently loses its alarm.
            raise Rejected("Field 'sound' must be one of %s" % ", ".join(SOUNDS))
        fields["sound"] = sound

    if retry is not None:
        fields["retry"] = str(_secs("retry", retry, MIN_RETRY_SECS, MAX_RETRY_SECS))
    if expire is not None:
        fields["expire"] = str(_secs("expire", expire, MIN_EXPIRE_SECS, MAX_EXPIRE_SECS))

    if ack:
        fields["ack"] = "required"
    if silent:
        if ack or fields.get("priority") == "page":
            raise Rejected("Field 'silent' cannot be combined with 'ack' or priority 'page'")
        fields["silent"] = "1"

    if timestamp is not None:
        fields["timestamp"] = str(timestamp)
    if resolve is not None:
        fields["resolve"] = str(resolve)

    # No check for an empty set of fields: a send with nothing in it is a
    # vital's heartbeat.
    return fields


def _capped(name, value, limit):
    value = str(value)
    if len(value) > limit:
        raise Rejected("Field '%s' is longer than %d characters" % (name, limit))
    return value


def _secs(name, value, low, high):
    value = int(value)
    if value == 0:
        return 0  # 0 turns the channel's default off, at every layer
    if value < low or value > high:
        raise Rejected("Field '%s' must be 0 or between %d and %d seconds" % (name, low, high))
    return value


def _body(response):
    try:
        raw = response.read()
    except OSError:
        return {}
    if not raw:
        return {}
    try:
        payload = json.loads(raw.decode("utf-8", "replace"))
    except ValueError:
        # A proxy in front of a self-hosted edge can answer with something that
        # is not JSON. That is a transport problem, not a message.
        return {}
    return payload if isinstance(payload, dict) else {}


def _message(payload, default):
    """The human half of an error body.

    Mast nests it: {"error": {"code", "message"}}. Reading payload["message"]
    finds nothing and falls back to the default, which turns a good key's "No
    such message." into "No such channel." and fails setup for every valid key.
    """
    error = payload.get("error")
    if isinstance(error, dict) and isinstance(error.get("message"), str):
        return error["message"]
    if isinstance(error, str) and error:
        return error
    if isinstance(payload.get("message"), str):
        return payload["message"]
    return default


def _retry_after(header):
    try:
        return max(1.0, float(header))
    except (TypeError, ValueError):
        return 1.0


def _parse_stamp(value):
    """Seconds since the epoch from an ISO-8601 instant, or None."""
    if not value:
        return None
    text = str(value).strip()
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        return datetime.datetime.fromisoformat(text).timestamp()
    except ValueError:
        return None
