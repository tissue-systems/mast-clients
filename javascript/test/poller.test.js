import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  POLL_BUDGET_PER_MIN,
  Poller,
  RateLimited,
  Rejected,
  Status,
  UnknownMessage,
} from "../src/mast.js";

const FIRST = "mm_" + "7a3e".repeat(8);
const SECOND = "mm_" + "91bc".repeat(8);

// Simulated seconds, so a ladder that spans ten minutes is tested in a
// millisecond and never depends on the machine being idle.
class Clock {
  constructor() {
    this.t = 1000;
  }

  now = () => this.t;

  sleep = async (ms) => {
    this.t += ms / 1000;
  };
}

class StubChannel {
  constructor(clock) {
    this.clock = clock;
    this.asks = [];
    this.answer = () => new Status({ state: "queued" });
  }

  async status(messageId) {
    this.asks.push({ at: this.clock.t, messageId });
    return this.answer(messageId);
  }
}

function setup(options = {}) {
  const clock = new Clock();
  const channel = new StubChannel(clock);
  const poller = new Poller(channel, { now: clock.now, sleep: clock.sleep, ...options });
  return { clock, channel, poller };
}

// Tick once per simulated second and report the seconds that produced a poll.
async function pollsOver(clock, channel, poller, seconds) {
  const at = [];
  for (let i = 0; i < seconds; i += 1) {
    const before = channel.asks.length;
    await poller.tick();
    if (channel.asks.length > before) at.push(clock.t);
    clock.t += 1;
  }
  return at;
}

describe("the ladder", () => {
  test("asks every three seconds while the page is new", async () => {
    const { clock, channel, poller } = setup();
    poller.watch(FIRST);
    const at = await pollsOver(clock, channel, poller, 30);
    assert.deepEqual(gaps(at), Array(at.length - 1).fill(3));
  });

  test("drops to ten seconds after half a minute", async () => {
    const { clock, channel, poller } = setup();
    poller.watch(FIRST);
    const at = await pollsOver(clock, channel, poller, 120);
    const late = at.filter((t) => t - 1000 > 40);
    assert.deepEqual(gaps(late), Array(late.length - 1).fill(10));
  });

  test("drops to half a minute once the page is five minutes old", async () => {
    const { clock, channel, poller } = setup({ budgetPerMinute: 1000 });
    poller.watch(FIRST);
    const at = await pollsOver(clock, channel, poller, 500);
    const late = at.filter((t) => t - 1000 > 320);
    assert.deepEqual(gaps(late), Array(late.length - 1).fill(30));
  });
});

describe("the budget", () => {
  test("twenty open pages still spend at most half the key's allowance", async () => {
    const { clock, channel, poller } = setup();
    for (let i = 0; i < 20; i += 1) poller.watch(messageId(i));
    await pollsOver(clock, channel, poller, 60);
    assert.ok(
      channel.asks.length <= POLL_BUDGET_PER_MIN,
      `spent ${channel.asks.length} requests in a minute`,
    );
  });

  test("the window rolls, so the next minute is not starved", async () => {
    const { clock, channel, poller } = setup();
    for (let i = 0; i < 20; i += 1) poller.watch(messageId(i));
    await pollsOver(clock, channel, poller, 121);
    assert.ok(channel.asks.length > POLL_BUDGET_PER_MIN);
    assert.ok(channel.asks.length <= POLL_BUDGET_PER_MIN * 3);
  });

  test("the oldest page gets the last request", async () => {
    const { clock, channel, poller } = setup({ budgetPerMinute: 1 });
    poller.watch(FIRST);
    clock.t += 5;
    poller.watch(SECOND);
    await poller.tick();
    assert.deepEqual(channel.asks.map((a) => a.messageId), [FIRST]);
  });

  test("both get asked about when there is room for both", async () => {
    const { clock, channel, poller } = setup();
    poller.watch(FIRST);
    clock.t += 5;
    poller.watch(SECOND);
    await poller.tick();
    assert.deepEqual(channel.asks.map((a) => a.messageId), [FIRST, SECOND]);
  });
});

describe("backing off", () => {
  test("a 429 stops the whole channel, not the one page", async () => {
    const { clock, channel, poller } = setup();
    poller.watch(FIRST);
    poller.watch(SECOND);
    channel.answer = () => {
      throw new RateLimited(20);
    };
    await poller.tick();
    assert.equal(channel.asks.length, 1);

    channel.answer = () => new Status({ state: "queued" });
    clock.t += 5;
    await poller.tick();
    assert.equal(channel.asks.length, 1);

    clock.t += 20;
    await poller.tick();
    assert.equal(channel.asks.length, 3);
  });

  test("a blip leaves the page open", async () => {
    const { clock, channel, poller } = setup();
    poller.watch(FIRST);
    channel.answer = () => {
      throw new Error("connection reset");
    };
    await poller.tick();
    assert.equal(poller.openCount, 1);

    channel.answer = () => new Status({ state: "acked" });
    clock.t += 5;
    await poller.tick();
    assert.equal(poller.openCount, 0);
  });

  test("a message the channel disowns is dropped", async () => {
    const { channel, poller } = setup();
    poller.watch(FIRST);
    channel.answer = () => {
      throw new UnknownMessage("No such message.");
    };
    await poller.tick();
    assert.equal(poller.openCount, 0);
  });
});

describe("settling", () => {
  test("an ack takes the page off the list", async () => {
    const { channel, poller } = setup();
    poller.watch(FIRST);
    channel.answer = () =>
      new Status({
        state: "acked",
        received_at: "2026-09-17T10:00:00Z",
        acked_at: "2026-09-17T10:00:12Z",
        acked_by: "denis",
      });
    const settled = await poller.tick();
    assert.equal(poller.openCount, 0);
    assert.equal(settled.get(FIRST).ackedBy, "denis");
    assert.equal(settled.get(FIRST).openFor, 12);
  });

  test("run returns once nothing is open", async () => {
    const { channel, poller } = setup();
    poller.watch(FIRST);
    channel.answer = () => new Status({ state: "resolved" });
    const settled = await poller.run();
    assert.deepEqual([...settled.keys()], [FIRST]);
  });

  test("a timeout leaves the page open rather than failing it", async () => {
    const { poller } = setup();
    poller.watch(FIRST);
    const settled = await poller.run({ timeout: 5000 });
    assert.equal(settled.size, 0);
    assert.equal(poller.openCount, 1);
    assert.equal(poller.lastStatus(FIRST).state, "queued");
  });

  test("watching the same page twice watches one page", () => {
    const { poller } = setup();
    poller.watch(FIRST);
    poller.watch(FIRST);
    assert.equal(poller.openCount, 1);
  });

  test("it will not watch something that is not a message id", () => {
    const { poller } = setup();
    assert.throws(() => poller.watch("mm_nope"), Rejected);
  });
});

function gaps(values) {
  return values.slice(1).map((value, i) => value - values[i]);
}

function messageId(n) {
  return "mm_" + String(n).padStart(4, "0").repeat(8);
}
