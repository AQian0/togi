# togi Agent 能力演进方案

> 目标：在现有单体 CLI Agent 基础上，渐进实现（1）子代理与编排（2）上下文索引 / RAG（3）稳定工具调用体系。
> 原则：每阶段独立可用、独立验收；优先复用现有代码与既有依赖，不为远期需求提前抽象。

---

## 0. 现状盘点（可复用资产）

| 资产 | 位置 | 对本方案的价值 |
|---|---|---|
| 完整 agent 循环(流式、重试、取消、上下文 hook) | `src/agent.rs` `stream_chat` | 子代理直接复用,不重写循环 |
| 多 provider 构建宏 | `src/agent.rs` `providers!` | 子代理可指定不同模型 |
| 工具 pipeline 包装（参数注入、分页） | `src/pipeline/` | 新工具自动获得 cwd 注入与分页 |
| 工具副作用分类注册表 | `src/tools.rs` `ToolRegistry` | 子代理进度展示、权限确认的挂点 |
| 滚动摘要压缩 + checkpoint 持久化 | `src/context.rs` | RAG 之外的上下文基线已具备 |
| turso (libsql) 持久化 | `src/store.rs` | 索引、记忆、黑板通信的存储层（Q6 已验证：原生 FTS 与向量函数可用，语法见 §2.1） |
| rig `embeddings` / `vector_store` 抽象 | rig 0.40 | 向量 RAG 的现成接口 |
| rig `tool_concurrency` 并行工具执行 | rig 0.40 | 并行子代理零编排代码 |

---

## 1. 子代理与编排

**核心设计：子代理就是一个工具。** 实现 rig `Tool` trait 的 `agent` 工具，内部构建受限的 `DynamicAgent`、跑现有 `stream_chat`，把最终文本作为 tool result 返回。编排、通信全部建立在这一原语之上。

### 架构边界（已定论，勿漂移）

- **统一的抽象是运行时 `stream_chat`，不是 `Tool` 接口。** 三种调用面（UI 提交、`agent` 工具、flow 节点）复用同一运行时；主代理不包装成工具（它的调用者是用户，包装后无人调用）。
- **不设独立通信层。** 通信 = 工具参数（父→子）+ 返回值（子→父）+ 黑板（兄弟间）+ `AgentEvent` 通道（→UI），均为具体机制。仅当出现跨进程 / A2A / 第四种通信模式时才引入适配层。
- **禁止子代理点对点直连消息**：通信只沿父子树边或经黑板（session 作用域），保证因果链是树、可重放、可调试。

### 阶段 1.1：基础子代理（MVP）

> **状态：已完成。** 验收通过（live e2e：委派 → 嵌套事件 → 结论回传）。与原文的实现偏差：
> - 事件通道与取消信号经 rig `tool_extensions` 按调用注入（工具经 `call_with_extensions` 取用），未引入共享槽位；inject / paginate 包装器同步补了扩展转发。
> - `AgentEvent` 用 `Child{depth}` 包装变体而非逐变体加 `depth` 字段；UI 只转发子代理的工具活动 / 通知并缩进展示，流式文本不转发（结论作为 `agent` 工具结果完整展示，混入父回答 markdown 流会错乱）。
> - 工具结果的副作用配对由 FIFO 队列改为按 `internal_call_id` 键控：父子事件交错时 FIFO 会把子结果错配给父调用，导致结论被只读折叠。

- 新增 `src/tools/agent.rs`：`AgentTool`，参数 `{ task: string, profile?: string }`。
- 子代理定义采用**文件定义**（Q3 已定）：`.togi/agents/*.md`，frontmatter 声明模型 / 工具白名单 / 描述，正文作为该代理的 preamble；格式后续可换，加载器单独成模块便于替换。
- 内部：`DynamicAgent::build`（可换模型）+ 工具白名单(子代理默认只给 `read`/`shell`,不给 `agent` 自身 → 天然防递归)。
- 独立 history（不与父共享），完成后只把结论返回父代理。
- `AgentEvent` 加 `depth` 字段转发到 UI，子代理过程缩进展示。
- 取消传播：复用现有 `watch::Receiver<bool>`，父取消 → 子取消。
- 约束：深度上限 2、单次子代理 `max_multi_turn` 减半、超时兜底。
- **验收**：主代理能在一次对话中委派子任务（如"让子代理分析这个目录"），UI 可见嵌套过程，结果正确回传。

### 阶段 1.2：并行子代理

