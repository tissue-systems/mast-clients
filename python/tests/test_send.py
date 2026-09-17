import unittest

from mast_pager import Channel, RateLimited, Rejected, UnknownChannel

from .fake_mast import MESSAGE_ID, OTHER_KEY, FakeMast


class SendTest(unittest.TestCase):
    def setUp(self):
        self.mast = FakeMast().start()
        self.addCleanup(self.mast.stop)
        self.channel = Channel(self.mast.url)

    def last(self):
        return self.mast.sends[-1]["fields"]

    def test_a_plain_send(self):
        sent = self.channel.send(title="db-01", body="replication stopped", priority="loud")
        self.assertEqual(sent.id, MESSAGE_ID)
        self.assertEqual(sent.state, "queued")
        self.assertFalse(sent.duplicate)
        self.assertEqual(
            self.last(),
            {"title": "db-01", "body": "replication stopped", "priority": "loud"},
        )

    def test_nothing_unset_is_sent(self):
        # A field left out has to stay out: an empty string is a value, and the
        # channel's own default is what an absent field means.
        self.channel.send(body="hello")
        self.assertEqual(list(self.last()), ["body"])

    def test_page_is_the_pager_tier(self):
        self.channel.page(title="promote?", key="release")
        self.assertEqual(self.last()["priority"], "page")
        self.assertEqual(self.last()["key"], "release")

    def test_a_heartbeat_carries_nothing(self):
        self.mast.state = "alive"
        sent = self.channel.ping()
        self.assertEqual(self.last(), {})
        self.assertEqual(sent.state, "alive")

    def test_a_fold_is_reported(self):
        self.mast.state = "deduped"
        self.mast.duplicate = True
        sent = self.channel.send(body="again", key="db-01")
        self.assertTrue(sent.duplicate)
        self.assertEqual(sent.state, "deduped")

    def test_resolve_by_key(self):
        self.channel.resolve(key="db-01", body="back up")
        self.assertEqual(self.last(), {"body": "back up", "resolve": "1", "key": "db-01"})

    def test_resolve_by_message_id(self):
        self.channel.resolve(message_id=MESSAGE_ID, body="back up")
        self.assertEqual(self.last()["resolve"], MESSAGE_ID)
        self.assertNotIn("key", self.last())

    def test_resolve_needs_something_to_close(self):
        with self.assertRaises(Rejected):
            self.channel.resolve(body="back up")

    def test_resolve_refuses_a_bad_message_id(self):
        with self.assertRaises(Rejected):
            self.channel.resolve(message_id="mm_nope", body="back up")

    def test_fail_takes_its_own_route(self):
        self.channel.fail(body="pg_dump exit 1")
        self.assertTrue(self.mast.sends[-1]["path"].endswith("/fail"))

    def test_retry_and_expire_are_seconds(self):
        self.channel.send(body="x", retry=60, expire=3600)
        self.assertEqual(self.last()["retry"], "60")
        self.assertEqual(self.last()["expire"], "3600")

    def test_zero_turns_a_channel_default_off(self):
        self.channel.send(body="x", retry=0, expire=0)
        self.assertEqual(self.last()["retry"], "0")
        self.assertEqual(self.last()["expire"], "0")

    def test_sound_takes_the_extension_or_not(self):
        self.channel.send(body="x", sound="klaxon")
        self.assertEqual(self.last()["sound"], "klaxon")
        self.channel.send(body="x", sound="pager.caf")
        self.assertEqual(self.last()["sound"], "pager.caf")

    def test_ack_required(self):
        self.channel.send(body="x", ack=True)
        self.assertEqual(self.last()["ack"], "required")

    def test_unicode_survives_the_form(self):
        self.channel.send(title="café-prod", body="数据库 unreachable")
        self.assertEqual(self.last()["title"], "café-prod")
        self.assertEqual(self.last()["body"], "数据库 unreachable")


class RefusedBeforeItCostsARequest(unittest.TestCase):
    """Caps the server already enforces, checked here so a doomed send is free."""

    def setUp(self):
        self.mast = FakeMast().start()
        self.addCleanup(self.mast.stop)
        self.channel = Channel(self.mast.url)

    def refuses(self, **kw):
        with self.assertRaises(Rejected):
            self.channel.send(**kw)
        self.assertEqual(self.mast.sends, [])

    def test_title_cap(self):
        self.refuses(title="x" * 251)

    def test_body_cap(self):
        self.refuses(body="x" * 4097)

    def test_dedupe_key_cap(self):
        self.refuses(body="x", key="k" * 121)

    def test_url_scheme(self):
        self.refuses(body="x", url="tissue://dashboard")

    def test_callback_must_be_https(self):
        self.refuses(body="x", callback="http://example.test/hook")

    def test_unknown_priority(self):
        self.refuses(body="x", priority="urgent")

    def test_unknown_sound(self):
        # iOS answers a sound it does not have by playing the default and
        # reporting nothing, so a typo has to be caught before it is sent.
        self.refuses(body="x", sound="klaxonn")

    def test_retry_out_of_range(self):
        self.refuses(body="x", retry=5)

    def test_expire_out_of_range(self):
        self.refuses(body="x", expire=604801)

    def test_silent_cannot_ask_for_an_ack(self):
        self.refuses(body="x", silent=True, ack=True)

    def test_silent_cannot_be_a_page(self):
        self.refuses(body="x", silent=True, priority="page")

    def test_a_title_at_the_cap_is_fine(self):
        self.channel.send(title="x" * 250)
        self.assertEqual(len(self.mast.sends), 1)


class ServerAnswers(unittest.TestCase):
    def setUp(self):
        self.mast = FakeMast().start()
        self.addCleanup(self.mast.stop)
        self.channel = Channel(self.mast.url)

    def test_an_unknown_key(self):
        channel = Channel("%s/%s" % (self.mast.origin, OTHER_KEY))
        with self.assertRaises(UnknownChannel):
            channel.send(body="x")

    def test_a_refused_field_keeps_the_servers_words(self):
        self.mast.send_error = (400, "validation_error", "Field 'url' must start with http://.")
        with self.assertRaises(Rejected) as caught:
            self.channel.send(body="x")
        # The message is one level down, inside "error". A client reading
        # payload["message"] gets nothing and reports the wrong thing.
        self.assertIn("Field 'url'", str(caught.exception))

    def test_rate_limited_carries_the_retry_after(self):
        self.mast.rate_limited = 1
        self.mast.retry_after = "7"
        with self.assertRaises(RateLimited) as caught:
            self.channel.send(body="x")
        self.assertEqual(caught.exception.retry_after, 7.0)

    def test_a_missing_retry_after_still_backs_off(self):
        self.mast.rate_limited = 1
        self.mast.retry_after = "later"
        with self.assertRaises(RateLimited) as caught:
            self.channel.send(body="x")
        self.assertEqual(caught.exception.retry_after, 1.0)

    def test_an_oversized_request(self):
        self.mast.send_error = (413, "payload_too_large", "Body over 16 KiB.")
        with self.assertRaises(Rejected):
            self.channel.send(body="x")


if __name__ == "__main__":
    unittest.main()
