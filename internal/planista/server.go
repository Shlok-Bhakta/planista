package planista

import (
	"crypto/rand"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"log"
	"mime"
	"net/http"
	"strings"
)

const (
	planIDBytes     = 12
	maxIDAttempts   = 5
	planIDLength    = 16
	wipeTokenLength = 32
)

// Server implements Planista's HTTP API.
type Server struct {
	config Config
	store  *Store
	wiper  *Wiper
	random io.Reader
	logger *log.Logger
}

// NewServer constructs a Planista HTTP handler.
func NewServer(config Config, store *Store, wiper *Wiper, logger *log.Logger) *Server {
	return &Server{
		config: config,
		store:  store,
		wiper:  wiper,
		random: rand.Reader,
		logger: logger,
	}
}

// ServeHTTP dispatches Planista's deliberately small API.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.URL.RawPath != "" || strings.Contains(strings.TrimPrefix(r.URL.Path, "/"), "/") {
		notFound(w)
		return
	}

	segment := strings.TrimPrefix(r.URL.Path, "/")
	switch {
	case segment == "":
		s.handleRoot(w, r)
	case segment == "healthz":
		s.handleHealth(w, r)
	case len(segment) == planIDLength && isBase64URL(segment):
		s.handlePlan(w, r, segment)
	case len(segment) == wipeTokenLength && r.Method == http.MethodPost:
		s.handleWipe(w, r, segment)
	default:
		notFound(w)
	}
}

func (s *Server) handleRoot(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Header().Set("Cache-Control", "no-store")
		_, _ = io.WriteString(w, landingPage)
	case http.MethodPost:
		s.handleCreate(w, r)
	default:
		methodNotAllowed(w, http.MethodGet+", "+http.MethodPost)
	}
}

func (s *Server) handleCreate(w http.ResponseWriter, r *http.Request) {
	mediaType, _, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
	if err != nil || !strings.EqualFold(mediaType, "text/html") {
		http.Error(w, "Content-Type must be text/html", http.StatusUnsupportedMediaType)
		return
	}

	r.Body = http.MaxBytesReader(w, r.Body, s.config.MaxPlanBytes)
	html, err := io.ReadAll(r.Body)
	if err != nil {
		var tooLarge *http.MaxBytesError
		if errors.As(err, &tooLarge) {
			http.Error(w, "plan exceeds maximum size", http.StatusRequestEntityTooLarge)
			return
		}
		http.Error(w, "could not read request body", http.StatusBadRequest)
		return
	}
	if len(html) == 0 {
		http.Error(w, "plan must not be empty", http.StatusBadRequest)
		return
	}

	var id string
	for attempt := 0; attempt < maxIDAttempts; attempt++ {
		id, err = randomID(s.random)
		if err != nil {
			s.logger.Printf("generate plan ID: %v", err)
			http.Error(w, "could not create plan", http.StatusInternalServerError)
			return
		}
		err = s.store.Create(r.Context(), id, html)
		if errors.Is(err, ErrIDCollision) {
			continue
		}
		break
	}
	switch {
	case errors.Is(err, ErrAtCapacity):
		http.Error(w, "plan limit reached", http.StatusInsufficientStorage)
		return
	case errors.Is(err, ErrIDCollision):
		s.logger.Printf("could not allocate a unique plan ID after %d attempts", maxIDAttempts)
		http.Error(w, "could not create plan", http.StatusInternalServerError)
		return
	case err != nil:
		s.logger.Printf("store plan: %v", err)
		http.Error(w, "could not create plan", http.StatusInternalServerError)
		return
	}

	permalink := s.config.BaseURL + "/" + id
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Location", permalink)
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(http.StatusCreated)
	_, _ = fmt.Fprintln(w, permalink)
}

func (s *Server) handlePlan(w http.ResponseWriter, r *http.Request, id string) {
	if r.Method != http.MethodGet && r.Method != http.MethodHead {
		methodNotAllowed(w, http.MethodGet+", "+http.MethodHead)
		return
	}
	html, err := s.store.Get(r.Context(), id)
	if errors.Is(err, ErrNotFound) {
		notFound(w)
		return
	}
	if err != nil {
		s.logger.Printf("get plan: %v", err)
		http.Error(w, "could not retrieve plan", http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Content-Length", fmt.Sprintf("%d", len(html)))
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Content-Security-Policy", "frame-ancestors 'none'")
	w.Header().Set("Referrer-Policy", "no-referrer")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	if r.Method == http.MethodGet {
		_, _ = w.Write(html)
	}
}

func (s *Server) handleHealth(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		methodNotAllowed(w, http.MethodGet)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	if err := s.store.Health(r.Context()); err != nil {
		s.logger.Printf("health check: %v", err)
		http.Error(w, "unhealthy", http.StatusServiceUnavailable)
		return
	}
	_, _ = io.WriteString(w, "ok\n")
}

func (s *Server) handleWipe(w http.ResponseWriter, r *http.Request, token string) {
	if !s.wiper.Matches(token) {
		notFound(w)
		return
	}
	if err := s.store.Wipe(r.Context()); err != nil {
		s.logger.Printf("wipe plans: %v", err)
		http.Error(w, "could not wipe plans", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func randomID(source io.Reader) (string, error) {
	raw := make([]byte, planIDBytes)
	if _, err := io.ReadFull(source, raw); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(raw), nil
}

func isBase64URL(value string) bool {
	for _, char := range value {
		if (char >= 'a' && char <= 'z') ||
			(char >= 'A' && char <= 'Z') ||
			(char >= '0' && char <= '9') ||
			char == '-' || char == '_' {
			continue
		}
		return false
	}
	return true
}

func methodNotAllowed(w http.ResponseWriter, allow string) {
	w.Header().Set("Allow", allow)
	http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
}

func notFound(w http.ResponseWriter) {
	http.Error(w, "not found", http.StatusNotFound)
}

const landingPage = `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Planista</title>
<style>
body{font:16px/1.5 system-ui,sans-serif;max-width:48rem;margin:4rem auto;padding:0 1rem;color:#202124}
code,pre{font-family:ui-monospace,monospace}pre{padding:1rem;background:#f4f4f5;overflow:auto}
</style>
<h1>Planista</h1>
<p>Post an HTML document. Get a short public permalink.</p>
<pre>curl --fail-with-body -H 'Content-Type: text/html' --data-binary @plan.html THIS_ORIGIN/</pre>
<p>Uploads are public, active HTML and are retained until an administrator wipes the server.</p>
</html>
`
