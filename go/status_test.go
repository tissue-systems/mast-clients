package mast

import (
	"context"
	"errors"
	"testing"
	"time"
)

func TestCheckSaysYesToAKeyThatWorks(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if err := channel.Check(context.Background()); err != nil {
		t.Fatalf("Check: %v", err)
	}
}

func TestCheckSendsNothingWhileDoingIt(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if err := channel.Check(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(mast.sends) != 0 {
		t.Errorf("a key check sent %d message(s)", len(mast.sends))
	}
	if len(mast.polls) != 1 || mast.polls[0] != "check" {
		t.Errorf("polls = %v", mast.polls)
	}
}

func TestCheckSaysNoToAKeyThatDoesNot(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, otherKey)

	if err := channel.Check(context.Background()); !errors.Is(err, ErrUnknownChannel) {
		t.Fatalf("err = %v, want ErrUnknownChannel", err)
	}
}

func TestTheTwo404sAreWhatTellsThemApart(t *testing.T) {
	mast := newFakeMast(t)

	bad := mast.channel(t, otherKey)
	if _, err := bad.Status(context.Background(), messageID); !errors.Is(err, ErrUnknownChannel) {
		t.Errorf("a bad key gave %v, want ErrUnknownChannel", err)
	}

	good := mast.channel(t, goodKey)
	mast.message["id"] = "mm_00000000000000000000000000000000"
	if _, err := good.Status(context.Background(), messageID); !errors.Is(err, ErrUnknownMessage) {
		t.Errorf("a good key gave %v, want ErrUnknownMessage", err)
	}
}

func TestStatusReadsAnOpenPage(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	status, err := channel.Status(context.Background(), messageID)
	if err != nil {
		t.Fatal(err)
	}
	if status.State != "queued" || status.Acknowledged() || status.Done() {
		t.Errorf("status = %+v", status)
	}
}

func TestStatusWorksOutHowLongAPageStoodOpen(t *testing.T) {
	mast := newFakeMast(t)
	mast.message["state"] = "acked"
	mast.message["acked_at"] = "2026-09-17T10:01:17Z"
	mast.message["acked_by"] = "denis"
	channel := mast.channel(t, goodKey)

	status, err := channel.Status(context.Background(), messageID)
	if err != nil {
		t.Fatal(err)
	}
	if !status.Acknowledged() || !status.Done() {
		t.Errorf("status = %+v", status)
	}
	if status.AckedBy != "denis" {
		t.Errorf("AckedBy = %q", status.AckedBy)
	}
	open, ok := status.OpenFor()
	if !ok || open != 77*time.Second {
		t.Errorf("OpenFor() = %s, %v, want 1m17s", open, ok)
	}
}

func TestAMissingStampIsUnknownRatherThanInstant(t *testing.T) {
	mast := newFakeMast(t)
	mast.message["state"] = "acked"
	channel := mast.channel(t, goodKey)

	status, err := channel.Status(context.Background(), messageID)
	if err != nil {
		t.Fatal(err)
	}
	if open, ok := status.OpenFor(); ok {
		t.Errorf("OpenFor() = %s, true, want unknown", open)
	}
}

func TestResolvedAndExpiredAreBothDone(t *testing.T) {
	for _, state := range []string{"resolved", "expired"} {
		t.Run(state, func(t *testing.T) {
			mast := newFakeMast(t)
			mast.message["state"] = state
			channel := mast.channel(t, goodKey)

			status, err := channel.Status(context.Background(), messageID)
			if err != nil {
				t.Fatal(err)
			}
			if !status.Done() || status.Acknowledged() {
				t.Errorf("status = %+v", status)
			}
		})
	}
}

func TestStatusCarriesTheFoldCount(t *testing.T) {
	mast := newFakeMast(t)
	mast.message["dedupe_count"] = 47
	channel := mast.channel(t, goodKey)

	status, err := channel.Status(context.Background(), messageID)
	if err != nil {
		t.Fatal(err)
	}
	if status.DedupeCount != 47 {
		t.Errorf("DedupeCount = %d, want 47", status.DedupeCount)
	}
}

func TestAnotherChannelsMessageIsNotThisOnes(t *testing.T) {
	mast := newFakeMast(t)
	mast.message["id"] = "mm_beefbeefbeefbeefbeefbeefbeefbeef"
	channel := mast.channel(t, goodKey)

	if _, err := channel.Status(context.Background(), messageID); !errors.Is(err, ErrUnknownMessage) {
		t.Fatalf("err = %v, want ErrUnknownMessage", err)
	}
}

func TestSomethingThatIsNotAMessageIDNeverLeavesTheProcess(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.Status(context.Background(), "nope"); err == nil {
		t.Fatal("a bad message id was accepted")
	}
	if len(mast.polls) != 0 {
		t.Errorf("a request was spent on %v", mast.polls)
	}
}

func TestWaitComesBackWithTheAcknowledgement(t *testing.T) {
	mast := newFakeMast(t)
	mast.message["state"] = "acked"
	mast.message["acked_at"] = "2026-09-17T10:00:09Z"
	mast.message["acked_by"] = "denis"
	channel := mast.channel(t, goodKey)

	status, err := channel.Wait(context.Background(), messageID)
	if err != nil {
		t.Fatal(err)
	}
	if !status.Acknowledged() {
		t.Errorf("status = %+v", status)
	}
	if open, _ := status.OpenFor(); open != 9*time.Second {
		t.Errorf("OpenFor() = %s, want 9s", open)
	}
}

func TestWaitHandsBackTheLastReadOnATimeout(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()

	status, err := channel.Wait(ctx, messageID)
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("err = %v, want the deadline", err)
	}
	if status.State != "queued" {
		t.Errorf("status = %+v, want the last thing read", status)
	}
}
