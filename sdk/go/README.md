# OCR Service Go Client

Dependency-free Go adapter for the local OCR service. It supports images and PDFs through one
`Recognize` method and streams multipart uploads instead of loading the entire file into memory.

## Installation

Install a tagged release directly from GitHub:

```bash
go get github.com/andrewrong/ocr-service/sdk/go@v0.1.0
```

For local SDK development, temporarily point the module at a checkout:

```bash
go mod edit -replace=github.com/andrewrong/ocr-service/sdk/go=/path/to/ocr-service/sdk/go
```

## Usage

```go
package main

import (
	"context"
	"fmt"
	"log"

	ocrclient "github.com/andrewrong/ocr-service/sdk/go"
)

func main() {
	client, err := ocrclient.NewClient("http://host.docker.internal:18100")
	if err != nil {
		log.Fatal(err)
	}

	result, err := client.Recognize(
		context.Background(),
		"/data/document.pdf",
		&ocrclient.RecognizeOptions{
			Engine: ocrclient.EngineAuto,
			Pages:  "1-10",
		},
	)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(result.Markdown)
}
```

Pass an empty base URL to use `OCR_SERVICE_URL`, falling back to
`http://127.0.0.1:18100`. The default request timeout is 15 minutes; customize it with
`WithTimeout`. Use `WithHTTPClient` to provide a shared client, proxy, transport, or test adapter.

The client validates engine and page options locally, detects PDFs by header or extension, uses
the versioned `/v1/ocr/*` routes, and retries a matching legacy `/ocr/*` route only after HTTP 404.
HTTP failures are returned as `*ocrclient.ServiceError`.

## Verification

```bash
go test ./...
go vet ./...
OCR_LIVE_TEST_URL=http://127.0.0.1:18100 go test -run TestLiveHealth -v
```
