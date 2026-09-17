package mast

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

const (
	goodKey   = "mk_" + "1c9f1c9f1c9f1c9f1c9f1c9f1c9f1c9f1c9f1c9f"
	otherKey  = "mk_" + "2b7d2b7d2b7d2b7d2b7d2b7d2b7d2b7d2b7d2b7d"
	messageID = "mm_" + "7a3e7a3e7a3e7a3e7a3e7a3e7a3e7a3e"
)

// fakeMast is enough of the ingest surface to test a client against, including
// the two different 404s.
type fakeMast struct {
	server *httptest.Server

	sends []send
	polls []string

	state      string
	duplicate  bool
	sendStatus int
	sendError  string
	limited    bool
	retryAfter string
	redirect   bool
	message    map[string]any
}

type send struct {
	path   string
	fields url.Values
}

func newFakeMast(t *testing.T) *fakeMast {
	t.Helper()
	m := &fakeMast{
		state:      "queued",
		sendStatus: http.StatusOK,
		message: map[string]any{
			"id":           messageID,
			"state":        "queued",
			"received_at":  "2026-09-17T10:00:00Z",
			"dedupe_count": 0,
		},
	}
	m.server = httptest.NewServer(http.HandlerFunc(m.handle))
	t.Cleanup(m.server.Close)
	return m
}

func (m *fakeMast) url(key string) string { return m.server.URL + "/m/" + key }

func (m *fakeMast) channel(t *testing.T, key string) *Channel {
	t.Helper()
	channel, err := Open(m.url(key))
	if err != nil {
		t.Fatalf("Open(%s): %v", m.url(key), err)
	}
	return channel
}

func (m *fakeMast) lastSend(t *testing.T) send {
	t.Helper()
	if len(m.sends) == 0 {
		t.Fatal("nothing was sent")
	}
	return m.sends[len(m.sends)-1]
}

func (m *fakeMast) handle(w http.ResponseWriter, r *http.Request) {
	// The edge rewrites "/" onto "/m/", so a client may use either.
	parts := strings.FieldsFunc(r.URL.Path, func(c rune) bool { return c == '/' })
	if len(parts) > 0 && parts[0] == "m" {
		parts = parts[1:]
	}
	var key, section, id string
	if len(parts) > 0 {
		key = parts[0]
	}
	if len(parts) > 1 {
		section = parts[1]
	}
	if len(parts) > 2 {
		id = parts[2]
	}

	switch {
	case m.redirect:
		http.Redirect(w, r, "https://elsewhere.example/", http.StatusFound)
		return
	case m.limited:
		if m.retryAfter != "" {
			w.Header().Set("Retry-After", m.retryAfter)
		}
		writeJSON(w, http.StatusTooManyRequests, errorBody("rate_limited", "Too many requests. Slow down."))
		return
	case key != goodKey:
		writeJSON(w, http.StatusNotFound, errorBody("not_found", "No such channel."))
		return
	}

	if r.Method == http.MethodPost && (section == "" || section == "fail") {
		r.ParseForm()
		m.sends = append(m.sends, send{path: r.URL.Path, fields: r.PostForm})
		if m.sendError != "" {
			writeJSON(w, m.sendStatus, errorBody("invalid_field", m.sendError))
			return
		}
		body := map[string]any{"id": messageID, "state": m.state}
		if m.duplicate {
			body["duplicate"] = true
		}
		writeJSON(w, http.StatusOK, body)
		return
	}

	if r.Method == http.MethodGet && section == "messages" {
		m.polls = append(m.polls, id)
		// The key is resolved before the id is, which is what makes an
		// impossible id a free key check.
		if id != m.message["id"] {
			writeJSON(w, http.StatusNotFound, errorBody("not_found", "No such message."))
			return
		}
		writeJSON(w, http.StatusOK, m.message)
		return
	}

	writeJSON(w, http.StatusNotFound, errorBody("not_found", "No such channel."))
}

func errorBody(code, text string) map[string]any {
	return map[string]any{"error": map[string]any{"code": code, "message": text}}
}

func writeJSON(w http.ResponseWriter, status int, body map[string]any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	json.NewEncoder(w).Encode(body)
}
