package ocrclient

import "fmt"

// ServiceError represents a non-success HTTP response from the OCR service.
type ServiceError struct {
	StatusCode int
	Message    string
	// CloudAttempted reports whether a request to Jina may have been made while serving the
	// request. Nil when the response did not say, for example with an older server.
	CloudAttempted *bool
}

// Error implements error.
func (failure *ServiceError) Error() string {
	if failure.StatusCode == 0 {
		return failure.Message
	}
	return fmt.Sprintf("OCR service returned HTTP %d: %s", failure.StatusCode, failure.Message)
}
