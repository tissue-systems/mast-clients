package mast

import (
	"context"
	"errors"
	"fmt"
	"testing"
	"time"
)

// Simulated seconds, so a ladder that spans ten minutes is tested in a
// millisecond and never depends on the machine being idle.
type fakeClock struct {
	at time.Time
}

func (c *fakeClock) now() time.Time { return c.at }

func (c *fakeClock) sleep(ctx context.Context, d time.Duration) error {
	c.at = c.at.Add(d)
	return nil
}

type stubChannel struct {
	clock  *fakeClock
	asks   []ask
	answer func(messageID string) (Status, error)
}

type ask struct {
	at        time.Time
	messageID string
}

func (s *stubChannel) Status(ctx context.Context, messageID string) (Status, error) {
	s.asks = append(s.asks, ask{at: s.clock.at, messageID: messageID})
	if s.answer == nil {
		return Status{State: "queued"}, nil
	}
	return s.answer(messageID)
}

func stubbed(t *testing.T) (*fakeClock, *stubChannel, *Poller) {
	t.Helper()
	clock := &fakeClock{at: time.Date(2026, 9, 17, 10, 0, 0, 0, time.UTC)}
	channel := &stubChannel{clock: clock}
	poller := &Poller{
		Budget:  PollBudgetPerMinute,
		channel: channel,
		open:    map[string]*watched{},
		last:    map[string]Status{},
		now:     clock.now,
		sleep:   clock.sleep,
	}
	return clock, channel, poller
}

// pollsOver ticks once per simulated second and reports the seconds that
// produced a poll.
func pollsOver(t *testing.T, clock *fakeClock, channel *stubChannel, poller *Poller, seconds int) []time.Time {
	t.Helper()
	var at []time.Time
	for i := 0; i < seconds; i++ {
		before := len(channel.asks)
		if _, err := poller.Tick(context.Background()); err != nil {
			t.Fatalf("Tick: %v", err)
		}
		if len(channel.asks) > before {
			at = append(at, clock.at)
		}
		clock.at = clock.at.Add(time.Second)
	}
	return at
}

func TestTheLadderSlowsDownAsThePageAges(t *testing.T) {
	cases := []struct {
		name    string
		seconds int
		after   time.Duration
		every   time.Duration
	}{
		{"three seconds while it is new", 30, 0, 3 * time.Second},
		{"ten seconds after half a minute", 120, 40 * time.Second, 10 * time.Second},
		{"half a minute after five", 500, 320 * time.Second, 30 * time.Second},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			clock, channel, poller := stubbed(t)
			poller.Budget = 1000
			started := clock.at
			if err := poller.Watch(messageID); err != nil {
				t.Fatal(err)
			}

			var late []time.Time
			for _, at := range pollsOver(t, clock, channel, poller, tc.seconds) {
				if at.Sub(started) > tc.after {
					late = append(late, at)
				}
			}
			if len(late) < 2 {
				t.Fatalf("only %d poll(s) in that band", len(late))
			}
			for i := 1; i < len(late); i++ {
				if gap := late[i].Sub(late[i-1]); gap != tc.every {
					t.Errorf("gap %d = %s, want %s", i, gap, tc.every)
				}
			}
		})
	}
}

func TestTwentyOpenPagesStillSpendHalfTheAllowance(t *testing.T) {
	clock, channel, poller := stubbed(t)
	for i := 0; i < 20; i++ {
		if err := poller.Watch(nthMessageID(i)); err != nil {
			t.Fatal(err)
		}
	}

	pollsOver(t, clock, channel, poller, 60)
	if len(channel.asks) > PollBudgetPerMinute {
		t.Errorf("spent %d requests in a minute, the whole key only gets %d", len(channel.asks), RateLimitPerMinute)
	}
}

func TestTheWindowRollsSoTheNextMinuteIsNotStarved(t *testing.T) {
	clock, channel, poller := stubbed(t)
	for i := 0; i < 20; i++ {
		if err := poller.Watch(nthMessageID(i)); err != nil {
			t.Fatal(err)
		}
	}

	pollsOver(t, clock, channel, poller, 121)
	if len(channel.asks) <= PollBudgetPerMinute {
		t.Errorf("spent %d requests in two minutes, so the window never reopened", len(channel.asks))
	}
	if len(channel.asks) > 3*PollBudgetPerMinute {
		t.Errorf("spent %d requests in two minutes", len(channel.asks))
	}
}

func TestTheOldestPageGetsTheLastRequest(t *testing.T) {
	clock, channel, poller := stubbed(t)
	poller.Budget = 1

	first, second := nthMessageID(1), nthMessageID(2)
	if err := poller.Watch(first); err != nil {
		t.Fatal(err)
	}
	clock.at = clock.at.Add(5 * time.Second)
	if err := poller.Watch(second); err != nil {
		t.Fatal(err)
	}

	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(channel.asks) != 1 || channel.asks[0].messageID != first {
		t.Errorf("asked about %v, want only the older page", channel.asks)
	}
}

