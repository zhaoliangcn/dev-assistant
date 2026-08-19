# Dev-Assistant-RS CLI 界面升级方案

> 分析时间：2025-08-19  
> 参考已有方案：`docs/ui-upgrade-plan.md`（grok-build，Phase 1-4 已落地）、`docs/codewhale-ui-optimization-analysis.md`（T1-W4 已落地）  
> 本文聚焦**尚未覆盖的增量升级**。

---

## 一、现状盘点

### 1.1 已实现能力（不重复）

| 能力 | 实现位置 | 状态 |
|------|---------|------|
| 块级消息渲染（7 种类型） | `ui/blocks.rs` 556 行 | ✅ |
| Markdown 渲染 + 语法高亮 | `ui/markdown.rs` 625 行 | ✅ |
| 流式追加（非全屏重绘） | `ui/mod.rs::render_blocks` | ✅ |
| 进度条 + 终端 resize 响应 | `ui/mod.rs::render_progress_bar` | ✅ |
| 玻璃拟态（OSC 11 + alpha 混合） | `ui/translucent.rs` 219 行 | ✅ |
| 主题系统（语义色常量） | `ui/style.rs` + `ui/theme.rs` 476 行 | ✅ |
| 子代理实时输出（带深度缩进） | `ui/realtime_output.rs` 137 行 | ✅ |
| 输入系统（行编辑 + 历史） | `ui/input.rs` 440 行 | ✅ |
| 消息块合并（连续同类折叠） | `repl.rs::merge_consecutive_blocks` | ✅ |
| 状态类型分类（6 种） | `ui/mod.rs::StatusType` | ✅ |
| Slash 命令（15+ 条） | `repl.rs` / `main.rs` | ✅ |
| `/expand` 截断内容展开 | `ui/mod.rs::LAST_TRUNCATED_CONTENT` | ✅ |
| 暗/亮色 diff 渲染 | `ui/blocks.rs` | ✅ |

### 1.2 核心差距（增量空间）

```
当前渲染模型：
  Agent → display buffer → render_agent_messages() → render_blocks() → 追加到 stdout
                                            ↑ 纯流式，无固定区域

目标渲染模型：
  ┌─ 固定头部（状态栏）─────────────────────────┐
  │  Model: gpt-4o │  Tokens: 12K/262K (4.6%) │ Approval: ON │ Project: ... │
  ├─ 可滚动消息区（主内容）─────────────────────┤
  │  💭 正在分析代码...
  │  🔧 read_file (src/main.rs)
  │  ✅ Read success: 47 lines
  │  🤖 助手：[Markdown 渲染 + 语法高亮]
  │  ── Context Budget ──
  │  ████████████░░░░░░ 4.6% used
  ├─ 固定底部（输入区）─────────────────────────┤
  │  > your message here...
  └────────────────────────────────────────────┘
```

---

## 二、升级路线图

### Phase A — 固定状态栏（最高 ROI）

**为什么优先**：不改变现有流式渲染，只加一层固定区域。用户每时每刻都能看到关键信息。

**设计**：

```
┌──────────────────────────────────────────────────────────────┐
│ 🤖 gpt-4o  │  💬 12.4K / 262K (4.7%)  │  🔒 Approval: ON  │ 📁 /path/to/project │
└──────────────────────────────────────────────────────────────┘
```

| 字段 | 数据来源 | 更新频率 |
|------|---------|---------|
| 当前模型 | `agent.active_model()` | 切换时 |
| Token 用量 | `agent.context.budget_manager.report()` | 每轮工具调用后 |
| 审批模式 | `config.no_approval` | 启动时 |
| 项目路径 | `config.working_dir` | 启动时 |
| 子代理数量 | `agent.subagents_running` | 实时 |

**实现要点**：
- 使用 `\x1b[{n}A` 上滚光标，在消息上方插入/更新状态栏
- 终端 resize 时响应（已有 `get_terminal_width()`，扩展支持 `rows` 检测）
- 消息区渲染时在开头先渲染状态栏，底部渲染输入提示

**预计工作量**：2-3 天

---

### Phase B — 上下文预算可视化

**为什么优先**：`ContextBudget` 数据已计算但未展示，用户不知道什么时候该压缩。这是 Agent 自有的"仪表盘"。

**设计**（嵌入状态栏或消息区顶部）：

```
  Context Budget
  ╭──────────────────────────────────────────╮
  │  System:  2.1K (0.8%)                   │
  │  Memory:  0.4K (0.2%)                   │
  │  History: 12.4K (4.7%)                  │
  │  Tools:   3.2K (1.2%)                   │
  │  ───────────────────────────            │
  │  Total:   18.1K / 262K (6.9%)           │
  │  ████████████░░░░░░░░░░░░░░░░░░ 6.9%    │
  │  Status: Normal                         │
  ╰──────────────────────────────────────────╯
```

