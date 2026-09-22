# 借鉴 open-code-review 的确定性评审流水线设计

> 状态: 草案
> 参考项目: [alibaba/open-code-review](https://github.com/alibaba/open-code-review) (ocr)
> 关联文档: `docs/subagent-design.md`, `docs/hook-design.md`, `.kb/decisions/ADR-005-sync-tool-cache.md`

## 1. 背景与动机

dev-assistant 当前的代码评审能力由 `skills/code-review/SKILL.md` 承载，是纯 prompt 驱动：
读什么文件、读多少、何时停止，全部依赖 LLM 自觉。这在中小改动下可用，但在真实场景
中暴露出与 ocr README 所批评的通用 agent 相同的三个痛点：

1. **覆盖率不完整** — 大 changeset 下 LLM "抄近路"，选择性跳过文件；
2. **位置漂移** — 报告的 file:line 与实际代码位置不符；
3. **质量不稳定** — prompt 细微变化导致评审质量波动，且难以调试。

ocr 的解法是 **"确定性工程 × Agent" 混合架构**：能用代码保证正确性的步骤不交给
LLM，只把需要判断力的部分留给模型。其 benchmark（AACR-Bench）显示同等模型下
Precision/F1 显著更高、token 消耗约 1/9。

本设计将 ocr 的核心机制移植到 dev-assistant 的 Rust 架构中，形成一条
**原生确定性评审流水线**，替代现有的 prompt 驱动 skill。

## 2. ocr 工作逻辑解析（源码阅读结论）

以下结论来自对 ocr 仓库源码的直接阅读，作为本设计的依据。

### 2.1 review 主流程

```
executeReviewContext (cmd/opencodereview/review_cmd.go)
  ├─ resolveRepoDir / requireGitRepo        # 仓库解析，硬性前置校验
  ├─ diff.Provider                          # internal/diff: 解析 git diff
  │    ├─ parser.go / hunk.go               #   结构化 Diff + hunk
  │    ├─ gitignore.go                      #   gitignore 感知过滤
  │    └─ workspace_file.go                 #   未跟踪文件纳入
  ├─ agent.selectFiles (internal/agent/selection.go)
  │    # 纯确定性决策: 每个 diff 一个 fileDecision(selected/excluded+Reason)
  │    # 排除原因显式枚举(ExcludeReason)，可解释、可上报
  ├─ groupDiffs (internal/agent/grouping.go)
  │    # 分两路:
  │    ├─ groupWithoutLLM: 策略分组(默认)，纯代码
  │    ├─ callGroupingLLM: LLM 辅助分组(可选)，失败回退 groupWithoutLLM
  │    # 分组后硬约束: enforceMaxFilesPerGroup + enforceGroupTokenBudget
  ├─ dispatchSubtasks (internal/agent/agent.go)
  │    # 每个 FileGroup 一个子评审单元，隔离上下文，并发派发
  └─ CommentCollector 汇总 → resolver 定位校验 → 输出
```

关键点：**文件选择、分组、token 预算三个步骤全部是确定性代码**，LLM 只在
"可选的智能分组"和"每个组的评审判断"中出现，且分组 LLM 失败可回退。

### 2.2 agent 循环与工具集

- 运行时 `internal/llmloop`：通用 tool-use 循环，含**异步上下文压缩**
  （`compression.go`：按轮次分区、active zone、后台触发压缩）。
- 工具集**刻意极简**（`internal/tool/definitions.go`，共 6 个）：

  | 工具 | 职责 |
  |------|------|
  | `task_done` | 结束 |
  | `code_comment` | 提交一条评审意见（结构化，含 file/line/severity/body） |
  | `file_read` | 读完整文件 |
  | `file_read_diff` | 读 diff 上下文 |
  | `file_find` | 找文件 |
  | `code_search` | 代码搜索 |

  工具集从生产 tool-call trace 提炼而来。对比：dev-assistant Reviewer 身份
  有 13 个工具，其中 `kb_store`/`save_summary`/`compress_context` 等对评审
  子任务是噪音。

- 评审产物是结构化的 `model.LlmComment`，不是自由文本报告。

### 2.3 位置定位与反思（三重保障）

1. **确定性解析** (`internal/diff/resolver.go`)：`ResolveLineNumbers` /
   `ResolveComment` 把 LLM 报的行号对齐到实际 diff hunk；
2. **跨文件重定位** (`relocation.go`)：位置无效时按代码内容在其它变更文件中
   搜索重定位；LLM 辅助的 `ReLocateComment` 是最后兜底；
3. **tracking / re-tracking / reflection 任务**（agent.go 注释提到的后处理
   任务类型）：对已产出的 comment 追踪与反思，剔除漂移和误报。

顺序体现了一个原则：**先便宜后昂贵** —— 纯代码校验 → 内容匹配 → LLM 兜底。

### 2.4 scan 流程与规则匹配

- `internal/scan/batch.go`：全文件扫描按 `languageKey` / `firstLevelDirKey`
  分 batch，每批独立评审（同 bundling 思想，粒度按语言/目录）。
- `internal/config/rules/system_rules.go`：**规则 = glob 模式 → 规则文档映射**，
  加 `FileFilter`（用户 include/exclude 优先级明确）。
- `internal/config/rules/sniffer.go`：文件嗅探（如 .h 首行探测是否 ObjC），
  决定应用哪套规则——按文件特征触发，避免规则噪音。

## 3. 现状差距分析

| ocr 机制 | dev-assistant 现状 | 差距 |
|----------|-------------------|------|
| 确定性文件选择 (selection.go) | 无，靠 LLM 用 glob/list_directory 自选 | 缺失 |
| 策略分组 + token 预算硬约束 (grouping.go) | 无，batch_read_files 一次全读 | 缺失 |
| 极简专用工具集 (6 个) | Reviewer 有 13 个通用工具 | 待瘦身 |
| 结构化评审产物 (LlmComment) | 自由文本 markdown 报告 | 缺失 |
| 行号解析/重定位 (resolver/relocation) | 无 | 缺失 |
| diff 感知 | 无 git diff 工具，面向全库扫描 | 缺失 |
| 子 agent 并发 + 隔离上下文 | 已有 (spawn_subagent, 后台 ≤4) | **已具备** |
| 上下文压缩 | 已有 (agent/compressor.rs + context_budget) | **已具备** |
| 危险操作审批 | 已有 (security/approval.rs) | **已具备** |
| 异步 IO 工具框架 | 已有 (tools/async_tool.rs) | **已具备** |

结论：dev-assistant 的底座（并发、隔离、审批、压缩）已就绪，缺的是
**流水线层** —— 即 ocr 把"不能出错的事"用代码管起来的那部分。

## 4. 目标架构

```
                       ┌─────────────────────────────────────┐
                       │  review_pipeline (新增, 纯 Rust)     │
 用户: 评审本次改动     │                                     │
 ────────────────────► │  ① DiffProvider: git diff 结构化     │
                       │  ② FileSelector: 选择/排除(可解释)   │
                       │  ③ Grouper: 策略分组+预算约束        │
                       │  ④ RuleMatcher: 文件特征→规则子集    │
                       └──────────────┬──────────────────────┘
                                      │ ReviewPlan (JSON)
                       ┌──────────────▼──────────────────────┐
                       │  执行层 (改造现有 spawn_subagent)     │
                       │  每个 bundle → 1 个 reviewer 子agent │
                       │  受限工具集 + file allowlist, 并发≤4  │
                       └──────────────┬──────────────────────┘
                                      │ LlmComment (结构化)
                       ┌──────────────▼──────────────────────┐
                       │  ⑤ Aggregator (纯 Rust)             │
                       │  行号校验→跨文件重定位→去重→分级报告   │
                       │  (可选) 反思子agent 过滤误报          │
                       └─────────────────────────────────────┘
```

五个环节中 ①②③④⑤ 全部是确定性 Rust 代码，LLM 只出现在执行层的评审判断
（和可选的反思环节）。

## 5. 模块设计

### 5.1 新增 `src/review/` 模块（纯 Rust，不调 LLM）

```
src/review/
  mod.rs          # ReviewPlan 数据结构与入口
  diff_provider.rs
  selector.rs
  grouper.rs
  rule_matcher.rs
  aggregator.rs
```

#### ReviewPlan

```rust
pub struct ReviewPlan {
    pub scope: ReviewScope,            // GitDiff { base: Option<String> } | Path(PathBuf)
    pub files: Vec<ReviewFile>,        // 入选文件及变更元数据
    pub excluded: Vec<ExcludedFile>,   // 排除文件 + 原因(可解释，对应 ocr ExcludeReason)
    pub bundles: Vec<Bundle>,          // 分组结果
}

pub struct ReviewFile {
    pub path: PathBuf,
    pub change_type: ChangeType,       // Added | Modified | Deleted | Renamed | Untracked
    pub hunks: Vec<Hunk>,              // 新旧行号区间
    pub added_lines: u64,
    pub deleted_lines: u64,
    pub language: Language,
}

pub struct Bundle {
    pub files: Vec<PathBuf>,
    pub estimated_tokens: u64,
    pub rules: Vec<String>,            // 本组命中的规则文档 id
}
```

#### diff_provider（对应 ocr internal/diff）

- `git diff --unified=0 --numstat` (+ `--staged`, untracked 用
  `git ls-files --others --exclude-standard`) 解析为结构化 hunks；
- 非 git 目录报错退出（评审无 diff 无意义），提示用 `scope=path` 走全文件扫描；
- **gitignore 感知**：复用现有工具层的 gitignore 遍历逻辑（file/io 已有），
  不重复造轮子。

#### selector（对应 ocr selection.go）

- 排除表（编译产物/依赖/lockfile/生成物/二进制/敏感文件）：

  ```
  target/, node_modules/, dist/, build/, __pycache__/, .venv/,
  *.lock, package-lock.json, Cargo.lock, *.min.js, *.map,
  .env*, *.key, *.pem, 图片/字体等二进制
  ```

  该表即现 SKILL.md 中的语言表格 → 固化为代码，**永不进 LLM 上下文**；
- 每个 `ExcludedFile` 携带 `ExcludeReason` 枚举（BuildArtifact / LockFile /
  SensitiveFile / Binary / Generated / UserExcluded），报告末尾汇总，
  保证"没审什么、为什么"可解释——这是 ocr 覆盖率可信的关键。

#### grouper（对应 ocr grouping.go 的确定性路径）

策略分组规则（全部纯代码，无 LLM）：

1. 同目录文件合组；
2. 配对规则：`*_test.rs` ↔ 被测模块、`.h` ↔ `.c/.cpp`、接口 ↔ 实现、
   i18n 同名资源对（`zh-CN/x` ↔ `en/x`）；
3. 预算硬约束（**对应 ocr enforceGroupTokenBudget，不可妥协**）：
   单组预估 token ≤ 上限（默认 ~8k diff 行预算，可配），超出按目录拆分；
   单组文件数上限（默认 10），超出强制拆分；
4. 复用 `agent/token_counter.rs` 做预估。

#### rule_matcher（对应 ocr system_rules + sniffer）

- 规则表：`Vec<(glob 模式集, 规则内容)>`，初版内置规则来源 = 现 SKILL.md 的
  "审查要点清单"按语言/特征拆分（Rust 生命周期与所有权、async 阻塞、C 内存、
  TS 类型与 XSS、Python 资源泄漏…）；
- 支持项目级自定义：`.dev-assistant/review-rules.toml`，格式对齐
  glob → 规则文档路径；
- 产出**规则子集**注入对应 bundle 的子 agent prompt —— 每个 bundle 只看到
  与自己相关的 5~10 条规则，而非全量清单（ocr "消除信息噪音"的核心手段）。

### 5.2 执行层改造（最小侵入）

#### spawn_subagent 增加 file allowlist

`tools/subagent.rs` 参数新增：

```json
{ "allowed_files": ["src/foo.rs", "src/foo_test.rs"],
  "system_rules": ["rust-ownership", "async-blocking"] }
```

`Agent::process_tool_calls` 拦截处理时将 allowlist 传入子 agent 构造，
子 agent 的读工具（read/batch_read/search）在 `ToolContext` 中校验：
- 允许读 allowlist 内文件 + 其直接依赖（按现有 file_dependencies 能力，
  简化实现：同目录 + mod 声明文件）；
- 越界读取返回明确错误信息，提示"仅允许评审指定文件"。

#### Reviewer 身份瘦身（agent/identity.rs）

- `default_tools` 删除 `kb_store`、`save_summary`、`compress_context`
  （bundle 足够小，压缩无必要；评审子 agent 不产生持久状态）；
  保留：read 类、search、exec_command、context_budget、finish；
- 新增 `code_comment` 工具（见下），产出结构化意见而非自由文本。

#### 新增 `code_comment` 工具（对应 ocr 同名工具）

```json
{ "file": "src/foo.rs", "line": 42, "severity": "high",
  "title": "未检查的 unwrap 可能 panic",
  "body": "…说明与建议…" }
```

- handler 做**确定性预校验**：file 必须在 allowlist、line 必须落在该文件
  实际 hunks 范围内，否则返回错误让 LLM 修正（第一次修正机会放在工具层，
  比"事后聚合时丢弃"成本低）；
- 收集到的 comments 直接进入 Aggregator，**finish 只交一句摘要**——
  彻底消除"报告超 2000 字写文件再引用"的现有绕行路径。

#### Reviewer 系统提示瘦身

现 SKILL.md 中防死循环条款全部删除（架构上不再可能：上下文按 bundle 隔离、
输入由确定性代码给全）。身份提示聚焦：评审维度 + code_comment 使用规范 +
"只报有把握的问题"（精度优先，对齐 ocr 的 trade-off）。

### 5.3 Aggregator（对应 ocr resolver + relocation + reflection）

纯 Rust 实现，顺序遵循"先便宜后昂贵"：

1. **行号校验**：每条 comment 的 line 对齐到实际 hunk（容差 ±3 行内的
   fuzzy 对齐：若目标行内容与 comment 引用代码片段匹配则接受）；
2. **跨文件重定位**：校验失败时，用 comment 中的代码片段在其它变更文件中
   精确搜索（`tools/file/search.rs` 已有实现可复用）；
3. **去重**：同文件 ±5 行、同 title 相似度 > 阈值的意见合并，保留严重度高者；
4. **（可选）反思子 agent**：仅输入 comment 清单（不含代码，token 极少），
   让其剔除明显误报、调整严重度 —— 对应 ocr reflection，作为 v2 增强；
5. **报告生成**：markdown 输出保持现有阅读习惯（严重度分级 + 代码片段），
   每条附加 `confidence: verified | relocated | unverified` 与命中的
   rule id；报告头部附选择统计（审了 N 个文件、排除 M 个及原因）。

## 6. 与现有机制的关系

| 现有机制 | 关系 |
|----------|------|
| `skills/code-review/SKILL.md` | 降级为入口说明文档（~20 行）：触发条件 + 流水线语义。87 行审查逻辑全部迁入 rule_matcher 内置规则 |
| `spawn_subagent` 后台并发（≤4） | 直接复用为 bundle 执行器 |
| `agent/compressor.rs` / `context_budget` | 保留，作为兜底（设计正确时 bundle 不会触发压缩） |
| `security/approval.rs` | 流水线整体入口走一次审批（告知将发起 N 个子 agent、预估 token），子 agent 内部读文件不再逐次审批 |
| `.kb/review/` | Aggregator 输出结构化结果后可写入，供 kb_query 复用 |
| scheduler / dream / hooks | 不涉及，本次改造不触碰 |

## 7. 分阶段实施计划

| 阶段 | 内容 | 规模 | 验收标准 |
|------|------|------|----------|
| P1 | `src/review/`: diff_provider + selector + grouper + ReviewPlan 输出（含单测） | ~600 行 + 测试 | 对 dev-assistant 自身仓库跑 `plan`，输出正确分组与排除原因 |
| P2 | spawn_subagent file allowlist + Reviewer 瘦身 + code_comment 工具 | ~200 行改动 | 子 agent 越界读被拒；comments 结构化收集 |
| P3 | Aggregator（校验/重定位/去重）+ 报告生成 | ~400 行 | 人为构造行号漂移 comment，验证 relocated 标记 |
| P4 | rule_matcher + 内置规则迁移（SKILL.md 内容拆分）+ 自定义规则加载 | ~300 行 | 不同语言 bundle 命中不同规则子集 |
| P5（增强） | 反思子 agent、scan 模式（按语言/目录 batch，对应 ocr scan） | ~300 行 | 全目录扫描去重有效 |

P1-P3 为核心闭环（可用），P4 补齐规则体系，P5 对齐 ocr 完整能力。

## 8. 效果度量

对齐 ocr 的证明方式，用 [AACR-Bench](https://huggingface.co/datasets/Alibaba-Aone/aacr-bench)
的子集做改造前后对比（可用其公开 ground-truth）：

- **Precision**：报告问题中真实缺陷占比（目标：显著高于纯 prompt 基线）；
- **Position accuracy**：`verified` 标记占比（纯 prompt 基线预期 < 50%）；
- **Token 成本**：每 PR 评审总 token（预期接近 ocr 的 ~1/9 量级改善）；
- **覆盖率**：入选文件中被实际评审的比例（目标 100%，由架构保证）。

## 9. 风险与对策

| 风险 | 对策 |
|------|------|
| LLM 辅助分组（ocr 有此路径）质量不稳 | 本设计只采用确定性分组；LLM 分组列为可选实验，失败自动回退 |
| allowlist 过严导致子 agent 缺上下文 | 允许读直接依赖；`context_budget` 兜底；allowlist 生成时包含关联测试/接口文件 |
| git diff 在超大 PR 上解析慢 | numstat 只取元数据，hunk 内容按 bundle 惰性加载 |
| 行号 fuzzy 对齐引入误判 | relocated 意见显式标注 confidence，用户可见可信度 |
| 与 ocr 工具化集成（前置讨论的 `code_review` 工具）冲突 | 不冲突：本设计是原生路线；若 ocr 已安装，可额外提供 `code_review_external` 工具走 ocr，二者共享 Aggregator 输出格式 |
