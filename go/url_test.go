package mast

import (
	"strings"
	"testing"
)

func TestOpenTakesEveryShapeOfChannelURL(t *testing.T) {
	cases := []struct {
		name   string
		raw    string
		origin string
		prefix string
	}{
		{"short form", "https://mast.tissue.dev/" + goodKey, "https://mast.tissue.dev", ""},
		{"with the m prefix", "https://mast.tissue.dev/m/" + goodKey, "https://mast.tissue.dev", "/m"},
		{"bare key", goodKey, DefaultHost, ""},
		{"no scheme", "mast.example.com/m/" + goodKey, "https://mast.example.com", "/m"},
		{"with a port", "http://127.0.0.1:8899/m/" + goodKey, "http://127.0.0.1:8899", "/m"},
		{"pasted with whitespace", "  https://mast.tissue.dev/" + goodKey + "\n", "https://mast.tissue.dev", ""},
		{"trailing slash", "https://mast.tissue.dev/" + goodKey + "/", "https://mast.tissue.dev", ""},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			channel, err := Open(tc.raw)
			if err != nil {
				t.Fatalf("Open: %v", err)
			}
			if channel.Origin != tc.origin {
				t.Errorf("origin = %q, want %q", channel.Origin, tc.origin)
			}
			if channel.Prefix != tc.prefix {
				t.Errorf("prefix = %q, want %q", channel.Prefix, tc.prefix)
			}
			if channel.Key != goodKey {
				t.Errorf("key = %q, want %q", channel.Key, goodKey)
			}
		})
	}
}

func TestOpenRefusesWhatIsNotAChannelURL(t *testing.T) {
	cases := []struct {
		name string
		raw  string
	}{
		{"nothing", "   "},
		{"no key", "https://mast.tissue.dev/m/"},
		{"wrong shape", "https://mast.tissue.dev/mk_short"},
		{"uppercase hex, which Mast does not mint", "mk_1C9F1C9F1C9F1C9F1C9F1C9F1C9F1C9F1C9F1C9F"},
		{"not an http scheme", "ftp://mast.tissue.dev/" + goodKey},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if _, err := Open(tc.raw); err == nil {
				t.Fatalf("Open(%q) returned no error", tc.raw)
			}
		})
	}
}

func TestChannelRebuildsItsURL(t *testing.T) {
	channel, err := Open("https://mast.tissue.dev/m/" + goodKey)
	if err != nil {
		t.Fatal(err)
	}
	if got, want := channel.URL(), "https://mast.tissue.dev/m/"+goodKey; got != want {
		t.Errorf("URL() = %q, want %q", got, want)
	}
}

func TestStringDoesNotCarryTheWholeKey(t *testing.T) {
	channel, err := Open(goodKey)
	if err != nil {
		t.Fatal(err)
	}
	described := channel.String()
	if strings.Contains(described, goodKey) {
		t.Fatalf("String() = %q, which gives the key away", described)
	}
	if !strings.Contains(described, "mk_1c9f") {
		t.Errorf("String() = %q, want the first few characters of the key", described)
	}
}
