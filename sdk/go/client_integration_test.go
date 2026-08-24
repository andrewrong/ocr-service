package ocrclient

import (
	"context"
	"os"
	"testing"
	"time"
)

func TestLiveHealth(t *testing.T) {
	baseURL := os.Getenv("OCR_LIVE_TEST_URL")
	if baseURL == "" {
		t.Skip("set OCR_LIVE_TEST_URL to run against a deployed OCR service")
	}

	client, err := NewClient(baseURL, WithTimeout(10*time.Second))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()

	health, err := client.Health(ctx)
	if err != nil {
		t.Fatalf("health: %v", err)
	}
	if !health.Ready() {
		t.Fatalf("OCR service is not ready: %#v", health)
	}
}
