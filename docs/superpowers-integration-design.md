# Superpowers 能力集成设计文档

> 状态：设计中 | 创建日期：2026-08-04 | 最后更新：2026-08-04

## 一、背景

[Superpowers](https://github.com/obra/superpowers) 是一套完整的软件开发方法论技能包，
以可组合的 skills 形式交付（brainstorming → writing-plans → test-driven-development →
subagent-driven-development → code-review → verification 等 14 个技能），已在
Claude Code、Cursor、Codex、Gemini、Kimi、OpenCode、pi 等十余个宿主上完成移植。

dev-assistant 已具备技能系统（SKILL.md 解析、关键词匹配激活、Git/本地源安装），
但缺少 superpowers 移植的核心机制——**会话启动时自动注入 bootstrap**。

## 二、目标

1. 将 superpowers 的技能能力集成到 dev-assistant
2. 满足 superpowers 移植的验收标准：每次会话自动加载 bootstrap，零 opt-in
3. 不修改 superpowers 技能正文（遵守其"技能描述动作、不指名工具"的铁律）

## 三、superpowers 移植三要素

| 组件 | 说明 | dev-assistant 现状 |
|------|------|-------------------|
| 技能本体 `skills/*/SKILL.md` | 与宿主无关，只描述动作 | ✅ 格式兼容（子目录 + SKILL.md） |
| 工具映射 `<harness>-tools.md` | 动作词汇 → 宿主真实工具名 | ❌ 需新增 |
| Bootstrap 注入器 | 会话开始注入 `using-superpowers` 全文 | ❌ 需新增（依赖 hook 机制） |

铁律：① 永不改写技能正文适配宿主；② 通过宿主自己的安装机制交付，不碰用户配置文件。

## 四、集成方案

### 4.1 交付层（几乎零成本）

利用已实现的技能安装器直接导入：

```bash
# 本地源
dev-assistant skill add ~/code/superpowers/skills --skill brainstorming --skill writing-plans ...

# Git 源
dev-assistant skill add https://github.com/obra/superpowers --skill systematic-debugging
```

注意事项：

- **不要一次全装**：14 个技能全部进入系统提示词会撑爆上下文。建议核心 6 个：
  `brainstorming`、`writing-plans`、`executing-plans`、`test-driven-development`、
  `systematic-debugging`、`subagent-driven-development`
- **关键词误触发**：superpowers 技能 frontmatter 无 `when_to_use` 字段，
  `compute_keywords` 只能从名字拆词，`subagent-driven-development` 会拆出
  `subagent`/`driven`/`development` 等高频词导致 `match_skill` 乱激活。
  需对无 `when_to_use` 的技能降级为"仅模型判断"。

### 4.2 Bootstrap 注入层（核心缺口）

通过通用 hook 机制实现，详见《Hook 机制设计文档》（`docs/hook-design.md`）。

具体流程：

1. **安装 bootstrap**：`dev-assistant skill add ~/code/superpowers/skills --skill using-superpowers`
   → skill installer 自动将 `hooks/session-start` 脚本复制到 `.dev-assistant/hooks/`，
   并注册到 `.dev-assistant/hooks.yaml`（YAML 格式，由 installer 管理）
2. **会话启动**：`App::build()` 加载 `hooks.yaml`，执行 shell hook
   → hook 脚本读取 `skills/using-superpowers/SKILL.md`，输出 bootstrap JSON
3. **注入**：hook 输出作为一条独立的 `system` 角色消息追加到 `ContextManager`（紧跟在
   系统提示词之后、用户消息之前），不污染核心系统提示词
4. **开关**：`--no-hooks` 参数可关闭所有 hook 执行

这样完全满足铁律②（通过宿主自己的安装机制交付，不碰用户配置）。

### 4.3 触发策略调整

| 现状 | 建议 |
|------|------|
| `match_skill` 对无 `when_to_use` 的技能做名字拆词 | 对无 `when_to_use` 的技能**禁用关键词激活**，只进系统提示词清单，由模型判断 |
| `format_skills_for_prompt` 全部列出 | bootstrap 技能全文注入，其余技能只列 name+description |
| 注入技能正文时不带映射 | 激活注入时同样附上工具映射段落 |

### 4.4 工具映射（新增 `references/dev-assistant-tools.md`）

| superpowers 动作 | dev-assistant 工具 | 备注 |
|---|---|---|
| read a file | `read_file` / `batch_read_files` | ✅ 已有 |
| edit / write | `edit_file` / `write_file` | ✅ 已有 |
| run shell command | `exec_command` | ✅ 已有 |
| dispatch a subagent | `spawn_subagent` | ✅ 已有（SDD 核心依赖齐了） |
| track progress | 降级：计划文件 / `task_status` | 无 todo 工具 |
| web fetch / search | 降级 | 无此工具 |
| mark task complete | `finish` | ✅ 已有 |

### 4.5 SDD 是最大增量价值

`spawn_subagent` + `analyze_codebase` + `schedule_task` 已存在，SDD
（subagent-driven-development）的 task brief / review-package 工作流可直接映射，
无需新造机器。

## 五、验收标准

按 superpowers `docs/porting-to-a-new-harness.md` 的 Definition of done：

1. `using-superpowers` bootstrap 每次会话启动自动加载，无 per-session opt-in
2. 工具映射存在（`references/dev-assistant-tools.md`）
3. 测试：`tests/dev-assistant/` 跑一次真实会话，断言 bootstrap 全文出现在提示词中、
   模型能列出已装技能

## 六、实施顺序

| 阶段 | 内容 | 改动量 |
|------|------|--------|
| Phase 0 | `skill add ~/code/superpowers/skills` 试装，验证 discover 兼容性 | 0 代码 |
| Phase 1 | hook 机制（见 docs/hook-design.md）或 bootstrap 标记 + 全文注入 | 小 |
| Phase 2 | 关键词匹配对无 `when_to_use` 技能降级 | 小 |
| Phase 3 | SDD 深度对齐：task brief 文件、spawn_subagent 参数、review 循环 | 中 |
| Phase 4 | 测试 + 文档，反向登记到 superpowers harness 表 | 中 |

## 七、风险

- using-superpowers 教模型"任何回复前必须查技能"，与 dev-assistant 现有
  "工具先行 + 关键词激活"风格有张力，Phase 1 落地后需跑真实会话观察调整
- superpowers 技能正文不可修改，若其动作词汇与 dev-assistant 工具差异过大，
  只能通过工具映射补充说明
