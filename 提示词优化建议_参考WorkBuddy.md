# 参考 WorkBuddy 优化 Dev-Assistant 提示词

> 分析时间：2026-10-21
> 前置阅读：`内置提示词优化分析报告.md`、`内置提示词优化分析报告_v2.md`、`记忆注入优化规划.md`
> 现状代码：`src/prompt.rs`(主系统提示词)、`src/agent/identity.rs`(6 种 Agent 身份)、`src/agent/pipeline_stages.rs`(流水线模板)

---

## 0. 核心洞察

WorkBuddy 的提示词之所以稳，不在于"写得多长"，而在于三件事：

1. **分层**：全局铁律（安全 / 合规）与任务路由 / 特定能力严格分离，互不污染。
2. **结构化**：用明确的区块（XML 标签 / 固定标题）把"必须永远遵守"和"按场景触发"区分开，模型遵从度更高。
3. **闭环**：显式定义了"循环 → 终止 → 呈现结果"的执行节拍（`agent_loop` / `result_presentation` / `final_answer_instructions`），让模型知道怎么开始、怎么停、怎么交付。

Dev-Assistant 当前提示词（`prompt.rs` + `identity.rs`）已具备很好的底层能力——安全策略、finish 决策树、上下文预算管理、kb 跨代理共享——但**缺的是"执行节拍"和"分层的显式化"**。这正是 WorkBuddy 最值得抄的部分。

> 注：WorkBuddy 的合规 / 区域 / 企业微信 / 邮箱等模块是通用助手专属，对纯编程工具 **不适用**，本文已剔除，只保留可迁移项。

---

## 1. 对照表：WorkBuddy 模式 → Dev-Assistant 现状 → 可迁移性

| WorkBuddy 模式 | Dev-Assistant 现状 | 可迁移 | 建议优先级 |
|---|---|---|---|
| `agent_loop`（显式迭代循环：分析 → 思考 → 选型 → 执行 → 观察 → 迭代 → 交付） | 仅有"工作流程"原则列表，无循环节拍 | 高 | **P0** |
| `working_modes`（Agent / Plan / Ask 三模式，Plan 先规划待确认） | 仅 CLI flag（`--no-approval`），无提示词级模式 | 高 | **P0** |
| `final_answer_instructions` / `result_presentation`（最终答复须自洽、携带折叠结果、精简） | 有"输出规范"但弱，未要求"自洽 / 携带关键证据" | 高 | P1 |
| `asking_questions`（何时问、如何给选项） | 仅一句"不确定：问用户" | 中 | P1 |
| `task_management`（多步任务建清单） | 无提示词指引（pipeline 属硬编排，非提示词级） | 中 | P1 |
| 安全分层（`personal_files_safety` 原则：禁区 / 只读扫描 / 警告确认 / 回收站而非删除 / 小批量） | 有审批 + gitignore，但缺"回收站而非删除""先备份" | 中 | P1 |
| `memory_system`（任务后写记忆的强制纪律） | 有 Dream + kb，但 prompt 未强制"做完即存" | 中 | P2 |
| 结构化区块（XML 标签包裹关键铁律） | 用 `##` 标题，铁律与建议混排 | 低-中 | P2 |
| 全局 / 任务分层（`content_policy` 永远在线，task 路由按需） | 全局规则与技能分节，边界已清晰 | 已具备 | — |
| Skill 加载纪律（命中即先加载） | 关键词自动激活，已具备 | 已具备 | — |
| 语言 / 区域 / 合规类模块 | 不适用（编程工具） | 不适用 | — |

---

## 2. 具体建议（按优先级）

### P0-1：补上"迭代执行循环"节拍（最大的缺口）

在 `prompt.rs` 核心原则里新增一节，把"工作"定义成**循环**而非清单。这是 WorkBuddy `agent_loop` 的精髓，能直接减少"没拿到工具结果就假设成功""中途提前 finish"等问题。

