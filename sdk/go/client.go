package ocrclient

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"
)

const (
	defaultBaseURL = "http://127.0.0.1:18100"
	defaultTimeout = 15 * time.Minute
	maxJSONBytes   = 2 << 20
)

var pageRangePattern = regexp.MustCompile(`^[1-9][0-9]*(?:-[1-9][0-9]*)?$`)

// Client recognizes files through the OCR service HTTP interface.
type Client struct {
	baseURL    string
	httpClient *http.Client
}

type clientConfig struct {
	httpClient *http.Client
	timeout    time.Duration
	timeoutSet bool
}

// ClientOption customizes a Client.
type ClientOption func(*clientConfig) error

// WithHTTPClient injects an HTTP client. The caller retains ownership of it.
func WithHTTPClient(httpClient *http.Client) ClientOption {
	return func(config *clientConfig) error {
		if httpClient == nil {
			return errors.New("http client cannot be nil")
		}
		config.httpClient = httpClient
		return nil
	}
}

// WithTimeout configures the overall timeout. It clones an injected HTTP client
// before changing its timeout.
func WithTimeout(timeout time.Duration) ClientOption {
	return func(config *clientConfig) error {
		if timeout <= 0 {
			return errors.New("timeout must be greater than zero")
		}
		config.timeout = timeout
		config.timeoutSet = true
		return nil
	}
}

// NewClient creates an OCR client. An empty baseURL uses OCR_SERVICE_URL and
// then http://127.0.0.1:18100.
func NewClient(baseURL string, options ...ClientOption) (*Client, error) {
	resolvedURL := strings.TrimSpace(baseURL)
	if resolvedURL == "" {
		resolvedURL = strings.TrimSpace(os.Getenv("OCR_SERVICE_URL"))
	}
	if resolvedURL == "" {
		resolvedURL = defaultBaseURL
	}
	resolvedURL = strings.TrimRight(resolvedURL, "/")
	if err := validateBaseURL(resolvedURL); err != nil {
		return nil, err
	}

	config := clientConfig{timeout: defaultTimeout}
	for _, option := range options {
		if option == nil {
			return nil, errors.New("client option cannot be nil")
		}
		if err := option(&config); err != nil {
			return nil, fmt.Errorf("configure OCR client: %w", err)
		}
	}

	httpClient := config.httpClient
	if httpClient == nil {
		httpClient = &http.Client{Timeout: config.timeout}
	} else if config.timeoutSet {
		clonedClient := *httpClient
		clonedClient.Timeout = config.timeout
		httpClient = &clonedClient
	}

	return &Client{baseURL: resolvedURL, httpClient: httpClient}, nil
}

// Recognize converts an image or PDF to Markdown.
func (client *Client) Recognize(
	ctx context.Context,
	filePath string,
	options *RecognizeOptions,
) (*Result, error) {
	path, pdf, err := inspectFile(filePath)
	if err != nil {
		return nil, err
	}
	engine, pages, err := validateRecognizeOptions(options, pdf)
	if err != nil {
		return nil, err
	}

	versionedPath, legacyPath := "/v1/ocr/image", "/ocr/image"
	contentType := imageContentType(path)
	if pdf {
		versionedPath, legacyPath = "/v1/ocr/pdf", "/ocr/pdf"
		contentType = "application/pdf"
	}
	fields := map[string]string{"engine": string(engine)}
	if pages != "" {
		fields["page_range"] = pages
	}

	response, err := client.uploadWithLegacyFallback(
		ctx,
		path,
		contentType,
		fields,
		versionedPath,
		legacyPath,
	)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode < http.StatusOK || response.StatusCode >= http.StatusMultipleChoices {
		return nil, decodeServiceError(response)
	}

	var result Result
	if err := decodeJSON(response.Body, &result); err != nil {
		return nil, fmt.Errorf("decode OCR response: %w", err)
	}
	if result.Markdown == "" || result.Engine == "" || result.Pages < 1 {
		return nil, errors.New("OCR service returned an invalid success response")
	}
	return &result, nil
}

