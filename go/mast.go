// Package mast sends to a Mast channel and waits for the acknowledgement.
//
// The channel key in the URL is the whole credential, so there is nothing to
// configure and nothing to sign. There is also nothing inbound: Mast stores a
// callback on a message but does not fire it, and most senders are not
// reachable from the internet anyway, so an acknowledgement is polled.
package mast

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"time"
)

const DefaultHost = "https://mast.tissue.dev"

var (
	keyPattern       = regexp.MustCompile(`^mk_[0-9a-f]{40}$`)
	messageIDPattern = regexp.MustCompile(`^mm_[0-9a-f]{32}$`)
)

// States a message never leaves. Anything else is still in play.
var terminalStates = map[string]bool{"acked": true, "resolved": true, "expired": true}

// 60 requests a minute per key, shared between sends and polls. Half is left
// for sends: a client that polls itself out of a budget cannot report the next
// outage.
const (
	RateLimitPerMinute  = 60
	PollBudgetPerMinute = 30
)

const defaultTimeout = 15 * time.Second

// ErrUnreachable means Mast could not be reached, or did not answer in time.
var ErrUnreachable = errors.New("mast: unreachable")

// ErrUnknownChannel means the key does not resolve. Unknown, malformed,
// revoked and rotated away past the grace window all answer alike, so that a
// key cannot be probed - and so a client cannot tell them apart either.
var ErrUnknownChannel = errors.New("mast: no such channel")

// ErrUnknownMessage means the key is good but the message id is not one of its
// own.
var ErrUnknownMessage = errors.New("mast: no such message")

// RateLimited is returned over 60 requests a minute on the key, or 600 across
// the owner's channels.
type RateLimited struct {
	RetryAfter time.Duration
}

func (e *RateLimited) Error() string {
	return fmt.Sprintf("mast: rate limited, retry in %s", e.RetryAfter)
}

// Rejected is a send Mast refused, or one this package refused on its behalf.
// The text names the field.
type Rejected struct {
	Reason string
}

func (e *Rejected) Error() string { return "mast: " + e.Reason }

// Unavailable is a 5xx. It does not prove the message was not stored: the
// failure can land on either side of the durability line, so retrying blind
// can page somebody twice. Retry with a dedupe key, or not at all.
type Unavailable struct {
	Status int
	Reason string
}

func (e *Unavailable) Error() string {
	return fmt.Sprintf("mast: server answered %d: %s", e.Status, e.Reason)
}

// Channel is one channel's URL and the client used to reach it.
type Channel struct {
	Origin string
	Prefix string
	Key    string
	HTTP   *http.Client
}

// Open parses a channel URL. It takes the short form the app copies, the
// explicit /m/ form the management API hands out, a host with no scheme and a
// bare key.
func Open(rawURL string) (*Channel, error) {
	origin, prefix, key, err := parseChannelURL(rawURL)
	if err != nil {
		return nil, err
	}
	return &Channel{
		Origin: origin,
		Prefix: prefix,
		Key:    key,
		HTTP: &http.Client{
			Timeout: defaultTimeout,
			// The key is in the URL, so following a redirect would hand it to
			// whoever answered.
			CheckRedirect: func(*http.Request, []*http.Request) error {
				return http.ErrUseLastResponse
			},
		},
	}, nil
}

func (c *Channel) URL() string { return c.Origin + c.Prefix + "/" + c.Key }

// String keeps the key out of logs.
func (c *Channel) String() string {
	return fmt.Sprintf("mast channel %s%s/%s...", c.Origin, c.Prefix, c.Key[:7])
}

// Send posts one message. It returns once Mast has stored it, which is before
// the push leaves for Apple: this is not a promise that a phone made a noise.
func (c *Channel) Send(ctx context.Context, m Message) (Sent, error) {
	form, err := m.form()
	if err != nil {
		return Sent{}, err
	}
	return c.post(ctx, c.URL(), form)
}

// Page sends at the pager tier, which repeats until somebody acknowledges it.
// A page is the one tier storm control will not fold, so give it a Key if its
// source can flap.
func (c *Channel) Page(ctx context.Context, m Message) (Sent, error) {
	m.Priority = "page"
	return c.Send(ctx, m)
}