```text
## 执行循环
你以循环方式工作，直到任务完成：
1. 分析：明确当前状态与目标，判断还差什么。
2. 思考：决定下一步动作（调用工具 / 直接回答 / 提问）。
3. 行动：执行选定的动作。
4. 观察：读取结果，判断是否推进、卡住或需换路。
5. 迭代：重复 1-4，直到目标达成。
6. 交付：执行任务调 finish，纯问答直接回答，并清晰呈现结果。
⚠️ 不要在第 1-2 步就结束；没拿到工具结果前不要假设成功。
```

`identity.rs` 各身份可在 `shared_guide()` 里共用一个精简版循环，让子代理也有同样的节拍意识。

### P0-2：引入三模式（Agent / Plan / Ask）

对应 WorkBuddy 的 `working_modes`，用**提示词级概念**弥补 CLI flag 的不足，让破坏性操作天然获得"先对齐再执行"的护栏：

```text
## 工作模式
- Agent（默认）：直接动手完成任务。
- Plan：先输出方案 / 步骤，等用户确认再执行；涉及多文件改动、重构、删除时优先用 Plan。
- Ask：只答疑、不改动任何文件（用户说"只分析 / 只解释"时）。
切换：用户说"先规划""帮我看看"→ 进 Plan/Ask；说"直接做"→ 进 Agent。
```

这比单纯靠 `--no-approval` 审批更顺，也更贴合"改前先对齐"的工程直觉。

### P1-1：强化最终结果纪律

在"输出规范"补强（对标 `final_answer_instructions`）：

- `finish(summary)` 须 **自洽可读**：即便工具调用细节被折叠，用户只看 summary 也能懂做了什么、结论是什么。
- 携带关键证据（文件路径:行号、命令、核心数据），不要只写"已修复"。
- 精简但完整（3-10 行）；大报告写文件并在 summary 引用路径。

### P1-2：明确"何时提问"决策

```text
## 何时提问
- 需求模糊且方向不唯一 → 用选项式提问（给出 2-4 个方案）。
- 会触发破坏性 / 不可逆操作且意图不明 → 先问。
- 信息可从代码 / 已有记忆推断 → 不要问，直接基于合理判断并备注假设。
```

### P1-3：破坏性操作安全（借鉴 `personal_files_safety` 原则，适配项目目录）

在"安全策略"补充（注意：dev-assistant 操作的是项目目录，不照搬桌面禁区，但照搬"回收站而非删除 / 先备份"原则）：

- 删除文件前优先移到备份 / 回收站，而非物理删除。
- 批量改动前先做 `git stash` 或备份清单。
- 一次只改少量文件并验证，避免"大爆炸"式改动。

### P1-4：多步任务用清单

补充：任务 ≥ 3 个独立步骤或多文件改动时，先列出简短步骤计划，每步完成后核对，再继续。

### P2-1：任务后写记忆纪律

在"上下文预算管理"或新增"记忆"小节，强化：实质性工作完成后用 `kb_store` 沉淀关键决策 / 接口 / 踩坑，与现有 Dream 晋升逻辑衔接。

### P2-2：关键铁律结构化

把"安全策略 / finish 纪律 / 不读构建产物"用更醒目的固定区块（如 `### 铁律（永远遵守）`）与"建议"区分，提升遵从度——这是 WorkBuddy 用 XML 标签包裹关键指令的思路在 Markdown 下的等价做法。

---

## 3. 与既有分析报告的衔接

- **v2 报告 P1**（read_symbol 子代理缺失、预算工具进 `default_tools`、`tool_display_order` 同步）属于**实现一致性**问题，与本次"结构优化"正交，应并行推进。
- **v2 报告 P2**（shared_guide 过于简洁、finish_warning 冗长、后续阶段重复 task_ref）正好是"分层 / 精简"的落点——可借本次重写一并发。
- **v1 报告 P1 待修**（流水线阶段模板硬编码在 `run_pipeline()`）建议把 6 阶段模板提取为外部 `pipeline-stages.toml`（用 `include_str!` 或运行时加载），使其能像 WorkBuddy 那样"规则与代码解耦、可热更新"。

---

