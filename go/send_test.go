package mast

import (
	"context"
	"errors"
	"net/http"
	"net/url"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestSendPostsTheFieldsItWasGiven(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	sent, err := channel.Send(context.Background(), Message{
		Title:    "Disk",
		Body:     "92% on s10",
		Priority: "loud",
	})
	if err != nil {
		t.Fatalf("Send: %v", err)
	}
	if sent.ID != messageID || sent.State != "queued" {
		t.Errorf("Send returned %+v", sent)
	}
	want := url.Values{"title": {"Disk"}, "body": {"92% on s10"}, "priority": {"loud"}}
	if got := mast.lastSend(t).fields; !reflect.DeepEqual(got, want) {
		t.Errorf("sent %v, want %v", got, want)
	}
}

func TestSendLeavesOutWhatItWasNotGiven(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.Send(context.Background(), Message{Body: "just this"}); err != nil {
		t.Fatal(err)
	}
	if got := mast.lastSend(t).fields; len(got) != 1 || got.Get("body") != "just this" {
		t.Errorf("sent %v, want only a body", got)
	}
}

func TestPageAsksForThePagerTier(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.Page(context.Background(), Message{Body: "s10 is down"}); err != nil {
		t.Fatal(err)
	}
	if got := mast.lastSend(t).fields.Get("priority"); got != "page" {
		t.Errorf("priority = %q, want page", got)
	}
}

func TestAHeartbeatCarriesNothing(t *testing.T) {
	mast := newFakeMast(t)
	mast.state = "alive"
	channel := mast.channel(t, goodKey)

	sent, err := channel.Ping(context.Background(), Message{})
	if err != nil {
		t.Fatal(err)
	}
	if sent.State != "alive" {
		t.Errorf("state = %q, want alive", sent.State)
	}
	if got := mast.lastSend(t).fields; len(got) != 0 {
		t.Errorf("a heartbeat sent %v", got)
	}
}

func TestAFoldComesBackAsADuplicate(t *testing.T) {
	mast := newFakeMast(t)
	mast.state = "deduped"
	mast.duplicate = true
	channel := mast.channel(t, goodKey)

	sent, err := channel.Send(context.Background(), Message{Body: "flapping", Key: "disk-s10"})
	if err != nil {
		t.Fatal(err)
	}
	if !sent.Duplicate {
		t.Error("Duplicate = false, want true")
	}
	if got := mast.lastSend(t).fields.Get("key"); got != "disk-s10" {
		t.Errorf("key = %q", got)
	}
}

func TestResolveByKey(t *testing.T) {
	mast := newFakeMast(t)
	mast.state = "resolved"
	channel := mast.channel(t, goodKey)

	if _, err := channel.ResolveKey(context.Background(), "disk-s10", Message{Body: "back under 80%"}); err != nil {
		t.Fatal(err)
	}
	want := url.Values{"body": {"back under 80%"}, "key": {"disk-s10"}, "resolve": {"1"}}
	if got := mast.lastSend(t).fields; !reflect.DeepEqual(got, want) {
		t.Errorf("sent %v, want %v", got, want)
	}
}

func TestResolveByMessageID(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.ResolveMessage(context.Background(), messageID, Message{}); err != nil {
		t.Fatal(err)
	}
	want := url.Values{"resolve": {messageID}}
	if got := mast.lastSend(t).fields; !reflect.DeepEqual(got, want) {
		t.Errorf("sent %v, want %v", got, want)
	}
}

func TestResolveNeedsSomethingToClose(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.ResolveKey(context.Background(), "", Message{}); err == nil {
		t.Error("an empty key was accepted")
	}
	if _, err := channel.ResolveMessage(context.Background(), "mm_nope", Message{}); err == nil {
		t.Error("a bad message id was accepted")
	}
	if len(mast.sends) != 0 {
		t.Errorf("%d request(s) were spent on a resolve that could not work", len(mast.sends))
	}
}

func TestFailPostsToTheVitalsOwnRoute(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.Fail(context.Background(), Message{Body: "no heartbeat"}); err != nil {
		t.Fatal(err)
	}
	if got := mast.lastSend(t).path; !strings.HasSuffix(got, "/fail") {
		t.Errorf("posted to %q", got)
	}
}

func TestRetryAndExpireAreSentAsSeconds(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	_, err := channel.Page(context.Background(), Message{
		Body:   "up",
		Retry:  time.Minute,
		Expire: time.Hour,
	})
	if err != nil {
		t.Fatal(err)
	}
	fields := mast.lastSend(t).fields
	if fields.Get("retry") != "60" || fields.Get("expire") != "3600" {
		t.Errorf("retry = %q, expire = %q", fields.Get("retry"), fields.Get("expire"))
	}
}

func TestDisableTurnsTheChannelDefaultOff(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{
		Body:   "quiet",
		Retry:  DisableRetry,
		Expire: DisableExpire,
	})
	if err != nil {
		t.Fatal(err)
	}
	fields := mast.lastSend(t).fields
	if fields.Get("retry") != "0" || fields.Get("expire") != "0" {
		t.Errorf("retry = %q, expire = %q, want both 0", fields.Get("retry"), fields.Get("expire"))
	}
}

func TestASoundIsTakenWithOrWithoutTheExtension(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	for _, sound := range []string{"klaxon", "klaxon.caf"} {
		if _, err := channel.Send(context.Background(), Message{Body: "a", Sound: sound}); err != nil {
			t.Fatalf("%s: %v", sound, err)
		}
		if got := mast.lastSend(t).fields.Get("sound"); got != sound {
			t.Errorf("sound = %q, want %q", got, sound)
		}
	}
}

