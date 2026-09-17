import assert from "node:assert/strict";
import { after, before, beforeEach, describe, test } from "node:test";

import { Channel } from "../src/mast.js";
import { FakeMast, MESSAGE_ID } from "./fake-mast.js";

let mast;
let channel;
let clock;

before(async () => {
  mast = await new FakeMast().start();
});

after(async () => {
  await mast.stop();
});

beforeEach(() => {
  mast.polls.length = 0;
  mast.message = {
    id: MESSAGE_ID,
    state: "queued",
    received_at: "2026-09-17T10:00:00Z",
    dedupe_count: 0,
  };
  channel = new Channel(mast.url());
  clock = { t: 0 };
});

const fake = {
  now: () => clock.t,
  sleep: async (ms) => {
    clock.t += ms / 1000;
  },
};

describe("wait", () => {
  test("comes back with the acknowledgement", async () => {
    mast.setMessage({ state: "acked", acked_at: "2026-09-17T10:00:09Z", acked_by: "denis" });
    const status = await channel.wait(MESSAGE_ID, fake);
    assert.ok(status.acknowledged);
    assert.equal(status.openFor, 9);
  });

  test("a timeout hands back the last thing it read", async () => {
    const status = await channel.wait(MESSAGE_ID, { ...fake, timeout: 10000 });
    assert.equal(status.state, "queued");
    assert.equal(status.acknowledged, false);
    assert.ok(mast.polls.length >= 2);
  });
});
