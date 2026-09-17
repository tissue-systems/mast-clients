import unittest

from mast_pager import Channel, MastError, Rejected, UnknownChannel, UnknownMessage

from .fake_mast import MESSAGE_ID, OTHER_KEY, FakeMast


class KeyCheck(unittest.TestCase):
    """The two 404s: "No such channel." for a bad key, "No such message." for a
    good one. Telling them apart is what lets setup prove a key without paging
    anybody."""

    def setUp(self):
        self.mast = FakeMast().start()
        self.addCleanup(self.mast.stop)

    def test_a_good_key(self):
        self.assertTrue(Channel(self.mast.url).check())

    def test_a_good_key_stores_nothing(self):
        Channel(self.mast.url).check()
        self.assertEqual(self.mast.sends, [])

    def test_a_bad_key(self):
        channel = Channel("%s/%s" % (self.mast.origin, OTHER_KEY))
        with self.assertRaises(UnknownChannel):
            channel.check()

    def test_the_two_are_told_apart_by_the_nested_message(self):
        # Reading payload["message"] instead of payload["error"]["message"]
        # finds nothing, falls back to the default, and calls a good key bad.
        channel = Channel(self.mast.url)
        with self.assertRaises(UnknownMessage):
            channel.status(MESSAGE_ID)


class StatusTest(unittest.TestCase):
    def setUp(self):
        self.mast = FakeMast().start()
        self.addCleanup(self.mast.stop)
        self.channel = Channel(self.mast.url)

    def test_an_open_page(self):
        self.mast.set_message(MESSAGE_ID, state="queued")
        status = self.channel.status(MESSAGE_ID)
        self.assertEqual(status.state, "queued")
        self.assertFalse(status.acknowledged)
        self.assertFalse(status.done)
        self.assertIsNone(status.open_for)

    def test_an_acknowledged_page(self):
        self.mast.set_message(
            MESSAGE_ID,
            state="acked",
            received_at="2026-08-19T03:17:44.902Z",
            acked_at="2026-08-19T03:19:02.117Z",
            acked_by="iPhone",
        )
        status = self.channel.status(MESSAGE_ID)
        self.assertTrue(status.acknowledged)
        self.assertTrue(status.done)
        self.assertEqual(status.acked_by, "iPhone")
        self.assertEqual(status.open_for, 77)

    def test_a_missing_stamp_is_not_a_zero(self):
        self.mast.set_message(MESSAGE_ID, state="acked", acked_at=None)
        self.assertIsNone(self.channel.status(MESSAGE_ID).open_for)

    def test_resolved_counts_as_done(self):
        self.mast.set_message(MESSAGE_ID, state="resolved")
        self.assertTrue(self.channel.status(MESSAGE_ID).done)

    def test_expired_counts_as_done(self):
        self.mast.set_message(MESSAGE_ID, state="expired")
        self.assertTrue(self.channel.status(MESSAGE_ID).done)

    def test_a_dedupe_count_comes_back(self):
        self.mast.set_message(MESSAGE_ID, state="queued", dedupe_count=47)
        self.assertEqual(self.channel.status(MESSAGE_ID).dedupe_count, 47)

    def test_a_message_from_another_channel(self):
        with self.assertRaises(UnknownMessage):
            self.channel.status("mm_" + "0" * 32)

    def test_something_that_is_not_a_message_id(self):
        with self.assertRaises(Rejected):
            self.channel.status("42")
        self.assertEqual(self.mast.polls, [])


class Transport(unittest.TestCase):
    def test_a_dead_host(self):
        channel = Channel("http://127.0.0.1:1/" + "mk_" + "1c9f" * 10)
        with self.assertRaises(MastError):
            channel.send(body="x")


if __name__ == "__main__":
    unittest.main()
