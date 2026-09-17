// A client for one Mast channel.
//
// The channel key is the whole credential, so this runs anywhere there is a
// fetch: Node, a Cell, a browser extension. There is nothing inbound, which is
// why an acknowledgement is polled rather than pushed.

export const DEFAULT_HOST = "https://mast.tissue.dev";

const KEY = /^mk_[0-9a-f]{40}$/;
const MESSAGE_ID = /^mm_[0-9a-f]{32}$/;

export const PRIORITIES = ["quiet", "normal", "loud", "page"];
export const SOUNDS = ["default", "bleep", "chirp", "klaxon", "pager", "rising"];

// States a message never leaves. Anything else is still in play.
export const TERMINAL_STATES = new Set(["acked", "resolved", "expired"]);

// The server's own caps, so a send that cannot be accepted costs nothing.
const MAX_TITLE_CHARS = 250;
const MAX_BODY_CHARS = 4096;
const MAX_URL_CHARS = 512;
const MAX_DEDUPE_KEY_CHARS = 120;
const RETRY_RANGE = [30, 86400];
const EXPIRE_RANGE = [60, 604800];

// 60 requests a minute per key, shared between sends and polls. Half is left
// for sends: a client that polls itself out of a budget cannot report the next
// outage.
export const RATE_LIMIT_PER_MIN = 60;
export const POLL_BUDGET_PER_MIN = 30;

// Age of an open page in seconds -> how often to ask about it. First rung the
// page has not aged past wins.
const POLL_LADDER = [
  [30, 3],
  [300, 10],
  [Infinity, 30],
];

const DEFAULT_TIMEOUT_MS = 15000;

export class MastError extends Error {
  constructor(message) {
    super(message);
    this.name = new.target.name;
  }
}

// Mast could not be reached, or did not answer in time.
export class Unreachable extends MastError {}

// The key does not resolve. Unknown, malformed, revoked or rotated away past
// its grace all answer the same way so that a key cannot be probed, and a
// client cannot tell them apart either.
export class UnknownChannel extends MastError {}

// The key is good, the message id is not one of its own.
export class UnknownMessage extends MastError {}

// Over 60 a minute on the key, or 600 across the owner.
export class RateLimited extends MastError {
  constructor(retryAfter = 1) {
    super("Rate limited by Mast");
    this.retryAfter = retryAfter;
  }
}

// The send was refused and the offending field was named.
export class Rejected extends MastError {}

export function isMessageId(value) {
  return MESSAGE_ID.test(String(value ?? ""));
}

// Split a channel URL into its parts. Takes the short form people copy out of
// the app, the explicit /m/ path the management API serves, a host with no
// scheme, and a bare key. The prefix that was pasted is kept rather than
// normalised, so an edge without the "/" to "/m/" rewrite still works.
export function parseChannelUrl(raw) {
  const text = String(raw ?? "").trim();
  if (!text) throw new TypeError("No channel URL given");
  if (KEY.test(text)) return { origin: DEFAULT_HOST, prefix: "", key: text };

  let parsed;
  try {
    parsed = new URL(text.includes("://") ? text : `https://${text}`);
  } catch {
    throw new TypeError(`${text} is not a URL`);
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    throw new TypeError("A channel URL has to be an http(s) URL");
  }

  let segments = parsed.pathname.split("/").filter(Boolean);
  let prefix = "";
  if (segments[0] === "m") {
    prefix = "/m";
    segments = segments.slice(1);
  }
  if (segments.length === 0) throw new TypeError("There is no key in that URL");
  if (!KEY.test(segments[0])) {
    throw new TypeError(`${segments[0]} is not a channel key: expected mk_ and 40 hex characters`);
  }
  return { origin: parsed.origin, prefix, key: segments[0] };
}

export class Sent {
  constructor(payload) {
    this.id = payload.id ?? null;
    this.state = payload.state ?? "queued";
    this.duplicate = payload.duplicate === true;
    this.openFor = payload.open_for ?? null;
    this.payload = payload;
  }
}

export class Status {
  constructor(payload) {
    this.id = payload.id ?? null;
    this.state = payload.state ?? "";
    this.receivedAt = payload.received_at ?? null;
    this.dedupeCount = payload.dedupe_count ?? 0;
    this.ackedAt = payload.acked_at ?? null;
    this.ackedBy = payload.acked_by ?? null;
    this.resolvedAt = payload.resolved_at ?? null;
    this.expiresAt = payload.expires_at ?? null;
    this.payload = payload;
  }

