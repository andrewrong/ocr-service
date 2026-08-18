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
