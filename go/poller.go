package mast

import (
	"context"
	"errors"
	"sort"
	"time"
)

// Age of an open page against how often to ask about it. The first rung the
// page has not aged past wins: fast while somebody may still be looking at it,
// slow once it is clear nobody is.
var pollLadder = []struct {
	Until time.Duration
	Every time.Duration
}{
	{30 * time.Second, 3 * time.Second},
	{5 * time.Minute, 10 * time.Second},
	{0, 30 * time.Second},
}

const pollTick = time.Second

// statuses is all a Poller needs of a Channel, which is also what makes the
// ladder and the budget testable without a socket.
type statuses interface {
	Status(ctx context.Context, messageID string) (Status, error)
}

// Poller watches open pages on one channel until they settle.
//
// Several at once share the channel's request budget, so the oldest is asked
// about first when there is not enough to go round: it is the one somebody is
// still standing over. A 429 backs off the whole channel rather than the one
// page, because the limit belongs to the key.
type Poller struct {
	// Budget is how many requests a minute polling may spend. The rest of the
	// channel's allowance is left for sending.
	Budget int

	channel statuses
	open    map[string]*watched
	last    map[string]Status
	spent   []time.Time
	paused  time.Time

	now   func() time.Time
	sleep func(context.Context, time.Duration) error
}

type watched struct {
	messageID string
	started   time.Time
	lastPoll  time.Time
}

func (w *watched) due(at time.Time) bool {
	age := at.Sub(w.started)
	for _, rung := range pollLadder {
		if rung.Until == 0 || age < rung.Until {
			return at.Sub(w.lastPoll) >= rung.Every
		}
	}
	return false
}

// NewPoller watches pages on one channel, spending half the channel's request
// allowance.
func NewPoller(c *Channel) *Poller {
	return &Poller{
		Budget:  PollBudgetPerMinute,
		channel: c,
		open:    map[string]*watched{},
		last:    map[string]Status{},
		now:     time.Now,
		sleep:   sleep,
	}
}

// Watch starts following a message. Watching one twice watches one page.
func (p *Poller) Watch(messageID string) error {
	if !messageIDPattern.MatchString(messageID) {
		return &Rejected{Reason: messageID + " is not a message id"}
	}
	if _, ok := p.open[messageID]; !ok {
		p.open[messageID] = &watched{messageID: messageID, started: p.now()}
	}
	return nil
}

// Open is how many pages are still being watched.
func (p *Poller) Open() int { return len(p.open) }

// Last is the most recent status read for a message.
func (p *Poller) Last(messageID string) (Status, bool) {
	status, ok := p.last[messageID]
	return status, ok
}

// Tick asks about whatever is due and affordable, and returns what settled.
func (p *Poller) Tick(ctx context.Context) (map[string]Status, error) {
	settled := map[string]Status{}
	at := p.now()
	if at.Before(p.paused) {
		return settled, nil
	}

	var due []*watched
	for _, page := range p.open {
		if page.due(at) {
			due = append(due, page)
		}
	}
	sort.Slice(due, func(i, j int) bool { return due[i].started.Before(due[j].started) })

	for _, page := range due {
		if !p.take(at) {
			break
		}
		page.lastPoll = at
		status, err := p.channel.Status(ctx, page.messageID)
		if err != nil {
			var limited *RateLimited
			switch {
			case errors.As(err, &limited):
				p.paused = p.now().Add(limited.RetryAfter)
				return settled, nil
			case errors.Is(err, ErrUnknownMessage), errors.Is(err, ErrUnknownChannel):
				// Nothing more is going to happen to a message the channel
				// will not talk about.
				delete(p.open, page.messageID)
			case ctx.Err() != nil:
				return settled, ctx.Err()
			default:
				// A blip is not an answer: leave the page open for the next
				// tick.
			}
			continue
		}

		p.last[page.messageID] = status
		if status.Done() {
			delete(p.open, page.messageID)
			settled[page.messageID] = status
		}
	}
	return settled, nil
}

// Run ticks until nothing is open or ctx is done. Whatever settled first is
// returned alongside ctx's error, and anything still open stays watched, so
// calling Run again picks up where this left off. A deadline here is not a
// failed page: it is still open on the phone and can still be answered.
func (p *Poller) Run(ctx context.Context) (map[string]Status, error) {
	settled := map[string]Status{}
	for len(p.open) > 0 {
		done, err := p.Tick(ctx)
		for id, status := range done {
			settled[id] = status
		}
		if err != nil {
			return settled, err
		}
		if len(p.open) == 0 {
			break
		}
		if err := p.sleep(ctx, pollTick); err != nil {
			return settled, err
		}
	}
	return settled, nil
}

// take spends one request if the last sixty seconds leave room for it.
//
// A token bucket would be the usual answer, but a full one lets through its
// own depth on top of the refill, which is how a limiter sized at half the
// channel's allowance ends up spending nearly all of it.
func (p *Poller) take(at time.Time) bool {
	cutoff := at.Add(-time.Minute)
	kept := p.spent[:0]
	for _, when := range p.spent {
		if when.After(cutoff) {
			kept = append(kept, when)
		}
	}
	p.spent = kept
	if len(p.spent) >= p.Budget {
		return false
	}
	p.spent = append(p.spent, at)
	return true
}

func sleep(ctx context.Context, d time.Duration) error {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}