// Health returns Ollama and configured model availability. A structured HTTP
// 503 response is returned as a degraded HealthResult rather than an error.
func (client *Client) Health(ctx context.Context) (*HealthResult, error) {
	response, err := client.getWithLegacyFallback(ctx, "/v1/ocr/health", "/ocr/health")
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK && response.StatusCode != http.StatusServiceUnavailable {
		return nil, decodeServiceError(response)
	}

	var health HealthResult
	if err := decodeJSON(response.Body, &health); err != nil {
		return nil, fmt.Errorf("decode OCR health response: %w", err)
	}
	if health.Status == "" || health.Models == nil {
		return nil, errors.New("OCR service returned an invalid health response")
	}
	return &health, nil
}

func validateBaseURL(baseURL string) error {
	parsed, err := url.Parse(baseURL)
	if err != nil {
		return fmt.Errorf("invalid OCR base URL: %w", err)
	}
	if (parsed.Scheme != "http" && parsed.Scheme != "https") || parsed.Host == "" {
		return errors.New("OCR base URL must be an absolute http or https URL")
	}
	if parsed.RawQuery != "" || parsed.Fragment != "" {
		return errors.New("OCR base URL cannot contain a query or fragment")
	}
	return nil
}

func inspectFile(filePath string) (string, bool, error) {
	path := strings.TrimSpace(filePath)
	if path == "" {
		return "", false, errors.New("file path cannot be empty")
	}
	info, err := os.Stat(path)
	if err != nil {
		return "", false, fmt.Errorf("inspect OCR file: %w", err)
	}
	if !info.Mode().IsRegular() {
		return "", false, errors.New("OCR file must be a regular file")
	}

	file, err := os.Open(path)
	if err != nil {
		return "", false, fmt.Errorf("open OCR file: %w", err)
	}
	defer file.Close()
	magic := make([]byte, 5)
	read, err := io.ReadFull(file, magic)
	if err != nil && !errors.Is(err, io.EOF) && !errors.Is(err, io.ErrUnexpectedEOF) {
		return "", false, fmt.Errorf("inspect OCR file contents: %w", err)
	}
	isPDF := string(magic[:read]) == "%PDF-" || strings.EqualFold(filepath.Ext(path), ".pdf")
	return path, isPDF, nil
}

func validateRecognizeOptions(options *RecognizeOptions, pdf bool) (Engine, string, error) {
	engine := EngineAuto
	pages := ""
	if options != nil {
		if options.Engine != "" {
			engine = Engine(strings.ToLower(strings.TrimSpace(string(options.Engine))))
		}
		pages = strings.TrimSpace(options.Pages)
	}

	switch engine {
	case EngineAuto, EnginePaddle, EngineGLM, EngineQwen:
	default:
		return "", "", errors.New("engine must be one of: auto, paddle, glm, qwen")
	}
	if pages == "" {
		return engine, "", nil
	}
	if !pdf {
		return "", "", errors.New("pages is only valid for PDF files")
	}
	if !pageRangePattern.MatchString(pages) {
		return "", "", errors.New("pages must be a one-based page or inclusive range such as 2-7")
	}
	if separator := strings.IndexByte(pages, '-'); separator >= 0 {
		start, _ := strconv.Atoi(pages[:separator])
		end, _ := strconv.Atoi(pages[separator+1:])
		if start > end {
			return "", "", errors.New("pages range start must be less than or equal to end")
		}
	}
	return engine, pages, nil
}

func imageContentType(path string) string {
	contentType := mime.TypeByExtension(strings.ToLower(filepath.Ext(path)))
	if strings.HasPrefix(contentType, "image/") {
		return contentType
	}
	return "application/octet-stream"
}

func (client *Client) uploadWithLegacyFallback(
	ctx context.Context,
	path string,
	contentType string,
	fields map[string]string,
	versionedPath string,
	legacyPath string,
) (*http.Response, error) {
	for _, endpoint := range []string{versionedPath, legacyPath} {
		response, err := client.upload(ctx, endpoint, path, contentType, fields)
		if err != nil {
			return nil, fmt.Errorf("upload OCR file: %w", err)
		}
		if response.StatusCode != http.StatusNotFound || endpoint == legacyPath {
			return response, nil
		}
		drainAndClose(response.Body)
	}
	return nil, errors.New("OCR route fallback did not return a response")
}

