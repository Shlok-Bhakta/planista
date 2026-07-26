package planista

import (
	"bytes"
	"context"
	"log"
	"strings"
	"testing"
	"time"
)

func TestWiperRotatesAndLogsCommand(t *testing.T) {
	t.Parallel()

	var logs bytes.Buffer
	wiper := &Wiper{
		baseURL:  "https://plans.example.com",
		interval: 2 * time.Minute,
		random:   bytes.NewReader(bytes.Repeat([]byte{7}, wipeTokenBytes)),
		logger:   log.New(&logs, "", 0),
		now:      func() time.Time { return time.Unix(0, 0) },
	}
	if err := wiper.Rotate(); err != nil {
		t.Fatalf("rotate: %v", err)
	}
	if len(wiper.token) != wipeTokenLength || !wiper.Matches(wiper.token) {
		t.Fatalf("invalid active token %q", wiper.token)
	}
	if !strings.Contains(logs.String(), "curl -fsS -X POST 'https://plans.example.com/") {
		t.Fatalf("log does not contain wipe command: %q", logs.String())
	}
	if !strings.Contains(logs.String(), "1970-01-01T00:02:00Z") {
		t.Fatalf("log does not contain expiry: %q", logs.String())
	}
}

func TestWiperRunReplacesOldToken(t *testing.T) {
	t.Parallel()

	first := bytes.Repeat([]byte{1}, wipeTokenBytes)
	second := bytes.Repeat([]byte{2}, wipeTokenBytes)
	wiper := &Wiper{
		baseURL:  "https://plans.example.com",
		interval: 5 * time.Millisecond,
		random:   bytes.NewReader(append(first, second...)),
		logger:   log.New(&bytes.Buffer{}, "", 0),
		now:      time.Now,
	}
	if err := wiper.Rotate(); err != nil {
		t.Fatalf("first rotate: %v", err)
	}
	oldToken := wiper.token

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go wiper.Run(ctx)

	deadline := time.Now().Add(250 * time.Millisecond)
	for wiper.Matches(oldToken) && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	if wiper.Matches(oldToken) {
		t.Fatal("old token remained active after rotation")
	}
}