  get acknowledged() {
    return this.state === "acked";
  }

  get done() {
    return TERMINAL_STATES.has(this.state);
  }

  // Whole seconds from arrival to the ack, or null if either stamp is missing.
  // Mast leaves a stamp out rather than zeroing it, so absent means unknown and
  // not instant.
  get openFor() {
    const start = Date.parse(this.receivedAt ?? "");
    const end = Date.parse(this.ackedAt ?? this.resolvedAt ?? "");
    if (Number.isNaN(start) || Number.isNaN(end)) return null;
    return Math.max(0, Math.floor((end - start) / 1000));
  }
}

export class Channel {
  constructor(url, options = {}) {
    const { origin, prefix, key } = parseChannelUrl(url);
    this.origin = origin;
    this.prefix = prefix;
    this.key = key;
    this.timeout = options.timeout ?? DEFAULT_TIMEOUT_MS;
    this._fetch = options.fetch ?? globalThis.fetch;
    if (typeof this._fetch !== "function") {
      throw new TypeError("No fetch available; pass one in options");
    }
  }

  get url() {
    return `${this.origin}${this.prefix}/${this.key}`;
  }

  // Never the whole key: this ends up in logs.
  toString() {
    return `Channel(${this.origin}${this.prefix}/${this.key.slice(0, 7)}...)`;
  }

  // Post one message. Answers once it is stored, before the push leaves for
  // Apple, so this is not a promise that a phone made a noise.
  async send(message = {}) {
    return new Sent(await this._post(this.url, fields(message)));
  }

  // The pager tier: repeats until somebody acknowledges it. A page is the one
  // tier storm control will not fold, so give it a key if its source can flap.
  async page(message = {}) {
    return this.send({ ...message, priority: "page" });
  }

  // Close an open incident: the card turns green and stops repeating. Either
  // the dedupe key the page carried or the message id from its own send. A
  // resolve that finds nothing open is not an error; it is stored quietly.
  async resolve(message = {}) {
    const { key, messageId, ...rest } = message;
    if (messageId !== undefined) {
      if (!isMessageId(messageId)) throw new Rejected(`${messageId} is not a message id`);
      return this.send({ ...rest, resolve: messageId });
    }
    if (!key) throw new Rejected("A resolve needs a key or a message id");
    return this.send({ ...rest, resolve: "1", key });
  }

  // A heartbeat on a vital. With no text it is proof of life and no card, so
  // there is no id to follow.
  async ping(message = {}) {
    return this.send(message);
  }

  // Flatline a vital now instead of waiting out its grace window. Only a vital
  // has this route; a plain channel answers the same 404 an unknown key gets.
  async fail(message = {}) {
    return new Sent(await this._post(`${this.url}/fail`, fields(message)));
  }

  async status(messageId) {
    if (!isMessageId(messageId)) throw new Rejected(`${messageId} is not a message id`);
    return new Status(await this._get(this._messageUrl(messageId)));
  }

  // Prove the key works without sending anything. The status route resolves
  // the key before it looks at the message id, so an id that cannot exist
  // separates the two 404s: a bad key says "No such channel." and a good one
  // says "No such message." Nothing is stored and no phone goes off.
  async check() {
    try {
      await this._get(this._messageUrl("check"));
    } catch (err) {
      if (err instanceof UnknownMessage) return true;
      throw err;
    }
    throw new MastError("Mast answered a key check in a way this version does not expect");
  }

  // Poll one message until it settles or the timeout runs out. Returns the
  // last status read; a timeout leaves the page open on the phone, where it can
  // still be answered.
  async wait(messageId, options = {}) {
    const poller = new Poller(this, options);
    poller.watch(messageId);
    const settled = await poller.run(options);
    return settled.get(messageId) ?? poller.lastStatus(messageId) ?? null;
  }

  _messageUrl(messageId) {
    return `${this.url}/messages/${messageId}`;
  }