- 模型一轮发起多个 `agent` 调用时，rig 的 `tool_concurrency` 已并行执行——本阶段几乎无代码，只需验证 + 补 UI 并行展示。
- **验收**：一次"同时审查这三个文件"产生 3 个并行子代理，结果各自正确配对。

### 阶段 1.3：自定义编排（声明式 DAG）

LLM 自主调用子代理（1.1/1.2）覆盖"复杂编排"的动态部分；用户自定义的固定流程走确定性编排器，不靠 LLM 现场决策：

```toml
# flows/code-review.toml
[[node]]
id = "analyze"
profile = "reader"          # 引用的子代理配置
prompt = "分析 {{target}} 的结构"

[[node]]
id = "review"
profile = "reviewer"
prompt = "基于以下分析审查问题：{{analyze.output}}"
after = ["analyze"]
```

- 新增 `src/flow/`：TOML 解析 → 拓扑排序 → 逐节点跑 `stream_chat`（每节点是独立子代理），`{{node.output}}` 做模板插值。
- CLI：`/flow run code-review <target>`；并行分支（无依赖的节点）用 `tokio::join!`。
- **验收**：内置 2 个示例 flow（如 code-review、refactor-plan）可跑通；节点失败时报告具体节点并可从失败处续跑。

### 阶段 1.4：代理间通信

按需求强度分三层，只做前两层，第三层预留：

1. **父子返回值**（1.1 已覆盖）——90% 场景。
2. **黑板模式**：turso 增加 `artifacts` 表 `(session, name, content, author, ts)`；配套 `artifact_put` / `artifact_get` / `artifact_list` 三个小工具。子代理写命名产物，兄弟/父代理按名读取。解决 flow 节点间大块结构化数据传递（塞 prompt 太长）。
3. **预留**：跨进程 / A2A 协议、订阅式消息推送——等真实需求出现再选型。

- **验收**：flow 中 review 节点通过 artifact 读取 analyze 节点产物，而非全部走 prompt 拼接。

---

## 2. 上下文管理与检索（索引 / RAG）

现有滚动摘要解决"上下文放不下"，本部分解决"找得到、记得住"。

### 阶段 2.1：关键词检索（Turso 原生 FTS，无 embedding 依赖，先做这个）

- turso 建 `chunks` 表 + FTS 索引。注意：Turso 引擎**不支持 SQLite FTS5 虚拟表**，官方兼容清单明确标注 ❌；其 FTS 是 Tantivy 实现的原生索引：
  - 建索引：`CREATE INDEX chunks_fts ON chunks USING fts (content)`
  - 查询：`fts_match(content, ?)` 过滤 + `fts_score` BM25 排序 + `fts_highlight` 高亮
  - 连接时需开 Builder 标志：`experimental_index_method(true)`（turso 0.7 已确认有此 API）
  - 官方有完全对口的指南：docs.turso.tech/guides/code-indexing（代码索引 FTS+向量混合），可直接参考其 schema
- 索引命令 `/index [path]`：遍历 → 哈希去重 → 分块入库；文件变更按 mtime+hash 增量更新。
- 新增 `search` 工具：FTS 查询 + 返回路径/行号/片段。
- 确定性、可单测、零新依赖。
- **验收**：索引本项目（~1.1 万行）秒级完成；`search` 工具查"retry backoff"能命中 `agent.rs` 正确片段。

### 阶段 2.2：向量 RAG（混合检索）

- 复用 rig `embeddings`：`EmbeddingModel`（provider 见待决策 Q1）在入库时对 chunk 生成向量，以 BLOB 存 `chunks` 表（`vector32(...)` 写入，`vector_distance_cos` 查询——已确认 turso 原生支持）。
- 检索 = FTS（fts_score）+ 向量余弦距离，两路结果加权融合（先固定权重，不做 RRF 之类的花活）。
- `search` 工具自动降级：无 embedding 配置时退化为纯 FTS。
- 预留 rerank：rig 有 `rerank` 模块，效果不达标再接。
- **验收**：语义查询（如“处理限流的地方”，代码里写的是 rate limit / 429）向量路能命中而纯 FTS 不能。

### 阶段 2.3：长期记忆

- turso `memories` 表：从会话中沉淀的事实 / 用户偏好 / 项目约定（由模型通过 `memory_save` 工具显式写入，不后台偷跑）。
- 注入策略：启动时不全量塞 preamble；模型用 `memory_search` 按需检索（FTS 复用 2.1 基建）。
- **验收**：新会话中模型能通过检索回忆起上一会话保存的项目约定。

### 与现有压缩机制的关系

滚动摘要（已有）管"主对话窗口"，RAG 管"外部知识"，记忆管"跨会话"。三者不合并、不互相替代。

