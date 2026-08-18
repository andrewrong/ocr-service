# OCR Service Benchmark Notes

- Date: 2026-08-18
- Machine: Mac Mini M4 Pro, 64 GB
- Build: debug (`cargo run`)
Fixture: 1400×700 PNG with English and Simplified Chinese; two-page mixed PDF rendered at 180 DPI

These are smoke-test measurements, not statistically rigorous benchmarks. Model timings include
HTTP and serialization overhead. Paddle was warm; GLM and Qwen were cold-loaded during the
three-engine comparison.

| Test | Result | API duration |
|---|---|---:|
| Paddle image | Exact text; normalized full-width punctuation spacing | 2.820 s |
| GLM image | Exact English and Chinese text | 11.502 s |
| Qwen image | Exact English and Chinese text | 14.232 s |
| Paddle mixed PDF, 2 pages | Correct page order and Markdown separators | 6.275 s total (3.138 s/page) |
| Paddle text PDF, 101 pages | HTTP 200; 101/101 page markers, no missing or reordered pages | 215.436 s total (2.133 s/page) |

Memory observations:

- HTTP service maximum RSS after startup and health check: 10,600,448 bytes (10.1 MiB).
- Ollama-reported loaded model size: Paddle 2.6 GB; Qwen 14 GB, both on GPU.
- Model residency is managed by Ollama and is not included in the service process RSS.
