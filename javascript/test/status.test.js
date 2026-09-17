import assert from "node:assert/strict";
import { after, before, beforeEach, describe, test } from "node:test";

import { Channel, Rejected, Unreachable, UnknownChannel, UnknownMessage } from "../src/mast.js";
import { FakeMast, MESSAGE_ID, OTHER_KEY } from "./fake-mast.js";

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
  mast.rateLimited = false;
  mast.redirect = false;
  mast.message = {
    id: MESSAGE_ID,
    state: "queued",
    received_at: "2026-09-17T10:00:00Z",
    dedupe_count: 0,
  };
  channel = new Channel(mast.url());
});

describe("check", () => {
  test("says yes to a key that works", async () => {
    assert.equal(await channel.check(), true);
  });

  test("sends nothing while doing it", async () => {
    await channel.check();
    assert.deepEqual(mast.sends, []);
    assert.deepEqual(mast.polls, ["check"]);
  });

  test("says no to a key that does not", async () => {
    const other = new Channel(mast.url(OTHER_KEY));
    await assert.rejects(() => other.check(), UnknownChannel);
  });

  test("the two 404s are what tells them apart", async () => {
    const other = new Channel(mast.url(OTHER_KEY));
    await assert.rejects(() => other.status(MESSAGE_ID), {
      name: "UnknownChannel",
      message: "No such channel.",
    });
    mast.setMessage({ id: "mm_" + "0000".repeat(8) });
    await assert.rejects(() => channel.status(MESSAGE_ID), {
      name: "UnknownMessage",
      message: "No such message.",
    });
  });
});

describe("status", () => {
  test("reads an open page", async () => {
    const status = await channel.status(MESSAGE_ID);
    assert.equal(status.state, "queued");
    assert.equal(status.acknowledged, false);
    assert.equal(status.done, false);
  });

  test("works out how long a page stayed open", async () => {
    mast.setMessage({
      state: "acked",
      acked_at: "2026-09-17T10:01:17Z",
      acked_by: "denis",
    });
    const status = await channel.status(MESSAGE_ID);
    assert.ok(status.acknowledged);
    assert.ok(status.done);
    assert.equal(status.ackedBy, "denis");
    assert.equal(status.openFor, 77);
  });

  test("a missing stamp reads as unknown, not as instant", async () => {
    mast.setMessage({ state: "acked", acked_at: null });
    const status = await channel.status(MESSAGE_ID);
    assert.equal(status.openFor, null);
  });

  test("resolved and expired are both done", async () => {
    for (const state of ["resolved", "expired"]) {
      mast.setMessage({ state });
      const status = await channel.status(MESSAGE_ID);
      assert.ok(status.done, state);
      assert.equal(status.acknowledged, false);
    }
  });

  test("carries the fold count", async () => {
    mast.setMessage({ state: "queued", dedupe_count: 47 });
    const status = await channel.status(MESSAGE_ID);
    assert.equal(status.dedupeCount, 47);
  });

  test("another channel's message id is not this channel's", async () => {
    mast.setMessage({ id: "mm_" + "beef".repeat(8) });
    await assert.rejects(() => channel.status(MESSAGE_ID), UnknownMessage);
  });

  test("something that is not a message id never leaves the process", async () => {
    await assert.rejects(() => channel.status("nope"), Rejected);
    assert.deepEqual(mast.polls, []);
  });
});

describe("transport", () => {
  test("a host that is not listening is unreachable, not a bad key", async () => {
    const dead = new Channel(`http://127.0.0.1:1/m/${channel.key}`, { timeout: 2000 });
    await assert.rejects(() => dead.send({ body: "a" }), Unreachable);
  });
});
