# OCR Service - 任务清单

## Phase 1: 基础搭建

- [x] 初始化 Rust 项目 (cargo init)
- [x] 配置 Cargo.toml 依赖
- [x] 安装 Ollama 模型 (PaddleOCR-VL-1.6, qwen3-vl:8b)
- [x] 实现 config.rs (模型名称、Ollama 地址、端口)

## Phase 2: OCR 核心引擎

- [x] 实现 ocr/engine.rs — Ollama API 统一调用
  - [x] Engine 枚举 (Paddle/Glm/Qwen/Auto)
  - [x] ocr_image() 方法: 图片 -> base64 -> Ollama chat -> Markdown
  - [x] 模型名称映射
  - [x] 超时和错误处理
- [x] 实现 ocr/pdf.rs — PDF 拆页
  - [x] 调用 pdftoppm 将 PDF 转为 PNG
  - [x] 支持 page_range 参数
  - [x] 临时文件管理 (tempfile)
- [x] 实现 ocr/merger.rs — 多页合并
  - [x] 逐页 Markdown 合并，页码分隔符
- [x] 实现 models.rs — 请求/响应类型

## Phase 3: HTTP API

- [x] 实现 http/routes.rs
  - [x] POST /ocr/pdf — 文件上传 -> 拆页 -> 并发OCR -> 合并 -> 返回
  - [x] POST /ocr/image — 文件上传 -> OCR -> 返回
  - [x] GET /ocr/health — Ollama 连接检查 + 模型状态
- [x] 实现 main.rs — clap `serve` 子命令
- [x] 集成测试: curl 调用验证

## Phase 4: Agent Skill

- [x] 创建项目级 `ocr-local-service` Skill
- [x] 实现 HTTP 调用脚本（health/image/pdf）
- [x] 验证 Skill 元数据、错误处理和真实 OCR 调用

## Phase 5: 测试与优化

- [x] 单图片 OCR 测试 (中文/英文)
- [x] 多页 PDF 测试 (扫描版/文本版)
- [x] 三引擎对比测试
- [x] 100+ 页 PDF 稳定性测试
- [x] 性能基准记录 (每页耗时、内存占用)

## Phase 6: Docker Compose 部署

- [x] 创建多阶段 Dockerfile（Rust 构建 + poppler 运行时）
- [x] 创建 Compose 服务、健康检查和安全约束
- [x] 静态验证 Compose、Dockerfile 和官方 ARM64 镜像标签
- [x] 启动 OrbStack 后验证容器构建和 Skill 到容器的端到端调用

## Phase 7: PDF 单页超时与模型回退

- [x] 为 Paddle、GLM、Qwen 配置独立的单页超时
- [x] 单页模型超时后按模型优先级自动回退
- [x] 多页混合使用模型时返回 `mixed` 标记
- [x] 增加真实 HTTP 边界的超时回退回归测试
- [x] 限制 Ollama 模型调用并发，避免排队时间被误判为模型超时
- [x] 用《权力的剧场》前 10 页验证稀疏页面不再拖死整份 PDF

### Review

第 5 页使用 Paddle 超过 30 秒后自动回退 GLM，其余页面继续使用 Paddle。前 10 页真实
回归在 93 秒内返回 HTTP 200，响应包含 10 页 Markdown，并以 `engine: "mixed"` 标记混合模型。
单元测试、Rust fmt 和 clippy 均通过。

## Phase 8: 标准化外部接入

- [x] 保留现有 `/ocr/*` 路由并增加版本化 `/v1/ocr/*` 路由
- [x] 提供可机器读取的 OpenAPI 3.1 契约
- [x] 编写面向宿主机、Docker 和远程调用方的接入文档
- [x] 提供 Python SDK，以统一 `recognize()` 接口隐藏图片/PDF路由和 multipart 上传
- [x] 增加 HTTP 路由兼容性测试和 Python SDK 单元测试
- [x] 运行 Rust/Python 测试、fmt、clippy 和敏感信息扫描

### Review

新增 `/v1/ocr/health`、`/v1/ocr/image`、`/v1/ocr/pdf` 和 `/openapi.yaml`，原有
`/ocr/*` 路由继续工作。Python SDK 使用单一异步 `recognize()` 接口自动识别 PDF、选择
路由、构造 multipart、校验页码并统一错误；版本化路由返回 404 时仅回退到对应旧路由，
已用仍未重建的 18100 部署验证兼容性。

OpenAPI 3.1 校验、Skill 校验、Rust fmt、11 个 Rust 测试、Clippy、Ruff、6 个 Python 测试
和候选文件敏感信息扫描均通过。临时源码实例验证了 OpenAPI、旧/新路由行为、Skill 健康
检查和 SDK 健康检查。

## Phase 9: Go SDK

- [x] 使用 Go 标准库实现 OCR 客户端，不增加运行时依赖
- [x] 提供统一 `Recognize`、`Health`、类型化结果和错误
- [x] 自动识别图片/PDF，校验引擎和 PDF 页码范围
- [x] 支持流式 multipart 上传和 `/v1` 到旧路由的滚动部署回退
- [x] 编写 SDK 文档、示例和单元测试
- [x] 运行 gofmt、go test、go vet、真实健康检查和敏感信息扫描

### Review

新增无第三方运行时依赖的 Go SDK。`Recognize` 隐藏图片/PDF判断、选项校验、流式
multipart 上传、响应解析和旧路由回退，`Health` 在 HTTP 503 时仍返回可检查的降级状态；
调用方可注入共享 `http.Client` 或配置整体超时。

6 个隔离单元测试、gofmt、go test、go vet 和 race detector 均通过。另通过
`OCR_LIVE_TEST_URL=http://127.0.0.1:18100` 对当前旧部署完成真实健康检查，确认 `/v1`
返回 404 时可以回退到旧健康路由。

## Phase 10: 多本地推理后端

- [x] 调研 Ollama、LM Studio、llama.cpp 的共同视觉推理协议
- [x] 评估 async-openai、rust-genai、rig 与直接 HTTP adapter
- [x] 使用统一 OpenAI-compatible Chat Completions 和模型列表接口
- [x] 增加 `ollama`、`lmstudio`、`llamacpp` 后端配置和可选 Bearer token
- [x] 保持 OCR HTTP 路由兼容，并扩展后端中立健康字段
- [x] 让超时和上游模型错误都进入下一模型槽
- [x] 使用隔离的 LM Studio-compatible 服务验证图片、模型列表、鉴权和回退协议
- [x] 验证本机 LM Studio 的真实模型列表和视觉能力元数据
- [ ] 部署前使用选定的轻量视觉模型验收图片和 10 页 PDF

### Review

实现采用现有 reqwest/serde，而不是引入覆盖 Agent、工具和多云供应商的大型依赖。内部
adapter 只暴露转录和模型列表能力，外部 OCR 路由、PDF 管线和 SDK 识别接口保持不变。
旧 `OCR_OLLAMA_URL` 和健康响应中的 `ollama` 字段作为兼容别名保留。

本机 LM Studio API 可连接并能返回标准模型列表；当前安装的视觉模型体积过大，不将其作为
本功能的性能或部署验收模型。代码验收使用隔离的 OpenAI-compatible 测试服务，不加载或
下载真实模型。部署验收应显式选择适合机器资源的视觉模型，并将它的精确 model ID 映射到
至少一个逻辑 OCR 模型槽。
