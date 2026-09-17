import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { Channel, parseChannelUrl } from "../src/mast.js";
import { GOOD_KEY } from "./fake-mast.js";

describe("parseChannelUrl", () => {
  test("takes the short form the app copies", () => {
    const { origin, prefix, key } = parseChannelUrl(`https://mast.tissue.dev/${GOOD_KEY}`);
    assert.equal(origin, "https://mast.tissue.dev");
    assert.equal(prefix, "");
    assert.equal(key, GOOD_KEY);
  });

  test("takes the /m/ form and keeps the prefix", () => {
    const parsed = parseChannelUrl(`https://mast.tissue.dev/m/${GOOD_KEY}`);
    assert.equal(parsed.prefix, "/m");
    assert.equal(parsed.key, GOOD_KEY);
  });

  test("takes a bare key", () => {
    const parsed = parseChannelUrl(GOOD_KEY);
    assert.equal(parsed.origin, "https://mast.tissue.dev");
    assert.equal(parsed.key, GOOD_KEY);
  });

  test("assumes https for a host with no scheme", () => {
    const parsed = parseChannelUrl(`mast.example.com/m/${GOOD_KEY}`);
    assert.equal(parsed.origin, "https://mast.example.com");
  });

  test("keeps a port", () => {
    const parsed = parseChannelUrl(`http://127.0.0.1:8899/m/${GOOD_KEY}`);
    assert.equal(parsed.origin, "http://127.0.0.1:8899");
  });

  test("ignores whitespace around a pasted URL", () => {
    const parsed = parseChannelUrl(`  https://mast.tissue.dev/${GOOD_KEY}\n`);
    assert.equal(parsed.key, GOOD_KEY);
  });

  test("ignores a trailing slash", () => {
    const parsed = parseChannelUrl(`https://mast.tissue.dev/${GOOD_KEY}/`);
    assert.equal(parsed.key, GOOD_KEY);
  });

  test("rejects nothing", () => {
    assert.throws(() => parseChannelUrl("   "), TypeError);
  });

  test("rejects a URL with no key in it", () => {
    assert.throws(() => parseChannelUrl("https://mast.tissue.dev/m/"), TypeError);
  });

  test("names the key when the shape is wrong", () => {
    assert.throws(() => parseChannelUrl("https://mast.tissue.dev/mk_short"), {
      message: /not a channel key/,
    });
  });

  test("rejects uppercase hex, which is not what Mast mints", () => {
    assert.throws(() => parseChannelUrl(`mk_${"1C9F".repeat(10)}`), TypeError);
  });

  test("rejects a scheme that is not http", () => {
    assert.throws(() => parseChannelUrl(`ftp://mast.tissue.dev/${GOOD_KEY}`), TypeError);
  });
});

describe("Channel", () => {
  test("rebuilds the URL it was given", () => {
    const channel = new Channel(`https://mast.tissue.dev/m/${GOOD_KEY}`);
    assert.equal(channel.url, `https://mast.tissue.dev/m/${GOOD_KEY}`);
  });

  test("does not put the whole key in its description", () => {
    const text = String(new Channel(GOOD_KEY));
    assert.ok(!text.includes(GOOD_KEY));
    assert.ok(text.includes("mk_1c9f"));
  });
});