---

## 3. 工具调用体系

### 回答「内置程序 vs 命令调用设备程序」：三层分工

| 层 | 适用 | 例子 | 稳定性来源 |
|---|---|---|---|
| **内置 Rust 工具** | 结果需结构化解析、高频、错误处理精细 | read / modify / search / agent | 类型化参数 + 规范错误码（已有 `TogiError::code`） |
| **shell 调用设备程序** | 长尾能力、一次写死反而脆 | git / rg / cargo / ffmpeg | 程序自身的成熟稳定；零开发成本 |
| **MCP 外部工具服务**（阶段 3.3） | 生态工具、第三方能力 | 数据库、浏览器、Figma | 标准协议 + schema 声明 |

原则：**能用设备上成熟程序稳定完成的，不内置重造**（如 grep 语义直接 shell 走 `rg`）；但**结果要被模型稳定解析、或调用频率高的，内置为类型化工具**。shell 是兜底而非主力解析对象。

### 阶段 3.1：内置工具补全

- `search`（2.1 的检索工具，内置）。
- 不为 git、curl 等做内置封装——shell 已够用（YAGNI，有具体痛点再加）。
- **验收**：见 2.1。

### 阶段 3.2：变更类工具的用户确认

> **状态：已完成。** `confirm` 工具包装器在执行前阻塞 `Mutating` 调用，经 `AgentEvent` 请求 UI 决策；确认缺失或通道关闭时 fail closed。确认卡片直接复用模型回答的块样式，支持允许一次、拒绝、会话内始终允许；`[approval]` 的 `always_allow` 提供配置白名单。子代理共享同一策略，变更调用逐项确认。

- `ToolRegistry` 已有 `ToolEffect::Mutating` 分类 → 在 `transform` / UI 层挂确认：mutating 调用执行前展示确认（可配置白名单 / `always allow`）。
- 这是子代理安全性的前提：子代理的 mutating 工具默认继承确认策略，深度执行时可配置自动批准只读。
- **验收**：`modify` / 写类 shell 命令触发确认；`read` 不触发。

### 阶段 3.3：MCP 接入（待决策 Q2）

- 用 `rmcp` crate 作为 MCP client；配置文件中声明 server（command + args）；启动时连接、列出工具、适配为 rig `ToolDyn` 注册进 registry。
- 工具名加 server 前缀防冲突（如 `github.create_issue`）。
- **验收**：接一个官方 reference server（如 filesystem），其工具出现在工具列表并可被模型调用。

---

## 4. 待你决策的技术选型

| # | 问题 | 选项 | 影响阶段 |
|---|---|---|---|
| Q1 | Embedding 方案 | A. API（OpenAI text-embedding-3-small 等，rig 现成）<br>B. 本地（fastembed-rs，无网络依赖，加依赖） | 2.2 |
| Q2 | MCP 是否进入本期范围 | 进 → 阶段 3.3；不进 → 搁置，shell + 内置工具先顶着 | 3.3 |
| Q3 | 子代理定义的载体 | **已定：文件定义**（`.togi/agents/*.md`，frontmatter 声明工具/模型；格式后续可换） | 1.1 |
| Q4 | flow DSL 是否需要条件分支 / 循环 | 暂只做 DAG 拓扑（推荐，YAGNI）；需要则升级为小型表达式 | 1.3 |
| Q5 | RAG 索引范围 | A. 仅当前项目目录（推荐先做这个）<br>B. 多工作区注册表 | 2.1 |
| Q6 | turso 引擎能力验证 | **已定：文档验证通过**——原生 FTS（Tantivy，`fts_match`/`fts_score`）与向量函数（`vector32`/`vector_distance_cos`）可用，需 Builder 开 `experimental_index_method(true)`；语法非 SQLite FTS5，见 §2.1 | 2.1 |

---

## 5. 里程碑总览

| 里程碑 | 内容 | 依赖 |
|---|---|---|
| **M1** | 1.1 基础子代理 ✅ + 3.2 工具确认 ✅ | 无 |
| **M2** | 1.2 并行子代理 + 2.1 FTS 索引/检索 | M1 |
| **M3** | 1.3 编排 DAG + 1.4 黑板通信 | M2、Q4 |
| **M4** | 2.2 向量 RAG + 2.3 长期记忆（+ 3.3 MCP） | Q1、Q2 |

工作量粗估：M1 最小（核心是一个新工具 + 事件转发），M3 的编排器与 M4 的向量检索是两个最大块。每里程碑结束都是可发布状态。
