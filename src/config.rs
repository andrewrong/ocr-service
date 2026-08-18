use std::{env, time::Duration};

use crate::models::Engine;

#[derive(Clone, Debug)]
pub struct Config {
    pub ollama_url: String,
    pub port: u16,
    pub paddle_model: String,
    pub glm_model: String,
    pub qwen_model: String,
    pub request_timeout: Duration,
    pub paddle_timeout: Duration,
    pub glm_timeout: Duration,
    pub qwen_timeout: Duration,
    pub max_concurrent_model_requests: usize,
    pub max_concurrent_pages: usize,
    pub max_upload_bytes: usize,
    pub pdf_dpi: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama_url: "http://127.0.0.1:11434".to_owned(),
            port: 8100,
            paddle_model: "hf.co/PaddlePaddle/PaddleOCR-VL-1.6-GGUF".to_owned(),
            glm_model: "glm-ocr".to_owned(),
            qwen_model: "qwen3-vl:8b".to_owned(),
            request_timeout: Duration::from_secs(300),
            paddle_timeout: Duration::from_secs(30),
            glm_timeout: Duration::from_secs(60),
            qwen_timeout: Duration::from_secs(120),
            max_concurrent_model_requests: 1,
            max_concurrent_pages: 4,
            max_upload_bytes: 100 * 1024 * 1024,
            pdf_dpi: 180,
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let defaults = Self::default();

        let config = Self {
            ollama_url: env_string("OCR_OLLAMA_URL", defaults.ollama_url),
            port: env_parse("OCR_PORT", defaults.port)?,
            paddle_model: env_string("OCR_PADDLE_MODEL", defaults.paddle_model),
            glm_model: env_string("OCR_GLM_MODEL", defaults.glm_model),
            qwen_model: env_string("OCR_QWEN_MODEL", defaults.qwen_model),
            request_timeout: Duration::from_secs(env_parse(
                "OCR_REQUEST_TIMEOUT_SECS",
                defaults.request_timeout.as_secs(),
            )?),
            paddle_timeout: Duration::from_secs(env_parse(
                "OCR_PADDLE_TIMEOUT_SECS",
                defaults.paddle_timeout.as_secs(),
            )?),
            glm_timeout: Duration::from_secs(env_parse(
                "OCR_GLM_TIMEOUT_SECS",
                defaults.glm_timeout.as_secs(),
            )?),
            qwen_timeout: Duration::from_secs(env_parse(
                "OCR_QWEN_TIMEOUT_SECS",
                defaults.qwen_timeout.as_secs(),
            )?),
            max_concurrent_model_requests: env_parse(
                "OCR_MAX_CONCURRENT_MODEL_REQUESTS",
                defaults.max_concurrent_model_requests,
            )?,
            max_concurrent_pages: env_parse(
                "OCR_MAX_CONCURRENT_PAGES",
                defaults.max_concurrent_pages,
            )?,
            max_upload_bytes: env_parse("OCR_MAX_UPLOAD_BYTES", defaults.max_upload_bytes)?,
            pdf_dpi: env_parse("OCR_PDF_DPI", defaults.pdf_dpi)?,
        };
        anyhow::ensure!(
            config.max_concurrent_pages > 0,
            "OCR_MAX_CONCURRENT_PAGES must be greater than zero"
        );
        anyhow::ensure!(
            config.max_upload_bytes > 0,
            "OCR_MAX_UPLOAD_BYTES must be greater than zero"
        );
        anyhow::ensure!(config.pdf_dpi > 0, "OCR_PDF_DPI must be greater than zero");
        anyhow::ensure!(
            !config.paddle_timeout.is_zero(),
            "OCR_PADDLE_TIMEOUT_SECS must be greater than zero"
        );
        anyhow::ensure!(
            !config.glm_timeout.is_zero(),
            "OCR_GLM_TIMEOUT_SECS must be greater than zero"
        );
        anyhow::ensure!(
            !config.qwen_timeout.is_zero(),
            "OCR_QWEN_TIMEOUT_SECS must be greater than zero"
        );
        anyhow::ensure!(
            config.max_concurrent_model_requests > 0,
            "OCR_MAX_CONCURRENT_MODEL_REQUESTS must be greater than zero"
        );
        anyhow::ensure!(
            config.request_timeout > config.paddle_timeout
                && config.request_timeout > config.glm_timeout
                && config.request_timeout > config.qwen_timeout,
            "OCR_REQUEST_TIMEOUT_SECS must be greater than every model timeout"
        );
        Ok(config)
    }

    pub fn model_names(&self) -> [&str; 3] {
        [&self.paddle_model, &self.glm_model, &self.qwen_model]
    }

    pub fn timeout_for(&self, engine: Engine) -> Duration {
        match engine {
            Engine::Paddle | Engine::Auto => self.paddle_timeout,
            Engine::Glm => self.glm_timeout,
            Engine::Qwen => self.qwen_timeout,
        }
    }
}

fn env_string(name: &str, default: String) -> String {
    env::var(name).unwrap_or(default)
}

fn env_parse<T>(name: &str, default: T) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid {name}={value:?}: {error}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Config;
    use crate::models::Engine;

    #[test]
    fn default_timeouts_are_tuned_per_model() {
        let config = Config::default();

        assert_eq!(config.timeout_for(Engine::Auto), Duration::from_secs(30));
        assert_eq!(config.timeout_for(Engine::Paddle), Duration::from_secs(30));
        assert_eq!(config.timeout_for(Engine::Glm), Duration::from_secs(60));
        assert_eq!(config.timeout_for(Engine::Qwen), Duration::from_secs(120));
        assert_eq!(config.max_concurrent_model_requests, 1);
    }
}