  async _post(url, form) {
    const { status, payload } = await this._request(url, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams(form).toString(),
    });
    if (status === 400) throw new Rejected(errorMessage(payload, "Mast refused the send"));
    if (status === 413) throw new Rejected("The request is over Mast's 16 KiB limit");
    if (status >= 500) {
      // A 502 does not prove nothing was stored: the failure can land on either
      // side of the durability line, so a blind retry can page twice. Retry
      // with a dedupe key or not at all.
      throw new MastError(errorMessage(payload, `Mast answered ${status}`));
    }
    return payload;
  }

  async _get(url) {
    const { status, payload } = await this._request(url, { method: "GET" });
    if (status >= 400) throw new MastError(errorMessage(payload, `Mast answered ${status}`));
    return payload;
  }

  async _request(url, init) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeout);
    let response;
    try {
      response = await this._fetch(url, {
        ...init,
        signal: controller.signal,
        // The key is in the URL, so a redirect somewhere else would hand it to
        // whoever answered.
        redirect: "manual",
      });
    } catch (err) {
      if (err?.name === "AbortError") throw new Unreachable("Mast did not answer in time");
      throw new Unreachable(`Could not reach Mast: ${err?.message ?? err}`);
    } finally {
      clearTimeout(timer);
    }

    if (response.status === 0 || (response.status >= 300 && response.status < 400)) {
      throw new Unreachable("Mast answered with a redirect, which is not followed");
    }

    const payload = await readJson(response);
    if (response.status === 429) {
      throw new RateLimited(retryAfter(response.headers.get("retry-after")));
    }
    if (response.status === 404) {
      const text = errorMessage(payload, "No such channel.");
      if (text.includes("No such message")) throw new UnknownMessage(text);
      throw new UnknownChannel(text);
    }
    return { status: response.status, payload };
  }
}

// At most perMinute requests in any sixty seconds. A token bucket would be the
// usual answer, but a full one lets through its own depth on top of the
// refill, which is how a limiter sized at half the allowance spends nearly all
// of it.
class Budget {
  constructor(perMinute, now) {
    this.perMinute = perMinute;
    this._now = now;
    this._spent = [];
  }

  take() {
    const at = this._now();
    while (this._spent.length && at - this._spent[0] >= 60) this._spent.shift();
    if (this._spent.length >= this.perMinute) return false;
    this._spent.push(at);
    return true;
  }
}

class Watched {
  constructor(messageId, at) {
    this.messageId = messageId;
    this.started = at;
    this.lastPoll = 0;
  }

  due(at) {
    const age = at - this.started;
    for (const [bound, every] of POLL_LADDER) {
      if (age < bound) return at - this.lastPoll >= every;
    }
    return false;
  }
}

// Watches open pages on one channel and stops when they settle. Several at once
// share the channel's budget, so the oldest is asked about first when there is
// not enough to go round: it is the one somebody is still standing over. A 429
// backs the whole channel off rather than the one page, because the limit
// belongs to the key.
export class Poller {
  constructor(channel, options = {}) {
    this.channel = channel;
    this._budget = new Budget(
      options.budgetPerMinute ?? POLL_BUDGET_PER_MIN,
      options.now ?? defaultNow,
    );
    this._now = options.now ?? defaultNow;
    this._sleep = options.sleep ?? defaultSleep;
    this._open = new Map();
    this._seen = new Map();
    this._pausedUntil = 0;
  }

  watch(messageId) {
    if (!isMessageId(messageId)) throw new Rejected(`${messageId} is not a message id`);
    if (!this._open.has(messageId)) {
      this._open.set(messageId, new Watched(messageId, this._now()));
    }
    return this._open.get(messageId);
  }

  get openCount() {
    return this._open.size;
  }

  lastStatus(messageId) {
    return this._seen.get(messageId);
  }

  // Ask about whatever is due and affordable. Returns what settled.
  async tick() {
    const settled = new Map();
    const at = this._now();
    if (at < this._pausedUntil) return settled;

    const due = [...this._open.values()].filter((w) => w.due(at));
    due.sort((a, b) => a.started - b.started);

    for (const watched of due) {
      if (!this._budget.take()) break;
      watched.lastPoll = at;
      let status;
      try {
        status = await this.channel.status(watched.messageId);
      } catch (err) {
        if (err instanceof RateLimited) {
          this._pausedUntil = this._now() + err.retryAfter;
          break;
        }
        if (err instanceof UnknownMessage || err instanceof UnknownChannel) {
          // Nothing more is going to happen to a message the channel will not
          // talk about.
          this._open.delete(watched.messageId);
          continue;
        }
        // A blip is not an answer: leave the page open and try the next tick.
        continue;
      }

      this._seen.set(watched.messageId, status);
      if (status.done) {
        this._open.delete(watched.messageId);
        settled.set(watched.messageId, status);
      }
    }
    return settled;
  }

