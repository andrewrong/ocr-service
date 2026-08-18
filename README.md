# OCR Service

Local HTTP OCR service backed by Ollama. Docker contains the Rust API and Poppler; Ollama and its models stay on the macOS host for Metal acceleration.

## Deploy

Prerequisites: OrbStack, Ollama, PaddleOCR-VL 1.6, GLM OCR, and Qwen3-VL 8B.

Allow the container to reach Ollama, then restart the Ollama application:

```bash
launchctl setenv OLLAMA_HOST 0.0.0.0:11434
```

Binding Ollama to all interfaces can expose it to the local network. Keep the macOS firewall enabled or restrict access with local firewall rules.

Start and verify the service:

```bash
cp .env.example .env
docker compose up -d --build
docker compose ps
curl -fsS http://127.0.0.1:18100/ocr/health | jq
```

Stop it with `docker compose down`. The API port binds to `127.0.0.1` by default.

## Agent Skill

The project Skill lives at `.agents/skills/ocr-local-service`. It uploads host files over HTTP, so the service container needs no filesystem mounts.

```bash
bash .agents/skills/ocr-local-service/scripts/ocr.sh image /path/to/image.png
bash .agents/skills/ocr-local-service/scripts/ocr.sh pdf /path/to/file.pdf --pages 2-7
```

To install it personally, copy the entire directory:

```bash
cp -R .agents/skills/ocr-local-service ~/.codex/skills/ocr-local-service
```

Set `OCR_SERVICE_URL` when the service is not at `http://127.0.0.1:18100`.
