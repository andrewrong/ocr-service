# OCR Service

本地 OCR 基础设施服务，运行在 Mac Mini M4 Pro (64GB) 上。

## 项目概述

Rust 编写的 OCR 服务，通过 Ollama 调用本地视觉模型，支持 PDF 和图片的文字识别，输出 Markdown 格式。服务以 Docker Compose 常驻 HTTP API 运行，Agent 通过项目 Skill 调用。

## 架构

- **HTTP 服务层**: axum，提供 REST API
- **Agent 集成层**: `.agents/skills/ocr-local-service`，通过 HTTP 上传本地文件
- **OCR 引擎层**: 统一封装 Ollama API，支持三个模型切换
- **PDF 处理层**: poppler pdftoppm 拆页，逐页 OCR 后合并

## 支持的模型

| 模型 | Ollama 名称 | 用途 |
|---|---|---|
| PaddleOCR-VL-1.6 | `hf.co/PaddlePaddle/PaddleOCR-VL-1.6-GGUF` | 默认引擎，SOTA 精度 (96.33) |
| glm-ocr | `glm-ocr` | 备选引擎 (94.62) |
| Qwen3-VL-8B | `qwen3-vl:8b` | 文档理解/问答 |

## 运行方式

- `docker compose up -d --build` — 构建并启动 HTTP 服务
- `docker compose logs -f ocr-service` — 查看服务日志
- `docker compose down` — 停止服务
- `ocr-service serve --port 8100` — 不使用容器时直接运行

## API

- `POST /ocr/pdf` — PDF OCR，参数: file, engine(auto|paddle|glm|qwen), page_range
- `POST /ocr/image` — 图片 OCR，参数: file, engine
- `GET /ocr/health` — 健康检查

## 外部依赖

- Ollama 运行在宿主机 11434；容器通过 `host.docker.internal` 连接
- poppler (`pdftoppm`) 已包含在运行时镜像
- Skill 客户端依赖宿主机的 `curl` 和 `jq`

## Skill

- 项目位置: `.agents/skills/ocr-local-service`
- 默认 Compose/Skill 服务地址: `http://127.0.0.1:18100`
- 自定义地址: `OCR_SERVICE_URL=http://host:port`
- 迁移到个人目录时，完整复制 Skill 目录到 `~/.codex/skills/ocr-local-service`

## 开发规范

- 使用 `cargo fmt` 格式化代码
- 使用 `cargo clippy` 检查
- 错误处理统一用 `anyhow::Result`
- 日志用 `tracing`
- 异步运行时用 `tokio`