## 4. 落地顺序建议

1. **先加 P0**：执行循环 + 三模式骨架（纯 `prompt.rs` / `identity.rs` 文本改动，低风险高收益）。
2. **再补 P1**：结果纪律 + 何时提问 + 破坏安全 + 多步清单。
3. **实现项**：抽流水线模板到 TOML、补子代理循环（P1/P2）。
4. **收尾 P2**：记忆纪律与铁律结构化。

---

## 5. 一句话总结

Dev-Assistant 不缺"能力提示词"，缺的是 **WorkBuddy 那种"怎么开始 / 怎么停 / 怎么交付"的节拍感，以及"铁律 vs 建议"的分层显式化**。先把 P0 的循环和模式补上，整体稳定性会明显上一个台阶。

---

## 6. 实施记录（2026-10-21 · P0 已完成）

### 改动文件
- `src/prompt.rs`
  - 新增 `## 执行循环` 区块：分析 → 思考 → 行动 → 观察 → 迭代 → 交付 的显式循环，并加"未确认结果前不假设成功"护栏。
  - 新增 `## 工作模式` 区块：Agent / Plan / Ask 三模式，Plan 高风险操作先对齐。
  - 新增 2 个单测锁定：`build_prompt_includes_execution_loop`、`build_prompt_includes_working_modes`。
- `src/agent/identity.rs`
  - `shared_guide()` 补充精简版循环节拍（分析→思考→行动→观察→迭代，未确认结果前不假设成功），6 种子代理身份共用。

### 验证
- `cargo test`：497 passed / 3 failed。
- 3 个失败均为环境/网络相关旧问题（`step_empty_response_retries_are_capped` 实际发了 HTTP 请求、`hook_timeout_produces_error` 超时判定、`connect_error_is_network_class_for_retry` connection refused 分类），与本次文本改动无关；所有 prompt / identity 测试通过。

### 下一步（P1，待实施）
结果纪律强化、何时提问决策、破坏性操作"回收站而非删除/先备份"、多步任务建清单。

---

## 7. 实施记录（2026-10-21 · P0 + P1 已完成）

### P1 改动（`src/prompt.rs`）
- **结果纪律强化**（对标 `final_answer_instructions`）：`## 输出规范` 增加"自洽可读"——即使工具细节被折叠，summary 也能独立看懂；要求携带关键证据（文件:行号、命令、核心数据），不要只写"已修复/已完成"。
- **何时提问决策**（对标 `asking_questions`）：新增 `### 何时提问`，明确"模糊且方向唯一→选项式提问 / 破坏性且意图不明→先问 / 可从代码或 kb 推断→不问直接判断并备注假设"。
- **破坏性操作安全**（对标 `personal_files_safety` 适配项目目录）：`### 安全策略` 增加"删除先备份"（优先备份/回收站而非物理删除、批量改动前 git stash）、"小批量改动"（一次少量文件并立即验证）。
- **多步任务建清单**（对标 `task_management`）：`### 工作流程` 第 2 条改为"≥3 步或多文件改动先列步骤清单逐步核对"。
- 新增 4 个单测锁定：`build_prompt_includes_destructive_safety`、`build_prompt_includes_ask_guidance`、`build_prompt_includes_multistep_checklist`、`build_prompt_includes_self_contained_summary`。

### 验证
- `cargo test build_prompt`：15 passed / 0 failed（含 P0 的 2 个 + P1 的 4 个新增测试）。

### 剩余项
- P2：任务后写记忆纪律（`kb_store` 强制沉淀）、关键铁律结构化（铁律 vs 建议分区）。
- v2 报告待修项（实现一致性）：read_symbol 子代理缺失、预算工具进 `default_tools`、`tool_display_order` 同步、shared_guide 精简、finish_warning 冗长、流水线模板抽 TOML。

---

## 8. 实施记录（2026-10-22 · P0 + P1 + P2 已完成）

