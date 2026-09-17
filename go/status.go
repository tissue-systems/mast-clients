package mast

import "time"

// Sent is what Mast answered a send with. State is one of queued, muted,
// deduped, suppressed, alive or resolved; ID is empty for a heartbeat, which
// stores no card.
type Sent struct {
	ID        string
	State     string
	Duplicate bool
	OpenFor   time.Duration
}

func newSent(payload map[string]any) Sent {
	sent := Sent{
		ID:    text(payload, "id"),
		State: text(payload, "state"),
	}
	if duplicate, ok := payload["duplicate"].(bool); ok {
		sent.Duplicate = duplicate
	}
	if seconds, ok := payload["open_for"].(float64); ok {
		sent.OpenFor = time.Duration(seconds) * time.Second
	}
	if sent.State == "" {
		sent.State = "queued"
	}
	return sent
}

// Status is one message as Mast currently sees it. A time that Mast did not
// send is the zero time: it omits rather than zeroes what it could not work
// out, so absent means unknown and not instant.
type Status struct {
	ID          string
	State       string
	ReceivedAt  time.Time
	AckedAt     time.Time
	AckedBy     string
	ResolvedAt  time.Time
	ExpiresAt   time.Time
	DedupeCount int
}

func newStatus(payload map[string]any) Status {
	status := Status{
		ID:         text(payload, "id"),
		State:      text(payload, "state"),
		ReceivedAt: stamp(payload, "received_at"),
		AckedAt:    stamp(payload, "acked_at"),
		AckedBy:    text(payload, "acked_by"),
		ResolvedAt: stamp(payload, "resolved_at"),
		ExpiresAt:  stamp(payload, "expires_at"),
	}
	if count, ok := payload["dedupe_count"].(float64); ok {
		status.DedupeCount = int(count)
	}
	return status
}

// Acknowledged reports whether a human answered the page.
func (s Status) Acknowledged() bool { return s.State == "acked" }

// Done reports whether the message has reached a state it will not leave,
// which is when polling it stops.
func (s Status) Done() bool { return terminalStates[s.State] }

// OpenFor is how long the page stood open. The second return is false when
// either stamp is missing, which is unknown rather than zero.
func (s Status) OpenFor() (time.Duration, bool) {
	end := s.AckedAt
	if end.IsZero() {
		end = s.ResolvedAt
	}
	if s.ReceivedAt.IsZero() || end.IsZero() {
		return 0, false
	}
	open := end.Sub(s.ReceivedAt)
	if open < 0 {
		return 0, true
	}
	return open, true
}

func text(payload map[string]any, field string) string {
	value, _ := payload[field].(string)
	return value
}

func stamp(payload map[string]any, field string) time.Time {
	value, ok := payload[field].(string)
	if !ok || value == "" {
		return time.Time{}
	}
	parsed, err := time.Parse(time.RFC3339, value)
	if err != nil {
		return time.Time{}
	}
	return parsed
}
