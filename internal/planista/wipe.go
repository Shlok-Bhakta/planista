package planista

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"fmt"
	"io"
	"log"
	"sync"
	"time"
)

const wipeTokenBytes = 24

// Wiper owns the short-lived global wipe token.
type Wiper struct {
	mu       sync.RWMutex
	token    string
	baseURL  string
	interval time.Duration
	random   io.Reader
	logger   *log.Logger
	now      func() time.Time
}

// NewWiper creates and logs the first wipe token.
func NewWiper(baseURL string, interval time.Duration, logger *log.Logger) (*Wiper, error) {
	wiper := &Wiper{
		baseURL:  baseURL,
		interval: interval,
		random:   rand.Reader,
		logger:   logger,
		now:      time.Now,
	}
	if err := wiper.Rotate(); err != nil {
		return nil, err
	}
	return wiper, nil
}

// Run rotates the wipe token until the context is cancelled.
func (w *Wiper) Run(ctx context.Context) {
	ticker := time.NewTicker(w.interval)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if err := w.Rotate(); err != nil {
				w.logger.Printf("could not rotate wipe URL: %v", err)
			}
		}
	}
}

// Rotate replaces the active token and logs a directly executable command.
func (w *Wiper) Rotate() error {
	raw := make([]byte, wipeTokenBytes)
	if _, err := io.ReadFull(w.random, raw); err != nil {
		return fmt.Errorf("generate wipe token: %w", err)
	}
	token := base64.RawURLEncoding.EncodeToString(raw)
	expires := w.now().Add(w.interval)

	w.mu.Lock()
	w.token = token
	w.mu.Unlock()

	w.logger.Printf(
		"PLANISTA WIPE (valid until %s): curl -fsS -X POST '%s/%s'",
		expires.UTC().Format(time.RFC3339),
		w.baseURL,
		token,
	)
	return nil
}

// Matches reports whether candidate is the currently active token.
func (w *Wiper) Matches(candidate string) bool {
	w.mu.RLock()
	token := w.token
	w.mu.RUnlock()
	return subtle.ConstantTimeCompare([]byte(candidate), []byte(token)) == 1
}