  // Tick until nothing is open or the timeout runs out. A timeout is not a
  // failure and anything still open stays watched, so calling run() again picks
  // up where this left off.
  async run(options = {}) {
    const interval = options.interval ?? 1;
    const deadline =
      options.timeout === undefined ? null : this._now() + options.timeout / 1000;
    const settled = new Map();
    while (this._open.size) {
      for (const [id, status] of await this.tick()) settled.set(id, status);
      if (this._open.size === 0) break;
      if (deadline !== null && this._now() >= deadline) break;
      await this._sleep(interval * 1000);
    }
    return settled;
  }
}

function fields(message) {
  const out = {};
  const put = (name, value, limit) => {
    const text = String(value);
    if (limit !== undefined && [...text].length > limit) {
      throw new Rejected(`Field '${name}' is longer than ${limit} characters`);
    }
    out[name] = text;
  };

  if (message.title !== undefined) put("title", message.title, MAX_TITLE_CHARS);
  if (message.body !== undefined) put("body", message.body, MAX_BODY_CHARS);

  if (message.priority !== undefined) {
    if (!PRIORITIES.includes(message.priority)) {
      throw new Rejected(`Field 'priority' must be one of ${PRIORITIES.join(", ")}`);
    }
    out.priority = message.priority;
  }

  if (message.url !== undefined) {
    put("url", message.url, MAX_URL_CHARS);
    if (!/^https?:\/\//.test(out.url)) {
      throw new Rejected("Field 'url' must start with http:// or https://");
    }
  }
  if (message.urlTitle !== undefined) put("url_title", message.urlTitle, MAX_TITLE_CHARS);
  if (message.callback !== undefined) {
    put("callback", message.callback, MAX_URL_CHARS);
    if (!out.callback.startsWith("https://")) {
      throw new Rejected("Field 'callback' must start with https://");
    }
  }

  if (message.key !== undefined) put("key", message.key, MAX_DEDUPE_KEY_CHARS);

  if (message.sound !== undefined) {
    const name = String(message.sound).replace(/\.caf$/, "");
    if (!SOUNDS.includes(name)) {
      // iOS plays its own tone for a sound it cannot find and reports nothing,
      // so a typo here is a page that silently loses its alarm.
      throw new Rejected(`Field 'sound' must be one of ${SOUNDS.join(", ")}`);
    }
    out.sound = String(message.sound);
  }

  if (message.retry !== undefined) out.retry = String(seconds("retry", message.retry, RETRY_RANGE));
  if (message.expire !== undefined) {
    out.expire = String(seconds("expire", message.expire, EXPIRE_RANGE));
  }

  if (message.ack) out.ack = "required";
  if (message.silent) {
    if (message.ack || out.priority === "page") {
      throw new Rejected("Field 'silent' cannot be combined with 'ack' or priority 'page'");
    }
    out.silent = "1";
  }

  if (message.timestamp !== undefined) out.timestamp = String(message.timestamp);
  if (message.resolve !== undefined) out.resolve = String(message.resolve);

  // No check for an empty set of fields: a send with nothing in it is a vital's
  // heartbeat.
  return out;
}

function seconds(name, value, [low, high]) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed)) {
    throw new Rejected(`Field '${name}' must be a whole number of seconds`);
  }
  // 0 turns the channel's default off, at every layer.
  if (parsed === 0) return 0;
  if (parsed < low || parsed > high) {
    throw new Rejected(`Field '${name}' must be 0 or between ${low} and ${high} seconds`);
  }
  return parsed;
}

async function readJson(response) {
  try {
    const payload = await response.json();
    return payload && typeof payload === "object" ? payload : {};
  } catch {
    // A proxy in front of a self-hosted edge can answer with something that is
    // not JSON at all.
    return {};
  }
}

// The human half of an error body. Mast nests it: {"error": {code, message}}.
// Reading payload.message finds nothing and falls back to the default, which
// turns a good key's "No such message." into "No such channel." and fails setup
// for every valid key.
function errorMessage(payload, fallback) {
  const error = payload?.error;
  if (error && typeof error === "object" && typeof error.message === "string") {
    return error.message;
  }
  if (typeof error === "string" && error) return error;
  if (typeof payload?.message === "string") return payload.message;
  return fallback;
}

function retryAfter(header) {
  const parsed = Number(header);
  return Number.isFinite(parsed) && parsed > 1 ? parsed : 1;
}

function defaultNow() {
  return Date.now() / 1000;
}

function defaultSleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
