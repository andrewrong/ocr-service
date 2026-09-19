use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Paddle,
    Glm,
    Qwen,
    Jina,
    #[default]
    Auto,
}

impl fmt::Display for Engine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Paddle => "paddle",
            Self::Glm => "glm",
            Self::Qwen => "qwen",
            Self::Jina => "jina",
            Self::Auto => "auto",
        })
    }
}

impl FromStr for Engine {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "paddle" => Ok(Self::Paddle),
            "glm" => Ok(Self::Glm),
            "qwen" => Ok(Self::Qwen),
            "jina" => Ok(Self::Jina),
            "auto" | "" => Ok(Self::Auto),
            other => {
                anyhow::bail!(
                    "unsupported engine {other:?}; expected auto, paddle, glm, qwen, or jina"
                )
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct OcrResponse {
    pub markdown: String,
    pub engine: String,
    pub pages: usize,
    pub duration_ms: u128,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub backend: String,
    pub backend_ready: bool,
    /// Deprecated readiness alias retained for SDKs released before backend selection existed.
    pub ollama: bool,
    pub models: Vec<ModelStatus>,
    pub jina: JinaHealth,
}

#[derive(Debug, Serialize)]
pub struct JinaHealth {
    pub configured: bool,
    pub reachable: bool,
}

#[derive(Debug, Serialize)]
pub struct ModelStatus {
    pub engine: Engine,
    pub name: String,
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}
