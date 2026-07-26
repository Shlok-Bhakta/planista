package planista

import (
	"fmt"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
)

const (
	defaultListenAddr   = ":8080"
	defaultDBPath       = "/data/planista.db"
	defaultMaxPlanBytes = int64(1 << 20)
	defaultMaxPlans     = 1000
	defaultWipeInterval = 2 * time.Minute
)

// Config contains all runtime configuration for a Planista server.
type Config struct {
	BaseURL      string
	ListenAddr   string
	DBPath       string
	MaxPlanBytes int64
	MaxPlans     int
	WipeInterval time.Duration
}

// LoadConfig reads and validates configuration from the process environment.
func LoadConfig() (Config, error) {
	return loadConfig(os.Getenv)
}

func loadConfig(getenv func(string) string) (Config, error) {
	baseURL, err := normalizeBaseURL(getenv("PLANISTA_BASE_URL"))
	if err != nil {
		return Config{}, err
	}

	cfg := Config{
		BaseURL:      baseURL,
		ListenAddr:   valueOrDefault(getenv("PLANISTA_LISTEN_ADDR"), defaultListenAddr),
		DBPath:       valueOrDefault(getenv("PLANISTA_DB_PATH"), defaultDBPath),
		MaxPlanBytes: defaultMaxPlanBytes,
		MaxPlans:     defaultMaxPlans,
		WipeInterval: defaultWipeInterval,
	}

	if value := getenv("PLANISTA_MAX_PLAN_BYTES"); value != "" {
		cfg.MaxPlanBytes, err = strconv.ParseInt(value, 10, 64)
		if err != nil || cfg.MaxPlanBytes <= 0 {
			return Config{}, fmt.Errorf("PLANISTA_MAX_PLAN_BYTES must be a positive integer")
		}
	}
	if value := getenv("PLANISTA_MAX_PLANS"); value != "" {
		cfg.MaxPlans, err = strconv.Atoi(value)
		if err != nil || cfg.MaxPlans <= 0 {
			return Config{}, fmt.Errorf("PLANISTA_MAX_PLANS must be a positive integer")
		}
	}
	if strings.TrimSpace(cfg.ListenAddr) == "" {
		return Config{}, fmt.Errorf("PLANISTA_LISTEN_ADDR must not be empty")
	}
	if strings.TrimSpace(cfg.DBPath) == "" {
		return Config{}, fmt.Errorf("PLANISTA_DB_PATH must not be empty")
	}

	return cfg, nil
}

func normalizeBaseURL(raw string) (string, error) {
	if raw == "" {
		return "", fmt.Errorf("PLANISTA_BASE_URL is required")
	}
	parsed, err := url.Parse(raw)
	if err != nil {
		return "", fmt.Errorf("parse PLANISTA_BASE_URL: %w", err)
	}
	if (parsed.Scheme != "http" && parsed.Scheme != "https") || parsed.Host == "" {
		return "", fmt.Errorf("PLANISTA_BASE_URL must be an absolute http or https URL")
	}
	if parsed.User != nil || parsed.RawQuery != "" || parsed.Fragment != "" {
		return "", fmt.Errorf("PLANISTA_BASE_URL must not contain credentials, a query, or a fragment")
	}
	if parsed.Path != "" && parsed.Path != "/" {
		return "", fmt.Errorf("PLANISTA_BASE_URL must not contain a path")
	}

	parsed.Path = ""
	return strings.TrimSuffix(parsed.String(), "/"), nil
}

func valueOrDefault(value, fallback string) string {
	if value == "" {
		return fallback
	}
	return value
}
