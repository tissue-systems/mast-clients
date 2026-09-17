import assert from "node:assert/strict";
import { after, before, beforeEach, describe, test } from "node:test";

import { Channel, MastError, RateLimited, Rejected, UnknownChannel } from "../src/mast.js";
import { FakeMast, GOOD_KEY, MESSAGE_ID, OTHER_KEY } from "./fake-mast.js";

let mast;
let channel;

before(async () => {
  mast = await new FakeMast().start();
});

after(async () => {
  await mast.stop();
});

beforeEach(() => {
  mast.sends.length = 0;
  mast.polls.length = 0;
  mast.state = "queued";
  mast.duplicate = false;
  mast.sendStatus = 200;
  mast.sendError = null;
  mast.rateLimited = false;
  mast.retryAfter = null;
  mast.redirect = false;
  channel = new Channel(mast.url());
});

describe("send", () => {
  test("posts the fields it was given", async () => {
    const sent = await channel.send({ title: "Disk", body: "92% on s10", priority: "loud" });
    assert.equal(sent.id, MESSAGE_ID);
    assert.equal(sent.state, "queued");
    assert.deepEqual(mast.lastSend.fields, {
      title: "Disk",
      body: "92% on s10",
      priority: "loud",
    });
  });

  test("sends nothing it was not given", async () => {
    await channel.send({ body: "just this" });
    assert.deepEqual(Object.keys(mast.lastSend.fields), ["body"]);
  });

  test("page() asks for the pager tier", async () => {
    await channel.page({ body: "s10 is down" });
    assert.equal(mast.lastSend.fields.priority, "page");
  });

  test("a heartbeat carries no fields at all", async () => {
    mast.state = "alive";
    const sent = await channel.ping();
    assert.equal(sent.state, "alive");
    assert.deepEqual(mast.lastSend.fields, {});
  });

  test("reports a fold against a dedupe key", async () => {
    mast.state = "deduped";
    mast.duplicate = true;
    const sent = await channel.send({ body: "flapping", key: "disk-s10" });
    assert.ok(sent.duplicate);
    assert.equal(mast.lastSend.fields.key, "disk-s10");
  });

  test("resolves by dedupe key", async () => {
    mast.state = "resolved";
    await channel.resolve({ key: "disk-s10", body: "back under 80%" });
    assert.deepEqual(mast.lastSend.fields, {
      body: "back under 80%",
      resolve: "1",
      key: "disk-s10",
    });
  });

  test("resolves by message id", async () => {
    await channel.resolve({ messageId: MESSAGE_ID });
    assert.deepEqual(mast.lastSend.fields, { resolve: MESSAGE_ID });
  });

  test("a resolve with nothing to close is refused here", async () => {
    await assert.rejects(() => channel.resolve({ body: "which one?" }), Rejected);
    assert.deepEqual(mast.sends, []);
  });

  test("a resolve will not take something that is not a message id", async () => {
    await assert.rejects(() => channel.resolve({ messageId: "mm_nope" }), Rejected);
    assert.deepEqual(mast.sends, []);
  });

  test("fail() posts to the vital's own route", async () => {
    mast.state = "queued";
    await channel.fail({ body: "no heartbeat" });
    assert.ok(mast.lastSend.path.endsWith("/fail"));
  });

  test("passes retry and expire through as seconds", async () => {
    await channel.page({ body: "up", retry: 60, expire: 3600 });
    assert.equal(mast.lastSend.fields.retry, "60");
    assert.equal(mast.lastSend.fields.expire, "3600");
  });

  test("zero turns the channel default off rather than being out of range", async () => {
    await channel.send({ body: "quiet", retry: 0, expire: 0 });
    assert.equal(mast.lastSend.fields.retry, "0");
    assert.equal(mast.lastSend.fields.expire, "0");
  });

  test("takes a sound with or without the .caf", async () => {
    await channel.send({ body: "a", sound: "klaxon" });
    assert.equal(mast.lastSend.fields.sound, "klaxon");
    await channel.send({ body: "b", sound: "klaxon.caf" });
    assert.equal(mast.lastSend.fields.sound, "klaxon.caf");
  });

  test("ack is sent as the word the server wants", async () => {
    await channel.send({ body: "confirm this", ack: true });
    assert.equal(mast.lastSend.fields.ack, "required");
  });

  test("counts characters, not bytes", async () => {
    const body = "é".repeat(4096);
    await channel.send({ body });
    assert.equal(mast.lastSend.fields.body.length, 4096);
  });
});

