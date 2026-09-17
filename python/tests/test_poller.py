import unittest

from mast_pager import MastError, Poller, RateLimited, Status, UnknownMessage
from mast_pager.client import POLL_BUDGET_PER_MIN

FIRST = "mm_" + "a" * 32
SECOND = "mm_" + "b" * 32


class Clock:
    def __init__(self):
        self.t = 1000.0

    def __call__(self):
        return self.t

    def advance(self, seconds):
        self.t += seconds


class StubChannel:
    """Answers whatever the test queued, and counts what was asked."""

    def __init__(self):
        self.asked = []
        self.answers = {}
        self.raise_next = None

    def status(self, message_id):
        self.asked.append(message_id)
        if self.raise_next is not None:
            err, self.raise_next = self.raise_next, None
            raise err
        return Status(self.answers.get(message_id, {"id": message_id, "state": "queued"}))


class Ladder(unittest.TestCase):
    """Three rungs: every 3s for the first half minute, every 10s to five
    minutes, every 30s after that. A page nobody is going to answer costs less
    the longer it stays open."""

    def setUp(self):
        self.clock = Clock()
        self.channel = StubChannel()
        self.poller = Poller(self.channel, clock=self.clock, sleep=lambda s: None)
        self.poller.watch(FIRST)

    def test_the_first_tick_asks(self):
        self.poller.tick()
        self.assertEqual(self.channel.asked, [FIRST])

    def test_a_young_page_is_asked_about_every_three_seconds(self):
        self.poller.tick()
        self.clock.advance(2)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 1)
        self.clock.advance(1)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 2)

    def test_past_half_a_minute_it_slows_to_ten_seconds(self):
        self.clock.advance(31)
        self.poller.tick()
        self.clock.advance(9)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 1)
        self.clock.advance(1)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 2)

    def test_past_five_minutes_it_slows_to_thirty(self):
        self.clock.advance(301)
        self.poller.tick()
        self.clock.advance(29)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 1)
        self.clock.advance(1)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 2)


class Budget(unittest.TestCase):
    """Half the key's 60 a minute, so polling cannot starve the sends."""

    def setUp(self):
        self.clock = Clock()
        self.channel = StubChannel()
        self.poller = Poller(self.channel, clock=self.clock, sleep=lambda s: None)

    def test_a_minute_of_polling_stays_inside_the_budget(self):
        # Twenty open pages on a three second rung would be four hundred
        # requests a minute, seven times the key's whole allowance.
        for n in range(20):
            self.poller.watch("mm_%032x" % n)
        for _ in range(60):
            self.poller.tick()
            self.clock.advance(1)
        self.assertLessEqual(len(self.channel.asked), POLL_BUDGET_PER_MIN)

    def test_the_window_rolls(self):
        poller = Poller(self.channel, budget_per_min=1, clock=self.clock,
                        sleep=lambda s: None)
        poller.watch(FIRST)
        poller.tick()
        self.clock.advance(30)
        poller.tick()
        self.assertEqual(len(self.channel.asked), 1)
        self.clock.advance(31)
        poller.tick()
        self.assertEqual(len(self.channel.asked), 2)

    def test_the_oldest_page_gets_the_last_token(self):
        # Two pages, one token. The one that has been waiting longest is the
        # one somebody is still standing over.
        poller = Poller(self.channel, budget_per_min=1, clock=self.clock,
                        sleep=lambda s: None)
        poller.watch(FIRST)
        self.clock.advance(1)
        poller.watch(SECOND)
        poller.tick()
        self.assertEqual(self.channel.asked, [FIRST])

    def test_both_get_asked_about_once_there_is_room(self):
        self.poller.watch(FIRST)
        self.poller.watch(SECOND)
        self.poller.tick()
        self.assertEqual(sorted(self.channel.asked), sorted([FIRST, SECOND]))


class Backoff(unittest.TestCase):
    def setUp(self):
        self.clock = Clock()
        self.channel = StubChannel()
        self.poller = Poller(self.channel, clock=self.clock, sleep=lambda s: None)

    def test_a_429_pauses_the_whole_channel(self):
        # The limit belongs to the key, so backing off one page and hammering
        # on with the next would spend the pause on another 429.
        self.poller.watch(FIRST)
        self.poller.watch(SECOND)
        self.channel.raise_next = RateLimited(30.0)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 1)

        self.clock.advance(29)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 1)

        self.clock.advance(2)
        self.poller.tick()
        self.assertEqual(len(self.channel.asked), 3)

    def test_a_blip_leaves_the_page_open(self):
        self.poller.watch(FIRST)
        self.channel.raise_next = MastError("connection reset")
        self.poller.tick()
        self.assertEqual(self.poller.open_count, 1)

    def test_a_message_the_channel_disowns_is_dropped(self):
        self.poller.watch(FIRST)
        self.channel.raise_next = UnknownMessage("No such message.")
        self.poller.tick()
        self.assertEqual(self.poller.open_count, 0)


class Settling(unittest.TestCase):
    def setUp(self):
        self.clock = Clock()
        self.channel = StubChannel()
        self.poller = Poller(self.channel, clock=self.clock, sleep=lambda s: None)

    def test_an_ack_settles_the_page(self):
        self.poller.watch(FIRST)
        self.channel.answers[FIRST] = {"id": FIRST, "state": "acked", "acked_by": "iPhone"}
        settled = self.poller.tick()
        self.assertEqual(settled[FIRST].acked_by, "iPhone")
        self.assertEqual(self.poller.open_count, 0)

    def test_run_returns_when_everything_has_settled(self):
        self.poller.watch(FIRST)
        self.channel.answers[FIRST] = {"id": FIRST, "state": "resolved"}
        settled = self.poller.run()
        self.assertEqual(list(settled), [FIRST])

    def test_a_timeout_leaves_the_page_watched(self):
        # Giving up waiting is not the same as the page being over: whoever was
        # paged can still answer it, and something else may be waiting on that.
        self.poller.watch(FIRST)
        sleeps = []

        def sleep(seconds):
            sleeps.append(seconds)
            self.clock.advance(seconds)

        poller = Poller(self.channel, clock=self.clock, sleep=sleep)
        poller.watch(FIRST)
        settled = poller.run(timeout=5)
        self.assertEqual(settled, {})
        self.assertEqual(poller.open_count, 1)
        self.assertTrue(sleeps)

    def test_watching_the_same_page_twice_is_one_page(self):
        self.poller.watch(FIRST)
        self.poller.watch(FIRST)
        self.assertEqual(self.poller.open_count, 1)


if __name__ == "__main__":
    unittest.main()