func TestAckIsSentAsTheWordTheServerWants(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	if _, err := channel.Send(context.Background(), Message{Body: "confirm this", Ack: true}); err != nil {
		t.Fatal(err)
	}
	if got := mast.lastSend(t).fields.Get("ack"); got != "required" {
		t.Errorf("ack = %q, want required", got)
	}
}

func TestLimitsCountCharactersNotBytes(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, goodKey)

	body := strings.Repeat("é", maxBodyChars)
	if _, err := channel.Send(context.Background(), Message{Body: body}); err != nil {
		t.Fatalf("a body of %d characters was refused: %v", maxBodyChars, err)
	}
	if _, err := channel.Send(context.Background(), Message{Body: body + "é"}); err == nil {
		t.Error("a body one character over the cap was accepted")
	}
}

func TestRefusedBeforeItCostsARequest(t *testing.T) {
	cases := []struct {
		name    string
		message Message
	}{
		{"a title over the cap", Message{Title: strings.Repeat("x", maxTitleChars+1)}},
		{"a body over the cap", Message{Body: strings.Repeat("x", maxBodyChars+1)}},
		{"a url over the cap", Message{URL: "https://e.example/" + strings.Repeat("x", maxURLChars)}},
		{"a dedupe key over the cap", Message{Body: "a", Key: strings.Repeat("k", maxDedupeKeyChars+1)}},
		{"a url that is not http", Message{Body: "a", URL: "ftp://files.example/x"}},
		{"a callback that is not https", Message{Body: "a", Callback: "http://cb.example/x"}},
		{"a priority Mast does not have", Message{Body: "a", Priority: "urgent"}},
		{"a sound that would play as the default", Message{Body: "a", Sound: "buzzer"}},
		{"a retry under the floor", Message{Body: "a", Retry: 29 * time.Second}},
		{"an expire over the ceiling", Message{Body: "a", Expire: 8 * 24 * time.Hour}},
		{"a retry that is not whole seconds", Message{Body: "a", Retry: 30500 * time.Millisecond}},
		{"silent with an ack", Message{Body: "a", Ack: true, Silent: true}},
		{"silent on a page", Message{Body: "a", Priority: "page", Silent: true}},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			mast := newFakeMast(t)
			channel := mast.channel(t, goodKey)

			_, err := channel.Send(context.Background(), tc.message)
			var rejected *Rejected
			if !errors.As(err, &rejected) {
				t.Fatalf("err = %v, want a Rejected", err)
			}
			if len(mast.sends) != 0 {
				t.Errorf("a request was spent on a send that could not be accepted")
			}
		})
	}
}

func TestAnUnknownKeyIsItsOwnError(t *testing.T) {
	mast := newFakeMast(t)
	channel := mast.channel(t, otherKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	if !errors.Is(err, ErrUnknownChannel) {
		t.Fatalf("err = %v, want ErrUnknownChannel", err)
	}
}

func TestTheReasonComesOutOfTheNestedErrorBody(t *testing.T) {
	mast := newFakeMast(t)
	mast.sendStatus = http.StatusBadRequest
	mast.sendError = "Field 'title' is longer than 250 characters."
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	var rejected *Rejected
	if !errors.As(err, &rejected) {
		t.Fatalf("err = %v, want a Rejected", err)
	}
	if rejected.Reason != mast.sendError {
		t.Errorf("reason = %q, want %q", rejected.Reason, mast.sendError)
	}
}

func TestRetryAfterIsReadOffA429(t *testing.T) {
	mast := newFakeMast(t)
	mast.limited = true
	mast.retryAfter = "7"
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	var limited *RateLimited
	if !errors.As(err, &limited) {
		t.Fatalf("err = %v, want a RateLimited", err)
	}
	if limited.RetryAfter != 7*time.Second {
		t.Errorf("RetryAfter = %s, want 7s", limited.RetryAfter)
	}
}

func TestAMissingRetryAfterFallsBackToASecond(t *testing.T) {
	mast := newFakeMast(t)
	mast.limited = true
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	var limited *RateLimited
	if !errors.As(err, &limited) || limited.RetryAfter != time.Second {
		t.Fatalf("err = %v, want a one second back-off", err)
	}
}

func TestAnOversizeRequestIsARejection(t *testing.T) {
	mast := newFakeMast(t)
	mast.sendStatus = http.StatusRequestEntityTooLarge
	mast.sendError = "Body too large."
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	var rejected *Rejected
	if !errors.As(err, &rejected) {
		t.Fatalf("err = %v, want a Rejected", err)
	}
}

func TestA5xxIsNotARejection(t *testing.T) {
	mast := newFakeMast(t)
	mast.sendStatus = http.StatusServiceUnavailable
	mast.sendError = "Upstream unavailable."
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	var unavailable *Unavailable
	if !errors.As(err, &unavailable) {
		t.Fatalf("err = %v, want an Unavailable", err)
	}
	var rejected *Rejected
	if errors.As(err, &rejected) {
		t.Error("a 5xx must not read as a refused send: the message may have been stored")
	}
}

func TestARedirectIsNotFollowed(t *testing.T) {
	mast := newFakeMast(t)
	mast.redirect = true
	channel := mast.channel(t, goodKey)

	_, err := channel.Send(context.Background(), Message{Body: "a"})
	if !errors.Is(err, ErrUnreachable) {
		t.Fatalf("err = %v, want ErrUnreachable", err)
	}
}

func TestAHostThatIsNotListeningIsUnreachable(t *testing.T) {
	channel, err := Open("http://127.0.0.1:1/m/" + goodKey)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := channel.Send(context.Background(), Message{Body: "a"}); !errors.Is(err, ErrUnreachable) {
		t.Fatalf("err = %v, want ErrUnreachable", err)
	}
}
