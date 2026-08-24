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

// HealthResult contains service and model availability.
type HealthResult struct {
	Status string        `json:"status"`
	Ollama bool          `json:"ollama"`
	Models []ModelStatus `json:"models"`
}

// Ready reports whether Ollama and at least one OCR model are available.
func (health HealthResult) Ready() bool {
	if !health.Ollama {
		return false
	}
	for _, model := range health.Models {
		if model.Available {
			return true
		}
	}
	return false
}