func TestBothGetAskedAboutWhenThereIsRoom(t *testing.T) {
	clock, channel, poller := stubbed(t)

	first, second := nthMessageID(1), nthMessageID(2)
	if err := poller.Watch(first); err != nil {
		t.Fatal(err)
	}
	clock.at = clock.at.Add(5 * time.Second)
	if err := poller.Watch(second); err != nil {
		t.Fatal(err)
	}

	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(channel.asks) != 2 {
		t.Fatalf("asked about %d page(s), want 2", len(channel.asks))
	}
	if channel.asks[0].messageID != first {
		t.Errorf("asked about %s first, want the older page", channel.asks[0].messageID)
	}
}

func TestA429StopsTheWholeChannel(t *testing.T) {
	clock, channel, poller := stubbed(t)
	if err := poller.Watch(nthMessageID(1)); err != nil {
		t.Fatal(err)
	}
	if err := poller.Watch(nthMessageID(2)); err != nil {
		t.Fatal(err)
	}

	channel.answer = func(string) (Status, error) {
		return Status{}, &RateLimited{RetryAfter: 20 * time.Second}
	}
	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(channel.asks) != 1 {
		t.Fatalf("asked %d times, want to have stopped at the first 429", len(channel.asks))
	}

	channel.answer = nil
	clock.at = clock.at.Add(5 * time.Second)
	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(channel.asks) != 1 {
		t.Fatalf("asked %d times, want the channel still paused", len(channel.asks))
	}

	clock.at = clock.at.Add(20 * time.Second)
	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(channel.asks) != 3 {
		t.Fatalf("asked %d times, want both pages once the pause is over", len(channel.asks))
	}
}

func TestABlipLeavesThePageOpen(t *testing.T) {
	clock, channel, poller := stubbed(t)
	if err := poller.Watch(messageID); err != nil {
		t.Fatal(err)
	}

	channel.answer = func(string) (Status, error) {
		return Status{}, fmt.Errorf("%w: connection reset", ErrUnreachable)
	}
	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if poller.Open() != 1 {
		t.Fatal("one failed poll closed the page")
	}

	channel.answer = func(string) (Status, error) { return Status{State: "acked"}, nil }
	clock.at = clock.at.Add(5 * time.Second)
	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if poller.Open() != 0 {
		t.Error("the ack did not settle the page")
	}
}

func TestAMessageTheChannelDisownsIsDropped(t *testing.T) {
	_, channel, poller := stubbed(t)
	if err := poller.Watch(messageID); err != nil {
		t.Fatal(err)
	}

	channel.answer = func(string) (Status, error) {
		return Status{}, fmt.Errorf("%w: No such message.", ErrUnknownMessage)
	}
	if _, err := poller.Tick(context.Background()); err != nil {
		t.Fatal(err)
	}
	if poller.Open() != 0 {
		t.Error("kept polling a message the channel does not have")
	}
}

func TestAnAckTakesThePageOffTheList(t *testing.T) {
	_, channel, poller := stubbed(t)
	if err := poller.Watch(messageID); err != nil {
		t.Fatal(err)
	}

	channel.answer = func(string) (Status, error) {
		return Status{
			State:      "acked",
			ReceivedAt: time.Date(2026, 9, 17, 10, 0, 0, 0, time.UTC),
			AckedAt:    time.Date(2026, 9, 17, 10, 0, 12, 0, time.UTC),
			AckedBy:    "denis",
		}, nil
	}

	settled, err := poller.Tick(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if poller.Open() != 0 {
		t.Error("the page is still being watched")
	}
	status, ok := settled[messageID]
	if !ok {
		t.Fatalf("settled = %v", settled)
	}
	if open, _ := status.OpenFor(); open != 12*time.Second || status.AckedBy != "denis" {
		t.Errorf("settled with %+v", status)
	}
}

func TestRunReturnsOnceNothingIsOpen(t *testing.T) {
	_, channel, poller := stubbed(t)
	if err := poller.Watch(messageID); err != nil {
		t.Fatal(err)
	}
	channel.answer = func(string) (Status, error) { return Status{State: "resolved"}, nil }

	settled, err := poller.Run(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if len(settled) != 1 {
		t.Fatalf("settled = %v", settled)
	}
}

func TestACancelledRunLeavesThePageWatched(t *testing.T) {
	_, _, poller := stubbed(t)
	if err := poller.Watch(messageID); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	poller.sleep = func(context.Context, time.Duration) error {
		cancel()
		return ctx.Err()
	}

	settled, err := poller.Run(ctx)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("err = %v, want the cancellation", err)
	}
	if len(settled) != 0 || poller.Open() != 1 {
		t.Errorf("settled = %v, open = %d", settled, poller.Open())
	}
	if status, ok := poller.Last(messageID); !ok || status.State != "queued" {
		t.Errorf("Last() = %+v, %v", status, ok)
	}
}

func TestWatchingTheSamePageTwiceWatchesOnePage(t *testing.T) {
	_, _, poller := stubbed(t)
	for i := 0; i < 2; i++ {
		if err := poller.Watch(messageID); err != nil {
			t.Fatal(err)
		}
	}
	if poller.Open() != 1 {
		t.Errorf("open = %d, want 1", poller.Open())
	}
}

func TestWatchRefusesSomethingThatIsNotAMessageID(t *testing.T) {
	_, _, poller := stubbed(t)
	if err := poller.Watch("mm_nope"); err == nil {
		t.Fatal("a bad message id was accepted")
	}
}

func nthMessageID(n int) string {
	return fmt.Sprintf("mm_%032x", n)
}
