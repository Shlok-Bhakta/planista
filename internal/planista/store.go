package planista

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	_ "modernc.org/sqlite"
)

var (
	ErrNotFound    = errors.New("plan not found")
	ErrAtCapacity  = errors.New("plan limit reached")
	ErrIDCollision = errors.New("plan id collision")
)

// Store persists plans in SQLite.
type Store struct {
	db       *sql.DB
	maxPlans int
}

// OpenStore opens a SQLite database and applies the current schema.
func OpenStore(path string, maxPlans int) (*Store, error) {
	parent := filepath.Dir(path)
	if parent != "." {
		if err := os.MkdirAll(parent, 0o755); err != nil {
			return nil, fmt.Errorf("create database directory: %w", err)
		}
	}

	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open database: %w", err)
	}
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)

	store := &Store{db: db, maxPlans: maxPlans}
	if err := store.initialize(context.Background()); err != nil {
		db.Close()
		return nil, err
	}
	return store, nil
}

func (s *Store) initialize(ctx context.Context) error {
	statements := []string{
		"PRAGMA busy_timeout = 5000",
		"PRAGMA journal_mode = WAL",
		"PRAGMA synchronous = NORMAL",
		`CREATE TABLE IF NOT EXISTS plans (
			id TEXT PRIMARY KEY,
			html BLOB NOT NULL,
			created_at INTEGER NOT NULL
		)`,
	}
	for _, statement := range statements {
		if _, err := s.db.ExecContext(ctx, statement); err != nil {
			return fmt.Errorf("initialize database: %w", err)
		}
	}
	return nil
}

// Create stores a plan if capacity remains and the ID is unused.
func (s *Store) Create(ctx context.Context, id string, html []byte) error {
	tx, err := s.db.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin create transaction: %w", err)
	}
	defer tx.Rollback()

	var count int
	if err := tx.QueryRowContext(ctx, "SELECT COUNT(*) FROM plans").Scan(&count); err != nil {
		return fmt.Errorf("count plans: %w", err)
	}
	if count >= s.maxPlans {
		return ErrAtCapacity
	}

	result, err := tx.ExecContext(
		ctx,
		"INSERT OR IGNORE INTO plans (id, html, created_at) VALUES (?, ?, ?)",
		id,
		html,
		time.Now().Unix(),
	)
	if err != nil {
		return fmt.Errorf("insert plan: %w", err)
	}
	rows, err := result.RowsAffected()
	if err != nil {
		return fmt.Errorf("read insert result: %w", err)
	}
	if rows == 0 {
		return ErrIDCollision
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit plan: %w", err)
	}
	return nil
}

// Get returns the exact HTML bytes for an ID.
func (s *Store) Get(ctx context.Context, id string) ([]byte, error) {
	var html []byte
	if err := s.db.QueryRowContext(ctx, "SELECT html FROM plans WHERE id = ?", id).Scan(&html); err != nil {
		if errors.Is(err, sql.ErrNoRows) {
			return nil, ErrNotFound
		}
		return nil, fmt.Errorf("get plan: %w", err)
	}
	return html, nil
}

// Wipe deletes every plan and reclaims unused database space.
func (s *Store) Wipe(ctx context.Context) error {
	if _, err := s.db.ExecContext(ctx, "DELETE FROM plans"); err != nil {
		return fmt.Errorf("delete plans: %w", err)
	}
	if _, err := s.db.ExecContext(ctx, "PRAGMA wal_checkpoint(TRUNCATE)"); err != nil {
		return fmt.Errorf("checkpoint database: %w", err)
	}
	if _, err := s.db.ExecContext(ctx, "VACUUM"); err != nil {
		return fmt.Errorf("vacuum database: %w", err)
	}
	return nil
}

// Health checks that the database can answer a trivial query.
func (s *Store) Health(ctx context.Context) error {
	var one int
	if err := s.db.QueryRowContext(ctx, "SELECT 1").Scan(&one); err != nil {
		return fmt.Errorf("check database: %w", err)
	}
	return nil
}

// Close closes the underlying database.
func (s *Store) Close() error {
	return s.db.Close()
}
