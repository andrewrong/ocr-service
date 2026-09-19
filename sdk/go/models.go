package ocrclient

// Engine selects the preferred OCR engine.
type Engine string

const (
	// EngineAuto lets the service select an engine and fall back on timeout.
	EngineAuto Engine = "auto"
	// EnginePaddle starts with PaddleOCR-VL.
	EnginePaddle Engine = "paddle"
	// EngineGLM starts with GLM OCR.
	EngineGLM Engine = "glm"
	// EngineQwen starts with Qwen3-VL.
	EngineQwen Engine = "qwen"
	// EngineJina explicitly uploads document pages to Jina's hosted OCR API.
	EngineJina Engine = "jina"
)

// RecognizeOptions controls one recognition request.
type RecognizeOptions struct {
	// Engine defaults to EngineAuto.
	Engine Engine
	// Pages is an optional inclusive, one-based PDF page or range such as "2-7".
	Pages string
}

// Result contains recognized Markdown and execution metadata.
type Result struct {
	Markdown   string `json:"markdown"`
	Engine     string `json:"engine"`
	Pages      int    `json:"pages"`
	DurationMS uint64 `json:"duration_ms"`
}

// ModelStatus describes one configured OCR engine.
type ModelStatus struct {
	Engine    string `json:"engine"`
	Name      string `json:"name"`
	Available bool   `json:"available"`
}

// JinaHealth describes configuration and metadata reachability, not key validation.
type JinaHealth struct {
	Configured bool `json:"configured"`
	Reachable  bool `json:"reachable"`
}

// HealthResult contains local and optional cloud model availability.
type HealthResult struct {
	Status       string `json:"status"`
	Backend      string `json:"backend"`
	BackendReady bool   `json:"backend_ready"`
	// Ollama is a deprecated readiness alias retained for older service responses.
	Ollama bool          `json:"ollama"`
	Models []ModelStatus `json:"models"`
	Jina   *JinaHealth   `json:"jina,omitempty"`
}

// Ready reports whether any local or cloud OCR model is available.
func (health HealthResult) Ready() bool {
	if health.Status != "ok" {
		return false
	}
	for _, model := range health.Models {
		if model.Available {
			return true
		}
	}
	return false
}
