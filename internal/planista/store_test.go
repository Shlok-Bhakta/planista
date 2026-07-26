package planista

import (
	"context"
	"errors"
	"path/filepath"
	"testing"
)

func TestStorePersistsAndWipes(t *testing.T) {
	t.Parallel()

	path := filepath.Join(t.TempDir(), "nested", "planista.db")
	store, err := OpenStore(path, 2)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	if err := store.Create(context.Background(), "abcdefghijklmnop", []byte("<h1>persisted</h1>")); err != nil {
		t.Fatalf("create plan: %v", err)
	}
	if err := store.Close(); err != nil {
		t.Fatalf("close store: %v", err)
	}

	store, err = OpenStore(path, 2)
	if err != nil {
		t.Fatalf("reopen store: %v", err)
	}
	t.Cleanup(func() { store.Close() })

	html, err := store.Get(context.Background(), "abcdefghijklmnop")
	if err != nil {
		t.Fatalf("get persisted plan: %v", err)
	}
	if string(html) != "<h1>persisted</h1>" {
		t.Fatalf("html = %q", html)
	}

	if err := store.Wipe(context.Background()); err != nil {
		t.Fatalf("wipe store: %v", err)
	}
	if _, err := store.Get(context.Background(), "abcdefghijklmnop"); !errors.Is(err, ErrNotFound) {
		t.Fatalf("get after wipe error = %v", err)
	}
	if err := store.Health(context.Background()); err != nil {
		t.Fatalf("health after wipe: %v", err)
	}
}

func TestStoreCapacityAndCollision(t *testing.T) {
	t.Parallel()

	store, err := OpenStore(filepath.Join(t.TempDir(), "planista.db"), 1)
	if err != nil {
		t.Fatalf("open store: %v", err)
	}
	t.Cleanup(func() { store.Close() })

	if err := store.Create(context.Background(), "abcdefghijklmnop", []byte("first")); err != nil {
		t.Fatalf("create first: %v", err)
	}
	if err := store.Create(context.Background(), "differentIDvalue", []byte("second")); !errors.Is(err, ErrAtCapacity) {
		t.Fatalf("capacity error = %v", err)
	}

	collisionStore, err := OpenStore(filepath.Join(t.TempDir(), "collision.db"), 2)
	if err != nil {
		t.Fatalf("open collision store: %v", err)
	}
	t.Cleanup(func() { collisionStore.Close() })
	if err := collisionStore.Create(context.Background(), "abcdefghijklmnop", []byte("first")); err != nil {
		t.Fatalf("seed collision: %v", err)
	}
	if err := collisionStore.Create(context.Background(), "abcdefghijklmnop", []byte("second")); !errors.Is(err, ErrIDCollision) {
		t.Fatalf("collision error = %v", err)
	}
}