// ResolveKey closes whatever that dedupe key has open: the card turns green
// and stops repeating. A resolve that finds nothing open is not an error; it
// is stored quietly.
func (c *Channel) ResolveKey(ctx context.Context, key string, m Message) (Sent, error) {
	if key == "" {
		return Sent{}, &Rejected{Reason: "a resolve needs a dedupe key"}
	}
	m.Key = key
	m.resolve = "1"
	return c.Send(ctx, m)
}

// ResolveMessage closes one specific card by the id its own send returned.
func (c *Channel) ResolveMessage(ctx context.Context, messageID string, m Message) (Sent, error) {
	if !messageIDPattern.MatchString(messageID) {
		return Sent{}, &Rejected{Reason: strconv.Quote(messageID) + " is not a message id"}
	}
	m.resolve = messageID
	return c.Send(ctx, m)
}

// Ping is a heartbeat on a vital. An empty Message is proof of life and no
// card, so there is no id to follow.
func (c *Channel) Ping(ctx context.Context, m Message) (Sent, error) {
	return c.Send(ctx, m)
}

// Fail flatlines a vital now rather than waiting out its grace window. Only a
// vital has this route; a plain channel answers the 404 an unknown key gets.
func (c *Channel) Fail(ctx context.Context, m Message) (Sent, error) {
	form, err := m.form()
	if err != nil {
		return Sent{}, err
	}
	return c.post(ctx, c.URL()+"/fail", form)
}

// Status reads one message.
func (c *Channel) Status(ctx context.Context, messageID string) (Status, error) {
	if !messageIDPattern.MatchString(messageID) {
		return Status{}, &Rejected{Reason: strconv.Quote(messageID) + " is not a message id"}
	}
	payload, err := c.get(ctx, c.messageURL(messageID))
	if err != nil {
		return Status{}, err
	}
	return newStatus(payload), nil
}

// Check proves the key works without sending anything.
//
// The status route resolves the key before it validates the message id, so an
// id that cannot exist separates the two 404s: a bad key answers "No such
// channel." and a good one "No such message." Nothing is stored and no phone
// goes off.
func (c *Channel) Check(ctx context.Context) error {
	_, err := c.get(ctx, c.messageURL("check"))
	if errors.Is(err, ErrUnknownMessage) {
		return nil
	}
	if err != nil {
		return err
	}
	return errors.New("mast: the key check was answered in a way this version does not expect")
}

// Wait polls one message until it settles or ctx runs out. A ctx deadline
// comes back as ctx's error with the last status read: the page is still open
// on the phone, where it can still be answered.
func (c *Channel) Wait(ctx context.Context, messageID string) (Status, error) {
	p := NewPoller(c)
	if err := p.Watch(messageID); err != nil {
		return Status{}, err
	}
	settled, err := p.Run(ctx)
	if status, ok := settled[messageID]; ok {
		return status, nil
	}
	last, _ := p.Last(messageID)
	return last, err
}

func (c *Channel) messageURL(messageID string) string {
	return c.URL() + "/messages/" + messageID
}

func (c *Channel) post(ctx context.Context, target string, form url.Values) (Sent, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, target, strings.NewReader(form.Encode()))
	if err != nil {
		return Sent{}, fmt.Errorf("%w: %v", ErrUnreachable, err)
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")

	status, payload, err := c.do(req)
	if err != nil {
		return Sent{}, err
	}
	switch {
	case status == http.StatusBadRequest:
		return Sent{}, &Rejected{Reason: message(payload, "the send was refused")}
	case status == http.StatusRequestEntityTooLarge:
		return Sent{}, &Rejected{Reason: "the request is over Mast's 16 KiB limit"}
	case status >= 500:
		return Sent{}, &Unavailable{Status: status, Reason: message(payload, "no reason given")}
	case status >= 400:
		return Sent{}, fmt.Errorf("mast: server answered %d: %s", status, message(payload, "no reason given"))
	}
	return newSent(payload), nil
}