**交互**：
- 压力等级颜色编码：Normal=绿，Warning=黄，Critical=橙，Exhausted=红
- 超过 Critical 时状态栏闪烁提醒
- 可通过 `/budget` slash 命令展开详细视图

**实现要点**：
- 复用 `ContextBudget::report()` 已有方法
- 进度条复用 `render_progress_bar` 的实现模式
- 玻璃拟态模式下用 alpha 混合背景色

**预计工作量**：1-2 天

---

### Phase C — 交互式审批（修复 P1 遗留）

**为什么优先**：这是前次审查明确指出的"最大遗留问题"——`Press Enter to approve` 消息返回给 LLM，真正的用户交互从未实现。

**设计**：

```
  ⚠️  需要审批（High）
     命令: chmod 644 test.txt
     理由: chmod requires approval

     [Y] 批准    [N] 拒绝    [C] 取消
     批准有效期限: 1 小时（本次会话同类操作有效）
```

**交互**：
- `Y` / `Enter` → 批准，写入 PermissionStore（会话级，有效期按危险级别）
- `N` → 拒绝，该操作在本轮会话中标记为已拒绝
- `C` → 取消，返回到 LLM 让它重新思考
- 支持 `--no-approval` 直接跳过

**实现要点**：
- 在 `Agent::step()` 循环中检测 `security_evaluation`，暂停等待用户输入
- 使用 `ui/input.rs` 的键盘读取能力（已有，无需新引入依赖）
- 批准结果写入 `ApprovalManager::approve()`（框架已存在）

**预计工作量**：2-3 天

---

### Phase D — 流式 Token 计数器

**设计**：在 LLM 流式输出期间，状态栏的 Token 计数实时增加：

```
  🤖 思考中...  Tokens: 8.2K → 8.3K → 8.5K → ... (实时)
```

**实现要点**：
- 利用现有的流式 SSE 事件，每收到一个 chunk 更新计数
- 用 `\r` 回到行首更新数字（类似下载进度条）

**预计工作量**：0.5 天

---

### Phase E — 亮色主题 + 自动检测

**依据**：`codewhale-ui-optimization-analysis.md` 指出这是"最优先修复"（T1）。

**设计**：

| 能力 | 说明 |
|------|------|
| 亮色主题 | 为亮色终端提供可读的配色 |
| 自动检测 | `COLORFGBG` 环境变量 → macOS `AppleInterfaceStyle` → 默认暗色 |
| 高对比适配 | 边界颜色处理 |
| 暗色/亮色切换 | 运行时通过 `/theme` slash 命令切换 |

**实现要点**：
- 在 `ui/theme.rs` 中定义两套 `Theme` 结构体（Dark / Light）
- 新增 `ui/detect.rs`：读环境变量 + 系统偏好
- 所有 `render_blocks_to_string` 中的颜色调用点改为通过 `active_theme()` 取值

**预计工作量**：3-4 天

---

### Phase F — 可视化 Diff（inline 渲染）

**设计**：当前 diff 是纯文本，升级为行内着色：

```diff
  fn main() {
+     let x = 42;
-     let y = 0;
+     println!("{}", x);
  }
```

- 新增行：绿色背景 + 绿色 `+` 前缀
- 删除行：红色背景 + 红色 `-` 前缀
- 普通行：灰色
- 玻璃拟态模式下自动 alpha 混合

**实现要点**：
- 在 `ui/markdown.rs` 中增加 diff 代码块的特殊处理
- 复用 `syntect` 已有的 diff 主题
- 已有 `is_dangerous_file` 检测可复用为"敏感文件 diff 警告"

**预计工作量**：2 天

---

### Phase G — 流水线阶段可视化

**设计**：将 6 阶段流水线渲染为视觉流程图：

```
  🏗 架构设计 ✅ → 💻 代码实现 ✅ → 🧪 测试验证 ⏳ → 🔍 代码审查 ⬜ → 🔧 问题修复 ⬜ → 📋 进度记录 ⬜
  ──────────────────────────────── 当前: 🧪 测试验证 (3/6) ─────────────────
```

**状态色码**：
- ✅ 完成：绿色
- ⏳ 进行中：蓝色（带旋转动画 `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`）
- ⬜ 待处理：灰色
- ❌ 失败：红色

**预计工作量**：1-2 天

---

### Phase H — 键盘快捷键

**设计**：

