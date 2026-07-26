package planista

import (
	"strings"
	"testing"
)

func TestLoadConfigDefaults(t *testing.T) {
	t.Parallel()

	cfg, err := loadConfig(mapEnv(map[string]string{
		"PLANISTA_BASE_URL": "https://plans.example.com/",
	}))
	if err != nil {
		t.Fatalf("load config: %v", err)
	}
	if cfg.BaseURL != "https://plans.example.com" {
		t.Fatalf("BaseURL = %q", cfg.BaseURL)
	}
	if cfg.ListenAddr != defaultListenAddr {
		t.Fatalf("ListenAddr = %q", cfg.ListenAddr)
	}
	if cfg.DBPath != defaultDBPath {
		t.Fatalf("DBPath = %q", cfg.DBPath)
	}
	if cfg.MaxPlanBytes != defaultMaxPlanBytes {
		t.Fatalf("MaxPlanBytes = %d", cfg.MaxPlanBytes)
	}
	if cfg.MaxPlans != defaultMaxPlans {
		t.Fatalf("MaxPlans = %d", cfg.MaxPlans)
	}
	if cfg.WipeInterval != defaultWipeInterval {
		t.Fatalf("WipeInterval = %s", cfg.WipeInterval)
	}
}

func TestLoadConfigOverrides(t *testing.T) {
	t.Parallel()

	cfg, err := loadConfig(mapEnv(map[string]string{
		"PLANISTA_BASE_URL":       "http://127.0.0.1:9090",
		"PLANISTA_LISTEN_ADDR":    "127.0.0.1:9090",
		"PLANISTA_DB_PATH":        "/tmp/custom.db",
		"PLANISTA_MAX_PLAN_BYTES": "2048",
		"PLANISTA_MAX_PLANS":      "25",
	}))
	if err != nil {
		t.Fatalf("load config: %v", err)
	}
	if cfg.ListenAddr != "127.0.0.1:9090" || cfg.DBPath != "/tmp/custom.db" {
		t.Fatalf("unexpected string overrides: %#v", cfg)
	}
	if cfg.MaxPlanBytes != 2048 || cfg.MaxPlans != 25 {
		t.Fatalf("unexpected numeric overrides: %#v", cfg)
	}
}

func TestLoadConfigRejectsInvalidValues(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name string
		env  map[string]string
		want string
	}{
		{name: "missing base URL", env: map[string]string{}, want: "required"},
		{name: "relative base URL", env: map[string]string{"PLANISTA_BASE_URL": "/plans"}, want: "absolute"},
		{name: "wrong scheme", env: map[string]string{"PLANISTA_BASE_URL": "ftp://example.com"}, want: "absolute"},
		{name: "credentials", env: map[string]string{"PLANISTA_BASE_URL": "https://user@example.com"}, want: "credentials"},
		{name: "query", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com?x=1"}, want: "query"},
		{name: "path", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com/plans"}, want: "path"},
		{name: "empty listen address", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com", "PLANISTA_LISTEN_ADDR": " "}, want: "LISTEN_ADDR"},
		{name: "empty database path", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com", "PLANISTA_DB_PATH": " "}, want: "DB_PATH"},
		{name: "zero bytes", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com", "PLANISTA_MAX_PLAN_BYTES": "0"}, want: "MAX_PLAN_BYTES"},
		{name: "bad bytes", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com", "PLANISTA_MAX_PLAN_BYTES": "many"}, want: "MAX_PLAN_BYTES"},
		{name: "zero plans", env: map[string]string{"PLANISTA_BASE_URL": "https://example.com", "PLANISTA_MAX_PLANS": "0"}, want: "MAX_PLANS"},
	}

	for _, test := range tests {
		test := test
		t.Run(test.name, func(t *testing.T) {
			t.Parallel()
			_, err := loadConfig(mapEnv(test.env))
			if err == nil || !strings.Contains(err.Error(), test.want) {
				t.Fatalf("error = %v, want substring %q", err, test.want)
			}
		})
	}
}

func mapEnv(values map[string]string) func(string) string {
	return func(key string) string {
		return values[key]
	}
}
