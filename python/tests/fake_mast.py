"""A stand-in for the mast ingest routes, good enough to test a client against.

It answers the three routes a channel key reaches and keeps what it was sent so
a test can assert on the wire, which is the half that matters: every trap this
library exists for is in the shape of a request or a 404.
"""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

GOOD_KEY = "mk_" + "1c9f" * 10
OTHER_KEY = "mk_" + "2b7d" * 10
MESSAGE_ID = "mm_" + "7a3e" * 8


class FakeMast:
    def __init__(self):
        self.sends = []
        self.polls = []
        self.messages = {}
        self.state = "queued"
        self.duplicate = False
        self.send_status = 202
        self.send_error = None
        self.rate_limited = 0  # number of answers to burn on a 429
        self.retry_after = "1"
        self._server = None
        self._thread = None

    def start(self):
        handler = _handler_for(self)
        self._server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        # The default poll interval makes every shutdown() take half a second,
        # which is most of a run once there is a server per test.
        self._thread = threading.Thread(
            target=self._server.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True
        )
        self._thread.start()
        return self

    def stop(self):
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)

    @property
    def origin(self):
        host, port = self._server.server_address[:2]
        return "http://%s:%d" % (host, port)

    @property
    def url(self):
        return "%s/%s" % (self.origin, GOOD_KEY)

    def set_message(self, message_id, **fields):
        record = {
            "id": message_id,
            "state": "queued",
            "received_at": "2026-08-19T03:17:44.902Z",
            "dedupe_count": 0,
            "acked_at": None,
            "acked_by": None,
            "resolved_at": None,
            "expires_at": None,
        }
        record.update(fields)
        self.messages[message_id] = record
        return record


def _handler_for(fake):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.0"

        def log_message(self, *args):
            pass

        def do_POST(self):
            path, key, rest = _split(self.path)
            if key not in (GOOD_KEY,):
                return self._error(404, "not_found", "No such channel.")
            if fake.rate_limited > 0:
                fake.rate_limited -= 1
                return self._error(
                    429, "rate_limited", "Too many sends to this channel.",
                    headers={"Retry-After": fake.retry_after},
                )

            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length).decode("utf-8")
            fields = {k: v[0] for k, v in parse_qs(raw, keep_blank_values=True).items()}
            fake.sends.append({"path": path, "fields": fields})

            if fake.send_error is not None:
                status, code, message = fake.send_error
                return self._error(status, code, message)

            body = {"ok": True, "id": MESSAGE_ID, "state": fake.state}
            if fake.duplicate:
                body["duplicate"] = True
            return self._json(fake.send_status, body)

        def do_GET(self):
            path, key, rest = _split(self.path)
            if key not in (GOOD_KEY,):
                return self._error(404, "not_found", "No such channel.")
            if fake.rate_limited > 0:
                fake.rate_limited -= 1
                return self._error(
                    429, "rate_limited", "Too many sends to this channel.",
                    headers={"Retry-After": fake.retry_after},
                )
            if len(rest) != 2 or rest[0] != "messages":
                return self._error(404, "not_found", "No such message.")
            fake.polls.append(rest[1])
            record = fake.messages.get(rest[1])
            if record is None:
                return self._error(404, "not_found", "No such message.")
            return self._json(200, record)

        def _json(self, status, payload, headers=None):
            raw = (json.dumps(payload) + "\n").encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            for name, value in (headers or {}).items():
                self.send_header(name, value)
            self.end_headers()
            self.wfile.write(raw)

        def _error(self, status, code, message, headers=None):
            self._json(status, {"error": {"code": code, "message": message}}, headers)

    return Handler


def _split(raw_path):
    path = urlsplit(raw_path).path
    segments = [s for s in path.split("/") if s]
    if segments and segments[0] == "m":
        segments = segments[1:]
    if not segments:
        return path, "", []
    return path, segments[0], segments[1:]