func (client *Client) upload(
	ctx context.Context,
	endpoint string,
	path string,
	contentType string,
	fields map[string]string,
) (*http.Response, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("open OCR file: %w", err)
	}
	reader, writer := io.Pipe()
	multipartWriter := multipart.NewWriter(writer)
	request, err := http.NewRequestWithContext(
		ctx,
		http.MethodPost,
		client.baseURL+endpoint,
		reader,
	)
	if err != nil {
		file.Close()
		reader.Close()
		writer.Close()
		return nil, fmt.Errorf("build OCR request: %w", err)
	}
	request.Header.Set("Content-Type", multipartWriter.FormDataContentType())
	request.Header.Set("User-Agent", "ocr-service-client-go/0.1.0")

	go writeMultipart(writer, multipartWriter, file, filepath.Base(path), contentType, fields)
	response, err := client.httpClient.Do(request)
	if err != nil {
		_ = reader.CloseWithError(err)
		return nil, err
	}
	return response, nil
}

func writeMultipart(
	pipeWriter *io.PipeWriter,
	multipartWriter *multipart.Writer,
	file *os.File,
	filename string,
	contentType string,
	fields map[string]string,
) {
	var writeErr error
	defer func() {
		if closeErr := multipartWriter.Close(); writeErr == nil {
			writeErr = closeErr
		}
		if closeErr := file.Close(); writeErr == nil {
			writeErr = closeErr
		}
		_ = pipeWriter.CloseWithError(writeErr)
	}()

	for name, value := range fields {
		if writeErr = multipartWriter.WriteField(name, value); writeErr != nil {
			return
		}
	}
	headers := make(textproto.MIMEHeader)
	headers.Set(
		"Content-Disposition",
		mime.FormatMediaType("form-data", map[string]string{"name": "file", "filename": filename}),
	)
	headers.Set("Content-Type", contentType)
	part, err := multipartWriter.CreatePart(headers)
	if err != nil {
		writeErr = err
		return
	}
	if _, err := io.Copy(part, file); err != nil {
		writeErr = err
		return
	}
}

func (client *Client) getWithLegacyFallback(
	ctx context.Context,
	versionedPath string,
	legacyPath string,
) (*http.Response, error) {
	for _, endpoint := range []string{versionedPath, legacyPath} {
		request, err := http.NewRequestWithContext(
			ctx,
			http.MethodGet,
			client.baseURL+endpoint,
			nil,
		)
		if err != nil {
			return nil, fmt.Errorf("build OCR health request: %w", err)
		}
		request.Header.Set("User-Agent", "ocr-service-client-go/0.1.0")
		response, err := client.httpClient.Do(request)
		if err != nil {
			return nil, fmt.Errorf("request OCR health: %w", err)
		}
		if response.StatusCode != http.StatusNotFound || endpoint == legacyPath {
			return response, nil
		}
		drainAndClose(response.Body)
	}
	return nil, errors.New("OCR health route fallback did not return a response")
}

func decodeServiceError(response *http.Response) error {
	var payload struct {
		Error   string `json:"error"`
		Message string `json:"message"`
	}
	_ = decodeJSON(response.Body, &payload)
	message := strings.TrimSpace(payload.Message)
	if message == "" {
		message = strings.TrimSpace(payload.Error)
	}
	if message == "" {
		message = http.StatusText(response.StatusCode)
	}
	return &ServiceError{StatusCode: response.StatusCode, Message: message}
}

func decodeJSON(reader io.Reader, destination any) error {
	decoder := json.NewDecoder(io.LimitReader(reader, maxJSONBytes))
	if err := decoder.Decode(destination); err != nil {
		return err
	}
	return nil
}

func drainAndClose(body io.ReadCloser) {
	_, _ = io.Copy(io.Discard, io.LimitReader(body, 64<<10))
	_ = body.Close()
}
