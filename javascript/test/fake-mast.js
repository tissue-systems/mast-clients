// A stand-in for the Mast ingest surface: enough of it to test a client
// against, including the two different 404s.

import { createServer } from "node:http";

export const GOOD_KEY = "mk_" + "1c9f".repeat(10);
export const OTHER_KEY = "mk_" + "2b7d".repeat(10);
export const MESSAGE_ID = "mm_" + "7a3e".repeat(8);

export class FakeMast {
  constructor() {
    this.sends = [];
    this.polls = [];
    this.state = "queued";
    this.duplicate = false;
    this.sendStatus = 200;
    this.sendError = null;
    this.rateLimited = false;
    this.retryAfter = null;
    this.redirect = false;
    this.message = {
      id: MESSAGE_ID,
      state: "queued",
      received_at: "2026-09-17T10:00:00Z",
      dedupe_count: 0,
    };
    this._server = createServer((req, res) => this._handle(req, res));
  }

  async start() {
    await new Promise((resolve) => this._server.listen(0, "127.0.0.1", resolve));
    const { port } = this._server.address();
    this.origin = `http://127.0.0.1:${port}`;
    return this;
  }

  async stop() {
    this._server.closeAllConnections();
    await new Promise((resolve) => this._server.close(resolve));
  }

  url(key = GOOD_KEY) {
    return `${this.origin}/m/${key}`;
  }

  setMessage(patch) {
    Object.assign(this.message, patch);
  }

  get lastSend() {
    return this.sends[this.sends.length - 1];
  }

  async _handle(req, res) {
    const url = new URL(req.url, this.origin);
    // The edge rewrites "/" onto "/m/", so a client is free to use either.
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts[0] === "m") parts.shift();
    const [key, section, messageId] = parts;

    if (this.redirect) {
      res.writeHead(302, { location: "https://elsewhere.example/" });
      res.end();
      return;
    }
    if (this.rateLimited) {
      const headers = { "content-type": "application/json" };
      if (this.retryAfter !== null) headers["retry-after"] = String(this.retryAfter);
      res.writeHead(429, headers);
      res.end(error("rate_limited", "Too many requests. Slow down."));
      return;
    }
    if (key !== GOOD_KEY) {
      json(res, 404, error("not_found", "No such channel."));
      return;
    }

    if (req.method === "POST" && (section === undefined || section === "fail")) {
      const body = await read(req);
      this.sends.push({ path: url.pathname, fields: Object.fromEntries(new URLSearchParams(body)) });
      if (this.sendError) {
        json(res, this.sendStatus, error("invalid_field", this.sendError));
        return;
      }
      const payload = { id: MESSAGE_ID, state: this.state };
      if (this.duplicate) payload.duplicate = true;
      json(res, 200, JSON.stringify(payload));
      return;
    }

    if (req.method === "GET" && section === "messages") {
      this.polls.push(messageId);
      // The key is resolved before the id is, which is what makes an
      // impossible id a free key check.
      if (messageId !== this.message.id) {
        json(res, 404, error("not_found", "No such message."));
        return;
      }
      json(res, 200, JSON.stringify(this.message));
      return;
    }

    json(res, 404, error("not_found", "No such channel."));
  }
}

function error(code, message) {
  return JSON.stringify({ error: { code, message } });
}

function json(res, status, body) {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(body);
}

function read(req) {
  return new Promise((resolve) => {
    let body = "";
    req.on("data", (chunk) => {
      body += chunk;
    });
    req.on("end", () => resolve(body));
  });
}
