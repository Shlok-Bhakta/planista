package planista

import (
	"bytes"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestPlanLifecycle(t *testing.T) {
	t.Parallel()

	server, _, _ := newTestServer(t, 1024, 10)
	html := "<!doctype html><script>document.body.textContent='active'</script>"

	created := upload(t, server, html, "text/html; charset=utf-8")
	if created.Code != http.StatusCreated {
		t.Fatalf("create status = %d, body = %q", created.Code, created.Body.String())
	}
	permalink := strings.TrimSpace(created.Body.String())
	if created.Header().Get("Location") != permalink {
		t.Fatalf("Location = %q, body = %q", created.Header().Get("Location"), permalink)
	}
	id := strings.TrimPrefix(permalink, "https://plans.example.com/")
	if len(id) != planIDLength || !isBase64URL(id) {
		t.Fatalf("invalid plan ID %q", id)
	}

	got := request(server, http.MethodGet, "/"+id, "", "")
	if got.Code != http.StatusOK {
		t.Fatalf("get status = %d, body = %q", got.Code, got.Body.String())
	}
	if got.Body.String() != html {
		t.Fatalf("retrieved HTML = %q", got.Body.String())
	}
	if got.Header().Get("Content-Type") != "text/html; charset=utf-8" {
		t.Fatalf("Content-Type = %q", got.Header().Get("Content-Type"))
	}
	if got.Header().Get("Content-Security-Policy") != "frame-ancestors 'none'" {
		t.Fatalf("CSP = %q", got.Header().Get("Content-Security-Policy"))
	}
	if got.Header().Get("Cache-Control") != "no-store" {
		t.Fatalf("Cache-Control = %q", got.Header().Get("Cache-Control"))
	}

	head := request(server, http.MethodHead, "/"+id, "", "")
	if head.Code != http.StatusOK || head.Body.Len() != 0 {
		t.Fatalf("head status = %d, body = %q", head.Code, head.Body.String())
	}
	if head.Header().Get("Content-Length") != fmt.Sprint(len(html)) {
		t.Fatalf("head Content-Length = %q", head.Header().Get("Content-Length"))
	}

	duplicate := upload(t, server, html, "text/html")
	if duplicate.Code != http.StatusCreated {
		t.Fatalf("duplicate status = %d", duplicate.Code)
	}
	if duplicate.Body.String() == created.Body.String() {
		t.Fatal("duplicate upload reused a permalink")
	}
}

func TestUploadValidationAndMethods(t *testing.T) {
	t.Parallel()

	server, _, _ := newTestServer(t, 4, 1)
	tests := []struct {
		name        string
		method      string
		path        string
		body        string
		contentType string
		want        int
	}{
		{name: "wrong content type", method: http.MethodPost, path: "/", body: "html", contentType: "text/plain", want: http.StatusUnsupportedMediaType},
		{name: "missing content type", method: http.MethodPost, path: "/", body: "html", want: http.StatusUnsupportedMediaType},
		{name: "empty", method: http.MethodPost, path: "/", contentType: "text/html", want: http.StatusBadRequest},
		{name: "too large", method: http.MethodPost, path: "/", body: "12345", contentType: "text/html", want: http.StatusRequestEntityTooLarge},
		{name: "root method", method: http.MethodDelete, path: "/", want: http.StatusMethodNotAllowed},
		{name: "health method", method: http.MethodPost, path: "/healthz", want: http.StatusMethodNotAllowed},
		{name: "nested path", method: http.MethodGet, path: "/a/b", want: http.StatusNotFound},
		{name: "unknown", method: http.MethodGet, path: "/abcdefghijklmnop", want: http.StatusNotFound},
	}
	for _, test := range tests {
		test := test
		t.Run(test.name, func(t *testing.T) {
			t.Parallel()
			got := request(server, test.method, test.path, test.body, test.contentType)
			if got.Code != test.want {
				t.Fatalf("status = %d, want %d, body = %q", got.Code, test.want, got.Body.String())
			}
		})
	}

	first := upload(t, server, "1234", "text/html")
	if first.Code != http.StatusCreated {
		t.Fatalf("first upload status = %d", first.Code)
	}
	full := upload(t, server, "1234", "text/html")
	if full.Code != http.StatusInsufficientStorage {
		t.Fatalf("full upload status = %d, body = %q", full.Code, full.Body.String())
	}
}

func TestWipeEndpointAndTokenInvalidation(t *testing.T) {
	t.Parallel()

	server, store, wiper := newTestServer(t, 1024, 10)
	created := upload(t, server, "<p>erase me</p>", "text/html")
	id := strings.TrimPrefix(strings.TrimSpace(created.Body.String()), "https://plans.example.com/")
	oldToken := wiper.token

	invalid := request(server, http.MethodPost, "/"+strings.Repeat("x", wipeTokenLength), "", "")
	if invalid.Code != http.StatusNotFound {
		t.Fatalf("invalid wipe status = %d", invalid.Code)
	}
	wiper.random = bytes.NewReader(bytes.Repeat([]byte{9}, wipeTokenBytes))
	if err := wiper.Rotate(); err != nil {
		t.Fatalf("rotate token: %v", err)
	}
	if got := request(server, http.MethodPost, "/"+oldToken, "", ""); got.Code != http.StatusNotFound {
		t.Fatalf("expired wipe status = %d", got.Code)
	}
	if got := request(server, http.MethodGet, "/"+wiper.token, "", ""); got.Code != http.StatusNotFound {
		t.Fatalf("GET wipe status = %d", got.Code)
	}
	if got := request(server, http.MethodPost, "/"+wiper.token, "", ""); got.Code != http.StatusNoContent {
		t.Fatalf("wipe status = %d, body = %q", got.Code, got.Body.String())
	}
	if _, err := store.Get(t.Context(), id); err != ErrNotFound {
		t.Fatalf("get wiped plan error = %v", err)
	}
}

func TestIDCollisionRetries(t *testing.T) {
	t.Parallel()

	server, _, _ := newTestServer(t, 1024, 10)
	zeroID := bytes.Repeat([]byte{0}, planIDBytes)
	oneID := bytes.Repeat([]byte{1}, planIDBytes)
	server.random = bytes.NewReader(append(append(zeroID, zeroID...), oneID...))

	first := upload(t, server, "first", "text/html")
	second := upload(t, server, "second", "text/html")
	if first.Code != http.StatusCreated || second.Code != http.StatusCreated {
		t.Fatalf("statuses = %d, %d", first.Code, second.Code)
	}
	if first.Body.String() == second.Body.String() {
		t.Fatal("collision retry did not allocate a new ID")
	}
}

func TestConcurrentUploadsRespectCapacity(t *testing.T) {
	t.Parallel()

	const (
		capacity = 10
		requests = 30
	)
	server, _, _ := newTestServer(t, 1024, capacity)
	var created atomic.Int32
	var full atomic.Int32
	var unexpected atomic.Int32
	var wg sync.WaitGroup

	for index := 0; index < requests; index++ {
		wg.Add(1)
		go func(index int) {
			defer wg.Done()
			response := upload(t, server, fmt.Sprintf("<p>%d</p>", index), "text/html")
			switch response.Code {
			case http.StatusCreated:
				created.Add(1)
			case http.StatusInsufficientStorage:
				full.Add(1)
			default:
				unexpected.Add(1)
			}
		}(index)
	}
	wg.Wait()

	if created.Load() != capacity || full.Load() != requests-capacity || unexpected.Load() != 0 {
		t.Fatalf("created=%d full=%d unexpected=%d", created.Load(), full.Load(), unexpected.Load())
	}
}

func TestHealthAndLandingPage(t *testing.T) {
	t.Parallel()

	server, _, _ := newTestServer(t, 1024, 10)
	health := request(server, http.MethodGet, "/healthz", "", "")
	if health.Code != http.StatusOK || health.Body.String() != "ok\n" {
		t.Fatalf("health = %d %q", health.Code, health.Body.String())
	}
	landing := request(server, http.MethodGet, "/", "", "")
	if landing.Code != http.StatusOK || !strings.Contains(landing.Body.String(), "Post an HTML document") {
		t.Fatalf("landing = %d %q", landing.Code, landing.Body.String())
	}
}

func newTestServer(t *testing.T, maxBytes int64, maxPlans int) (*Server, *Store, *Wiper) {
	t.Helper()

	store, err := OpenStore(filepath.Join(t.TempDir(), "planista.db"), maxPlans)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { store.Close() })

	logger := log.New(io.Discard, "", 0)
	wiper, err := NewWiper("https://plans.example.com", 2*time.Minute, logger)
	if err != nil {
		t.Fatalf("create wiper: %v", err)
	}
	cfg := Config{
		BaseURL:      "https://plans.example.com",
		MaxPlanBytes: maxBytes,
		MaxPlans:     maxPlans,
		WipeInterval: 2 * time.Minute,
	}
	return NewServer(cfg, store, wiper, logger), store, wiper
}

func upload(t *testing.T, handler http.Handler, body, contentType string) *httptest.ResponseRecorder {
	t.Helper()
	return request(handler, http.MethodPost, "/", body, contentType)
}

func request(handler http.Handler, method, path, body, contentType string) *httptest.ResponseRecorder {
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	response := httptest.NewRecorder()
	handler.ServeHTTP(response, req)
	return response
}
