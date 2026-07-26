package main

import (
	"context"
	"errors"
	"log"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/Shlok-Bhakta/planista/internal/planista"
)

func main() {
	logger := log.New(os.Stdout, "", log.LstdFlags|log.LUTC)

	config, err := planista.LoadConfig()
	if err != nil {
		logger.Fatalf("configuration: %v", err)
	}
	store, err := planista.OpenStore(config.DBPath, config.MaxPlans)
	if err != nil {
		logger.Fatalf("database: %v", err)
	}
	defer store.Close()

	wiper, err := planista.NewWiper(config.BaseURL, config.WipeInterval, logger)
	if err != nil {
		logger.Fatalf("wipe URL: %v", err)
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	go wiper.Run(ctx)

	httpServer := &http.Server{
		Addr:              config.ListenAddr,
		Handler:           planista.NewServer(config, store, wiper, logger),
		ReadHeaderTimeout: 5 * time.Second,
		ReadTimeout:       15 * time.Second,
		WriteTimeout:      30 * time.Second,
		IdleTimeout:       60 * time.Second,
	}

	errs := make(chan error, 1)
	go func() {
		logger.Printf("listening on %s", config.ListenAddr)
		errs <- httpServer.ListenAndServe()
	}()

	select {
	case <-ctx.Done():
		logger.Print("shutting down")
	case err := <-errs:
		if !errors.Is(err, http.ErrServerClosed) {
			logger.Fatalf("serve: %v", err)
		}
		return
	}

	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := httpServer.Shutdown(shutdownCtx); err != nil {
		logger.Printf("shutdown: %v", err)
	}
}