describe("refused before it costs a request", () => {
  const refuses = (name, message) =>
    test(name, async () => {
      await assert.rejects(() => channel.send(message), Rejected);
      assert.deepEqual(mast.sends, []);
    });

  refuses("a title over the cap", { title: "x".repeat(251) });
  refuses("a body over the cap", { body: "x".repeat(4097) });
  refuses("a url over the cap", { url: `https://e.example/${"x".repeat(512)}` });
  refuses("a dedupe key over the cap", { body: "a", key: "k".repeat(121) });
  refuses("a url that is not http", { body: "a", url: "ftp://files.example/x" });
  refuses("a callback that is not https", { body: "a", callback: "http://cb.example/x" });
  refuses("a priority Mast does not have", { body: "a", priority: "urgent" });
  refuses("a sound that would play as the default", { body: "a", sound: "buzzer" });
  refuses("a retry under the floor", { body: "a", retry: 29 });
  refuses("an expire over the ceiling", { body: "a", expire: 604801 });
  refuses("a retry that is not whole seconds", { body: "a", retry: 30.5 });
  refuses("silent with an ack, which cannot both be true", { body: "a", ack: true, silent: true });
  refuses("silent on a page", { body: "a", priority: "page", silent: true });

  test("a title exactly at the cap is fine", async () => {
    await channel.send({ title: "x".repeat(250) });
    assert.equal(mast.sends.length, 1);
  });
});

describe("what the server answers", () => {
  test("an unknown key is its own error", async () => {
    const other = new Channel(mast.url(OTHER_KEY));
    await assert.rejects(() => other.send({ body: "a" }), UnknownChannel);
  });

  test("keeps the reason out of the nested error body", async () => {
    mast.sendStatus = 400;
    mast.sendError = "Field 'title' is longer than 250 characters.";
    await assert.rejects(() => channel.send({ body: "a" }), {
      name: "Rejected",
      message: "Field 'title' is longer than 250 characters.",
    });
  });

  test("reads Retry-After off a 429", async () => {
    mast.rateLimited = true;
    mast.retryAfter = 7;
    await assert.rejects(() => channel.send({ body: "a" }), (err) => {
      assert.ok(err instanceof RateLimited);
      assert.equal(err.retryAfter, 7);
      return true;
    });
  });

  test("falls back to a second when Retry-After is missing", async () => {
    mast.rateLimited = true;
    await assert.rejects(() => channel.send({ body: "a" }), (err) => {
      assert.equal(err.retryAfter, 1);
      return true;
    });
  });

  test("an oversize request is a rejection, not an outage", async () => {
    mast.sendStatus = 413;
    mast.sendError = "Body too large.";
    await assert.rejects(() => channel.send({ body: "a" }), Rejected);
  });

  test("a 500 stays a MastError, because the send may still have landed", async () => {
    mast.sendStatus = 503;
    mast.sendError = "Upstream unavailable.";
    await assert.rejects(() => channel.send({ body: "a" }), (err) => {
      assert.ok(err instanceof MastError);
      assert.ok(!(err instanceof Rejected));
      return true;
    });
  });

  test("a redirect is not followed, so the key is not handed on", async () => {
    mast.redirect = true;
    await assert.rejects(() => channel.send({ body: "a" }), { name: "Unreachable" });
  });
});