### P2 改动（`src/prompt.rs`）
- **记忆沉淀纪律**（对标 `memory_system`）：新增 `### 记忆沉淀`，要求实质性工作后用 `kb_store` 沉淀关键决策 / 接口 / 踩坑（写入前先 `kb_query` 查重），与内置 Dream 巩固协同。
- **铁律 vs 建议分区**（对标 WorkBuddy 显式分层）：`## 核心原则` 顶部新增 `### 铁律（永远遵守）`，列出 4 条不可违背规则（审批 / 不读构建产物 / 删除先备份与小批量 / 必须 `finish` 收尾），并注明其余小节为可灵活取舍的工作方式建议。
- 新增 2 个单测：`build_prompt_includes_immutable_rules`、`build_prompt_includes_memory_discipline`。

### 验证
- `cargo test build_prompt`：17 passed / 0 failed（P0:2 + P1:4 + P2:2 全部覆盖）。

### 剩余项（仅 v2 实现一致性，与本次结构优化正交）
- 已收尾，见第 9 节。

---

## 9. 实施记录（2026-10-22 · v2 实现一致性闭环）

把"参考 WorkBuddy"的 P0/P1/P2（结构优化）与 v2 报告的"实现一致性"待修项合并收尾。先逐条核验 v2 报告 9 项当前代码状态：

### 已在代码库中落地（本次会话前已修复，无需改动）
- **v2 P1#1** `read_symbol` 子代理注册表缺失 → `src/tools/mod.rs` 的 `new_subagent_registry()`（L397）与 `new_subagent_registry_with_identity()`（L433）已含 `"read_symbol"`。
- **v2 P1#2 / #3** 子代理 `default_tools()` 与 `tool_display_order()` 缺预算/符号工具 → `src/agent/identity.rs` 6 种身份 `default_tools()`（L220-323）与 `tool_display_order()`（L70-87）均已含 `read_symbol`/`context_budget`/`compress_context`/`save_summary`。
- **v2 P2#5** `finish_warning()` 冗长 → `src/agent/mod.rs:808` 当前已是精简版（约 40 字）。
- **v2 P3#8** `tool_display_order()` 与注册表不一致 → 随上面一并修复，已对齐。

### 本次会话落地（实质改动 3 项 + 轻量收尾 1 项）
- **v2 P2#4** 主提示词"特殊工具说明"缺 `context_budget` 等 → `src/prompt.rs` 新增 `context_budget/compress_context/save_summary` 条目（含用法指引）；加测试 `build_prompt_includes_context_budget_in_special_tools`。
- **v2 P2#6** 流水线后续阶段重复 `{task_ref}` → `src/agent/pipeline_stages.rs` 第四（审查）/五（修复）/六（记录）阶段模板移除 `{task_ref}` 整块、仅保留 `{context}`；阶段 1-3 保留原始需求定位。无测试依赖，安全。
- **v2 P2#7** `shared_guide()` 过于简洁 → `src/agent/identity.rs` 重写：箭头链改"工具优先级"自然语言、补"完成条件"、软化"不写中间文件"（仅大型交付物例外）；加测试 `shared_guide_includes_completion_condition_and_tool_priority`。
- **v2 P3#9** code-review 表格冗余 → `skills/code-review/SKILL.md` 表格下方加注"构建目录清单以主提示词安全策略为准，此处不再重复维护"。

### 验证
- `cargo test` → **505 passed / 3 failed**。3 个失败均为既有环境/网络旧问题（`step_empty_response_retries_are_capped` 连 127.0.0.1、`hook_timeout_produces_error`、`connect_error_is_network_class_for_retry`），与文本改动无关；历次新增 8 个测试（P0:2 + P1:4 + P2:2）外加本次 2 个全部通过。

### 结论
至此两份报告全部闭环：
- "参考 WorkBuddy"结构优化：P0（执行循环 + 三模式）、P1（结果纪律 / 何时提问 / 破坏安全 / 多步清单）、P2（记忆沉淀 / 铁律分区）。
- v2 实现一致性：9 项中 6 项此前已修复，本次完成 3 项实质改动 + 1 项轻量收尾。
- dev-assistant 提示词整改整体完成。