func (c *Channel) get(ctx context.Context, target string) (map[string]any, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, target, nil)
	if err != nil {
		return nil, fmt.Errorf("%w: %v", ErrUnreachable, err)
	}
	status, payload, err := c.do(req)
	if err != nil {
		return nil, err
	}
	if status >= 400 {
		return nil, fmt.Errorf("mast: server answered %d: %s", status, message(payload, "no reason given"))
	}
	return payload, nil
}

func (c *Channel) do(req *http.Request) (int, map[string]any, error) {
	client := c.HTTP
	if client == nil {
		client = http.DefaultClient
	}
	resp, err := client.Do(req)
	if err != nil {
		return 0, nil, fmt.Errorf("%w: %v", ErrUnreachable, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 300 && resp.StatusCode < 400 {
		return 0, nil, fmt.Errorf("%w: answered a redirect, which is not followed", ErrUnreachable)
	}

	payload := decode(resp.Body)
	switch resp.StatusCode {
	case http.StatusTooManyRequests:
		return 0, nil, &RateLimited{RetryAfter: retryAfter(resp.Header.Get("Retry-After"))}
	case http.StatusNotFound:
		text := message(payload, "No such channel.")
		if strings.Contains(text, "No such message") {
			return 0, nil, fmt.Errorf("%w: %s", ErrUnknownMessage, text)
		}
		return 0, nil, fmt.Errorf("%w: %s", ErrUnknownChannel, text)
	}
	return resp.StatusCode, payload, nil
}

func decode(body io.Reader) map[string]any {
	var payload map[string]any
	// A proxy in front of a self-hosted edge can answer with something that is
	// not JSON at all, and that is not worth a separate error.
	if err := json.NewDecoder(body).Decode(&payload); err != nil {
		return map[string]any{}
	}
	if payload == nil {
		return map[string]any{}
	}
	return payload
}

// message digs out the human half of an error body. Mast nests it:
// {"error": {"code": ..., "message": ...}}. Reading payload["message"] finds
// nothing and falls back to the default, which turns a good key's "No such
// message." into "No such channel." and fails setup for every valid key.
func message(payload map[string]any, fallback string) string {
	if nested, ok := payload["error"].(map[string]any); ok {
		if text, ok := nested["message"].(string); ok && text != "" {
			return text
		}
	}
	if text, ok := payload["error"].(string); ok && text != "" {
		return text
	}
	if text, ok := payload["message"].(string); ok && text != "" {
		return text
	}
	return fallback
}

func retryAfter(header string) time.Duration {
	seconds, err := strconv.Atoi(strings.TrimSpace(header))
	if err != nil || seconds < 1 {
		return time.Second
	}
	return time.Duration(seconds) * time.Second
}

func parseChannelURL(raw string) (origin, prefix, key string, err error) {
	text := strings.TrimSpace(raw)
	if text == "" {
		return "", "", "", errors.New("mast: no channel URL given")
	}
	if keyPattern.MatchString(text) {
		return DefaultHost, "", text, nil
	}
	if !strings.Contains(text, "://") {
		text = "https://" + text
	}
	parsed, err := url.Parse(text)
	if err != nil {
		return "", "", "", fmt.Errorf("mast: %q is not a URL", raw)
	}
	if parsed.Scheme != "https" && parsed.Scheme != "http" {
		return "", "", "", errors.New("mast: a channel URL has to be an http(s) URL")
	}
	if parsed.Host == "" {
		return "", "", "", fmt.Errorf("mast: %q has no host", raw)
	}

	var segments []string
	for _, segment := range strings.Split(parsed.Path, "/") {
		if segment != "" {
			segments = append(segments, segment)
		}
	}
	// The edge rewrites "/" onto "/m/", but a self-hosted one in front of the
	// management API may not, so whichever prefix was pasted is kept.
	if len(segments) > 0 && segments[0] == "m" {
		prefix = "/m"
		segments = segments[1:]
	}
	if len(segments) == 0 {
		return "", "", "", errors.New("mast: there is no key in that URL")
	}
	if !keyPattern.MatchString(segments[0]) {
		return "", "", "", fmt.Errorf("mast: %q is not a channel key: expected mk_ and 40 hex characters", segments[0])
	}
	return parsed.Scheme + "://" + parsed.Host, prefix, segments[0], nil
}
