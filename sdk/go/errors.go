package ocrclient

import "fmt"

// ServiceError represents a non-success HTTP response from the OCR service.
type ServiceError struct {
	StatusCode int
	Message    string
}

// Error implements error.
func (failure *ServiceError) Error() string {
	if failure.StatusCode == 0 {
		return failure.Message
	}
	return fmt.Sprintf("OCR service returned HTTP %d: %s", failure.StatusCode, failure.Message)
}
