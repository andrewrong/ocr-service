package ocrclient

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func TestRecognizeImageSelectsVersionedRouteAndParsesResult(t *testing.T) {
	t.Parallel()

	image := writeTestFile(t, "receipt.png", []byte("\x89PNG\r\n\x1a\nimage"))
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if request.URL.Path != "/v1/ocr/image" {
			t.Errorf("path = %q, want /v1/ocr/image", request.URL.Path)
		}
		if err := request.ParseMultipartForm(1 << 20); err != nil {
			t.Fatalf("parse multipart: %v", err)
		}
		if engine := request.FormValue("engine"); engine != "auto" {
			t.Errorf("engine = %q, want auto", engine)
		}
		if pages := request.FormValue("page_range"); pages != "" {
			t.Errorf("page_range = %q, want empty", pages)
		}
		file, _, err := request.FormFile("file")
		if err != nil {
			t.Fatalf("uploaded file: %v", err)
		}
		defer file.Close()

		response.Header().Set("Content-Type", "application/json")
		fmt.Fprint(response, `{"markdown":"# Receipt","engine":"paddle","pages":1,"duration_ms":321}`)
	}))
	t.Cleanup(server.Close)

	client, err := NewClient(server.URL, WithHTTPClient(server.Client()))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	result, err := client.Recognize(context.Background(), image, nil)
	if err != nil {
		t.Fatalf("recognize: %v", err)
	}

	if result.Markdown != "# Receipt" || result.Engine != "paddle" {
		t.Errorf("result = %#v", result)
	}
	if result.Pages != 1 || result.DurationMS != 321 {
		t.Errorf("metadata = %#v", result)
	}
}

func TestRecognizePDFDetectsMagicAndSendsPageRange(t *testing.T) {
	t.Parallel()

	document := writeTestFile(t, "scan.bin", []byte("%PDF-1.7\ncontent"))
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if request.URL.Path != "/v1/ocr/pdf" {
			t.Errorf("path = %q, want /v1/ocr/pdf", request.URL.Path)
		}
		if err := request.ParseMultipartForm(1 << 20); err != nil {
			t.Fatalf("parse multipart: %v", err)
		}
		if pages := request.FormValue("page_range"); pages != "2-7" {
			t.Errorf("page_range = %q, want 2-7", pages)
		}
		if engine := request.FormValue("engine"); engine != "glm" {
			t.Errorf("engine = %q, want glm", engine)
		}

		response.Header().Set("Content-Type", "application/json")
		fmt.Fprint(response, `{"markdown":"## Page 2","engine":"mixed","pages":6,"duration_ms":12000}`)
	}))
	t.Cleanup(server.Close)

	client, err := NewClient(server.URL, WithHTTPClient(server.Client()))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	result, err := client.Recognize(context.Background(), document, &RecognizeOptions{
		Engine: EngineGLM,
		Pages:  "2-7",
	})
	if err != nil {
		t.Fatalf("recognize: %v", err)
	}
	if result.Engine != "mixed" || result.Pages != 6 {
		t.Errorf("result = %#v", result)
	}
}

func TestRecognizeFallsBackToLegacyRoute(t *testing.T) {
	t.Parallel()

	image := writeTestFile(t, "photo.png", []byte("png"))
	var paths []string
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		paths = append(paths, request.URL.Path)
		if request.URL.Path == "/v1/ocr/image" {
			http.NotFound(response, request)
			return
		}
		response.Header().Set("Content-Type", "application/json")
		fmt.Fprint(response, `{"markdown":"legacy","engine":"glm","pages":1,"duration_ms":100}`)
	}))
	t.Cleanup(server.Close)

	client, err := NewClient(server.URL, WithHTTPClient(server.Client()))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	result, err := client.Recognize(context.Background(), image, nil)
	if err != nil {
		t.Fatalf("recognize: %v", err)
	}

	if !reflect.DeepEqual(paths, []string{"/v1/ocr/image", "/ocr/image"}) {
		t.Errorf("paths = %#v", paths)
	}
	if result.Markdown != "legacy" {
		t.Errorf("markdown = %q", result.Markdown)
	}
}

func TestRecognizeReturnsTypedServiceError(t *testing.T) {
	t.Parallel()

	image := writeTestFile(t, "photo.png", []byte("png"))
	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		response.Header().Set("Content-Type", "application/json")
		response.WriteHeader(http.StatusBadGateway)
		fmt.Fprint(response, `{"error":"all OCR engines timed out"}`)
	}))
	t.Cleanup(server.Close)

	client, err := NewClient(server.URL, WithHTTPClient(server.Client()))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	_, err = client.Recognize(context.Background(), image, nil)
	var serviceError *ServiceError
	if !errors.As(err, &serviceError) {
		t.Fatalf("error = %T %v, want *ServiceError", err, err)
	}
	if serviceError.StatusCode != http.StatusBadGateway {
		t.Errorf("status = %d", serviceError.StatusCode)
	}
	if serviceError.Message != "all OCR engines timed out" {
		t.Errorf("message = %q", serviceError.Message)
	}
}

func TestHealthReturnsDegradedModelAvailability(t *testing.T) {
	t.Parallel()

	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		response.Header().Set("Content-Type", "application/json")
		response.WriteHeader(http.StatusServiceUnavailable)
		fmt.Fprint(response, `{"status":"degraded","backend":"lmstudio","backend_ready":false,"ollama":false,"models":[{"engine":"paddle","name":"paddle-model","available":false}]}`)
	}))
	t.Cleanup(server.Close)

	client, err := NewClient(server.URL, WithHTTPClient(server.Client()))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	health, err := client.Health(context.Background())
	if err != nil {
		t.Fatalf("health: %v", err)
	}
	if health.Ready() {
		t.Fatal("degraded health should not be ready")
	}
	if len(health.Models) != 1 || health.Models[0].Engine != "paddle" {
		t.Errorf("models = %#v", health.Models)
	}
}

func TestHealthAcceptsLegacyOllamaReadiness(t *testing.T) {
	t.Parallel()

	server := httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		response.Header().Set("Content-Type", "application/json")
		fmt.Fprint(response, `{"status":"ok","ollama":true,"models":[{"engine":"qwen","name":"qwen-model","available":true}]}`)
	}))
	t.Cleanup(server.Close)

	client, err := NewClient(server.URL, WithHTTPClient(server.Client()))
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	health, err := client.Health(context.Background())
	if err != nil {
		t.Fatalf("health: %v", err)
	}
	if !health.Ready() {
		t.Fatal("legacy Ollama health should remain ready")
	}
}

func TestRecognizeRejectsPagesForImageBeforeRequest(t *testing.T) {
	t.Parallel()

	image := writeTestFile(t, "photo.jpg", []byte("jpeg"))
	client, err := NewClient("http://ocr.test")
	if err != nil {
		t.Fatalf("new client: %v", err)
	}
	_, err = client.Recognize(context.Background(), image, &RecognizeOptions{Pages: "1-2"})
	if err == nil {
		t.Fatal("recognize should reject pages for an image")
	}
}

func writeTestFile(t *testing.T, name string, data []byte) string {
	t.Helper()

	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("write test file: %v", err)
	}
	return path
}
