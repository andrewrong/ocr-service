// Package ocrclient recognizes local images and PDFs through the OCR service.
//
// The package hides file-type detection, route selection, streaming multipart
// uploads, rolling-deployment compatibility, and response parsing behind the
// Client interface.
package ocrclient