| 快捷键 | 功能 |
|--------|------|
| `Ctrl+L` | 清屏（保留历史在滚动缓冲区） |
| `Ctrl+R` | 滚动到顶部 |
| `Ctrl+D` | 退出 |
| `Ctrl+T` | 切换暗/亮主题 |
| `Ctrl+B` | 展开/折叠上下文预算面板 |
| `Ctrl+/` | 显示/隐藏状态栏 |
| `Alt+Up` | 上一条历史消息 |
| `Alt+Down` | 下一条历史消息 |

**实现要点**：
- 复用 `ui/input.rs` 的键盘归一化
- 在主 REPL 循环中拦截快捷键

**预计工作量**：1-2 天

---

### Phase I — 子代理树可视化

**设计**：当多个子代理并行时，用树形结构展示层级：

```
  🌳 代理树
  ├── 🧠 Agent (main) [depth 0]
  │   ├── 🏗 Architect [depth 1] — 架构设计完成 ✅
  │   ├── 💻 Implementer [depth 1] — 代码实现 ⏳
  │   └── 🔍 Reviewer [depth 1] — 代码审查 ⬜
  └── 🧪 Tester [depth 1] — 测试验证 ⏳
```

**预计工作量**：2 天

---

## 三、优先级排序

| 优先级 | Phase | 影响 | 工作量 | 依赖 |
|--------|-------|------|--------|------|
| ⭐⭐⭐ | **A: 状态栏** | 每时每刻可见关键信息 | 2-3 天 | 无 |
| ⭐⭐⭐ | **B: 上下文预算** | 用户感知上下文压力 | 1-2 天 | 无（复用已有 API） |
| ⭐⭐⭐ | **C: 交互式审批** | 修复 P1 遗留的安全承诺 | 2-3 天 | 依赖 A（共享输入系统） |
| ⭐⭐ | **E: 亮色主题** | 亮色终端可用性 | 3-4 天 | 改动面大（所有颜色调用点） |
| ⭐⭐ | **F: 可视化 Diff** | 代码审查体验 | 2 天 | 无 |
| ⭐⭐ | **D: Token 计数器** | 流式体验 | 0.5 天 | 无 |
| ⭐ | **G: 流水线可视化** | 复杂任务可见性 | 1-2 天 | 无 |
| ⭐ | **H: 快捷键** | 操作效率 | 1-2 天 | 无 |
| ⭐ | **I: 子代理树** | 并行任务可见性 | 2 天 | 无 |

---

## 四、架构建议

### 4.1 渲染接口不变性

当前 `render_blocks(blocks: &[MessageBlock])` 已设计为纯追加渲染。Phase A（状态栏）需要在消息上方插入固定区域，建议：

```
// 新接口：支持固定区域
pub fn render_screen(
    status_bar: &StatusBar,       // 固定头部
    blocks: &[MessageBlock],      // 可滚动消息
    input_prompt: &str,           // 固定底部
) -> io::Result<()>;

// 旧接口保留，内部委托
pub fn render_blocks(blocks: &[MessageBlock], md: &MarkdownRenderer) -> io::Result<()>;
```

### 4.2 依赖不变性

**不引入任何新依赖**。所有升级均可用现有的：
- `unicode-width`（Cjk 宽度计算）
- `syntect`（语法高亮）
- `pulldown-cmark`（Markdown 解析）
- ANSI escape codes（终端控制）
- `libc`（终端尺寸检测）

### 4.3 模块划分

```
ui/
├── mod.rs              — 渲染入口（render_screen / render_blocks）
├── blocks.rs           — 消息块类型（已存在）
├── markdown.rs         — Markdown 渲染（已存在）
├── status_bar.rs       — 新增：固定状态栏
├── budget_widget.rs    — 新增：上下文预算小部件
├── approval_prompt.rs  — 新增：交互式审批 UI
├── theme.rs            — 扩展：亮色主题 + 自动检测
├── detect.rs           — 新增：终端模式检测（暗/亮）
├── keyboard.rs         — 新增：快捷键处理
├── input.rs            — 已有：行编辑
├── style.rs            — 扩展：主题色常量
├── realtime_output.rs  — 已有：子代理输出
├── translucent.rs      — 已有：玻璃拟态
```

---

## 五、总工作量估算

| 阶段 | 工作量 |
|------|--------|
| Phase A（状态栏） | 2-3 天 |
| Phase B（上下文预算） | 1-2 天 |
| Phase C（交互式审批） | 2-3 天 |
| Phase D（Token 计数器） | 0.5 天 |
| Phase E（亮色主题） | 3-4 天 |
| Phase F（可视化 Diff） | 2 天 |
| Phase G（流水线可视化） | 1-2 天 |
| Phase H（快捷键） | 1-2 天 |
| Phase I（子代理树） | 2 天 |
| **合计** | **15-20 天** |

前 3 个 Phase（A+B+C，7 天）可覆盖 80% 的用户体验提升。
