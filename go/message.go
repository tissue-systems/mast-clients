package mast

import (
	"fmt"
	"net/url"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

// Priorities and sounds Mast knows. A sound it does not know is not an error
// on the phone: iOS plays its own tone and reports nothing, so a typo is a
// page that silently loses its alarm.
var (
	Priorities = []string{"quiet", "normal", "loud", "page"}
	Sounds     = []string{"default", "bleep", "chirp", "klaxon", "pager", "rising"}
)

// The server's own caps, mirrored here so a send that cannot be accepted costs
// none of the channel's 60 requests a minute.
const (
	maxTitleChars     = 250
	maxBodyChars      = 4096
	maxURLChars       = 512
	maxDedupeKeyChars = 120

	minRetry  = 30 * time.Second
	maxRetry  = 24 * time.Hour
	minExpire = time.Minute
	maxExpire = 7 * 24 * time.Hour
)

// Retry and Expire are unset at their zero value. These turn the channel's own
// default off, which the wire spells as a literal 0.
const (
	DisableRetry  = time.Duration(-1)
	DisableExpire = time.Duration(-1)
)

// Message is one send. Every field is optional: a Message with nothing in it is
// a vital's heartbeat.
type Message struct {
	Title    string
	Body     string
	Priority string
	Sound    string

	// URL and URLTitle put a link on the card: the dashboard, the runbook.
	URL      string
	URLTitle string

	// Callback is stored on the message and, for now, never fired. Poll
	// instead.
	Callback string

	// Key folds a second send into the card an earlier one opened, and is what
	// ResolveKey closes later.
	Key string

	Retry  time.Duration
	Expire time.Duration

	// Ack keeps the page repeating until somebody answers it.
	Ack bool

	// Silent delivers without a sound. It cannot be combined with Ack or the
	// page tier, which exist to make a noise.
	Silent bool

	// Timestamp is when the event happened, if that is not now.
	Timestamp time.Time

	resolve string
}

func (m Message) form() (url.Values, error) {
	form := url.Values{}

	if err := put(form, "title", m.Title, maxTitleChars); err != nil {
		return nil, err
	}
	if err := put(form, "body", m.Body, maxBodyChars); err != nil {
		return nil, err
	}
	if err := put(form, "url_title", m.URLTitle, maxTitleChars); err != nil {
		return nil, err
	}
	if err := put(form, "key", m.Key, maxDedupeKeyChars); err != nil {
		return nil, err
	}

	if m.Priority != "" {
		if !contains(Priorities, m.Priority) {
			return nil, &Rejected{Reason: "priority must be one of " + strings.Join(Priorities, ", ")}
		}
		form.Set("priority", m.Priority)
	}

	if m.Sound != "" {
		if !contains(Sounds, strings.TrimSuffix(m.Sound, ".caf")) {
			return nil, &Rejected{Reason: "sound must be one of " + strings.Join(Sounds, ", ")}
		}
		form.Set("sound", m.Sound)
	}

	if m.URL != "" {
		if err := put(form, "url", m.URL, maxURLChars); err != nil {
			return nil, err
		}
		if !strings.HasPrefix(m.URL, "http://") && !strings.HasPrefix(m.URL, "https://") {
			return nil, &Rejected{Reason: "url must start with http:// or https://"}
		}
	}

	if m.Callback != "" {
		if err := put(form, "callback", m.Callback, maxURLChars); err != nil {
			return nil, err
		}
		if !strings.HasPrefix(m.Callback, "https://") {
			return nil, &Rejected{Reason: "callback must start with https://"}
		}
	}

	if m.Retry != 0 {
		seconds, err := wholeSeconds("retry", "DisableRetry", m.Retry, minRetry, maxRetry)
		if err != nil {
			return nil, err
		}
		form.Set("retry", strconv.Itoa(seconds))
	}
	if m.Expire != 0 {
		seconds, err := wholeSeconds("expire", "DisableExpire", m.Expire, minExpire, maxExpire)
		if err != nil {
			return nil, err
		}
		form.Set("expire", strconv.Itoa(seconds))
	}

	if m.Ack {
		form.Set("ack", "required")
	}
	if m.Silent {
		if m.Ack || m.Priority == "page" {
			return nil, &Rejected{Reason: "silent cannot be combined with ack or priority page"}
		}
		form.Set("silent", "1")
	}

	if !m.Timestamp.IsZero() {
		form.Set("timestamp", strconv.FormatInt(m.Timestamp.Unix(), 10))
	}
	if m.resolve != "" {
		form.Set("resolve", m.resolve)
	}
	return form, nil
}

// put writes a field if it has anything in it, counting characters rather than
// bytes, which is how the server counts them.
func put(form url.Values, name, value string, limit int) error {
	if value == "" {
		return nil
	}
	if utf8.RuneCountInString(value) > limit {
		return &Rejected{Reason: fmt.Sprintf("%s is longer than %d characters", name, limit)}
	}
	form.Set(name, value)
	return nil
}

func wholeSeconds(name, disable string, d, low, high time.Duration) (int, error) {
	if d == -1 {
		return 0, nil
	}
	if d%time.Second != 0 {
		return 0, &Rejected{Reason: name + " has to be a whole number of seconds"}
	}
	if d < low || d > high {
		return 0, &Rejected{
			Reason: fmt.Sprintf("%s has to be between %s and %s, or %s", name, low, high, disable),
		}
	}
	return int(d / time.Second), nil
}

func contains(values []string, want string) bool {
	for _, value := range values {
		if value == want {
			return true
		}
	}
	return false
}
