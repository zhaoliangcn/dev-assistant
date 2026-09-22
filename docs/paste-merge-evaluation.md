# 粘贴合并可行性评估

日期：2026-09-17
范围：`src/ui/input.rs` 多行粘贴合并方案
状态：评估完成，给出推荐方案

---

## 背景

此前曾尝试用「后台线程 + 80ms 时间窗口」实现多行粘贴合并，但引入三大高危缺陷：

| 缺陷 | 说明 |
|---|---|
| 🔴 Drop 挂死 | `Drop::drop` 中 `handle.join()` 永久阻塞：主循环 `/exit` 退出后 drop 后台线程，而线程阻塞在 `readline` 等待输入，程序无法退出 |
| 🔴 动态 prompt 失效 | `read_line(prompt)` 参数被忽略、线程硬编码 `"> "`，T5 动态提示符（模式/模型/消息数）和 `/model` 的 `"  > "` 均丢失，行为回归 |
| 🔴 终端交错/竞态 | reader 线程每行后立即重渲染 `"> "`，LLM 输出中途提示符可能提前出现；type-ahead 积压改变输入语义 |

**结论：后台线程 + 时间窗口方案不可行，应放弃。**

## 关键发现：rustyline 已原生支持

调研 rustyline 12.0.0 源码，多行粘贴**本就由库本身解决**：

1. **Bracketed paste（Unix 默认开启）**
   - `Config::default()`：`enable_bracketed_paste: true`
   - 现代终端（iTerm2 / GNOME Terminal / Windows Terminal / VS Code 终端 / Alacritty 等）粘贴多行时，
     以 `\x1b[200~ ... \x1b[201~` 包裹，rustyline 将其作为**单个 `Cmd::Insert`** 一次性插入 buffer，
     **不会逐行触发 Enter**。
   - 因此一次 `readline()` 直接返回完整多行字符串 → **天然合并**。

2. **Windows**
   - `PasteFromClipboard` 命令（`lib.rs:767-770`）调用 `edit_yank(...)` **一次性**粘贴整个剪贴板内容，
     同样不会逐行提交。

3. **Validator 多行编辑（原生扩展点）**
   - Enter 绑定 `Cmd::AcceptOrInsertLine`，行为由 `Validator::validate()` 决定：
     - 返回 `ValidationResult::Incomplete` → 插入 `\n`，继续编辑（`command.rs:125-133`）
     - 返回 `ValidationResult::Valid` → 提交当前 buffer
   - 内置 `MatchingBracketValidator`（括号未闭合 → Incomplete），可参考。

## 结论

**当前代码（已恢复的原始版本）在主流终端下已经正确合并多行粘贴，无需任何改动。**

"逐行触发多条 LLM 请求"仅发生在**不支持 bracketed paste 的老旧终端**，属边缘场景，
不值得为它引入后台线程的复杂度与高危缺陷。

## 可选增强方案（按推荐度排序）

### 方案 1：零改动（推荐）
- 依赖 rustyline 默认 bracketed paste。
- 适用：现代主流终端，覆盖绝大多数用户。
- 成本：0；风险：0。

### 方案 2：Validator 支持手动多行输入（可选增强）
- 给 `SlashHelper` 实现 `Validator`，当 buffer 未闭合（如括号不平衡、以 `\` 结尾等）返回 `Incomplete`，
  让用户用 **Alt-Enter / 续行** 编辑多行后一次提交。
- **优点**：不碰后台线程，无竞态/挂死；保持动态 prompt；纯同步。
- **代价**：改变了单行交互（回车语义受 validator 影响），需谨慎定义"未闭合"规则，避免误拦截普通输入。
- 与粘贴合并目标无直接关系（bracketed paste 已解决），属体验锦上添花。

### 方案 3：仅非 tty/管道模式的吸收窗口（兜底）
- 在 `read_line` 返回后，仅当 stdin 非 tty（管道/文件）时，用很短窗口吸收后续行合并。
- **优点**：避开交互竞态（非 tty 无 prompt 重绘问题）。
- **代价**：对不支持 bracketed paste 的交互终端仍无效；覆盖面小，收益有限。

## 建议

- **直接采纳方案 1**，回退现有改动（已完成），保持 `src/ui/input.rs` 原始版本。
- 若后续确实需要手动多行编辑，优先方案 2 用 `Validator` 实现，切勿再引入后台线程。
- `run.sh` 权限位 `100755 → 100644` 的改动仍待确认是否有意（不影响代码逻辑）。
