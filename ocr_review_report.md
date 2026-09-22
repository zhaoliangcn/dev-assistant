# OpenCodeReview 代码审查报告

**审查工具**：`opencodereview-windows-amd64.exe` v1.12.4  
**目标仓库**：`D:\dev-assistant`  
**审查模式**：workspace（工作区变更：staged + unstaged + untracked）  
**LLM 模型**：`deepseek-v4-flash`（via `https://token.sensenova.cn`）  
**Session ID**：`868dd5b0-3d16-4597-a930-143aae9d5a8d`  
**执行时间**：约 1m36s

---

## 一、审查范围

| 文件 | 状态 | 变更量 | 是否审查 |
|------|------|--------|----------|
| `run.sh` | modified | +0 / -0（仅 mode 变化） | ✅ |
| `run.ps1` | added | +12 / -0 | ✅ |
| `run.bat` | added | +6 / -0 | ❌ 排除：unsupported_ext |
| `docs/deterministic-review-pipeline-design.md` | added | +325 / -0 | ❌ 排除：unsupported_ext |

**统计**：2 个文件被审查 / 4 个候选文件；总插入 343 行，总删除 0 行。

---

## 二、审查发现（2 条）

### 🔶 Finding 1 — `run.sh` 可执行位被移除（medium / other）

**位置**：`run.sh`（mode-only diff）

**问题描述**：  
该变更将 `run.sh` 的文件模式从 `100755` 改为 `100644`，移除了可执行位。但脚本自身的用法示例（`./run.sh ...`）和 shebang（`#!/usr/bin/env bash`）都假定它会被直接调用。变更后用户执行 `./run.sh` 会得到 `Permission denied`，除非显式调用 `bash run.sh`。

**建议**：
- 若非有意为之，恢复 `100755`：`chmod +x run.sh` 或 `git update-index --chmod=+x run.sh`
- 若有意移除，同步更新 `run.sh` 与 `run.ps1` 中的用法示例，改为 `bash run.sh ...`

---

### 🔶 Finding 2 — `run.ps1` 配置路径与工作目录耦合（medium / bug）

**位置**：`run.ps1:12`

**问题描述**：  
二进制通过绝对路径调用（`$ScriptDir\target\debug\dev-assistant.exe`），但配置文件路径 `.dev-assistant-models.toml` 是相对路径，相对于**当前工作目录**。当脚本从项目根目录以外的位置调用时（例如 Windows 下双击、或从其他目录 `& .\run.ps1`），会出现两种风险：
1. 找不到配置文件，导致启动失败；
2. 更糟的是，如果 CWD 中恰好存在同名文件，会**静默加载错误的配置**。

**建议代码**：
```powershell
# 原代码
& "$ScriptDir\target\debug\dev-assistant.exe" --config .dev-assistant-models.toml --max-tokens 1000000 @args

# 建议
& "$ScriptDir\target\debug\dev-assistant.exe" --config "$ScriptDir\.dev-assistant-models.toml" --max-tokens 1000000 @args
```

---

## 三、执行统计

| 指标 | 数值 |
|------|------|
| 审查文件数 | 2 |
| 评论数 | 2 |
| 总 Token | 41,794（输入 33,534 / 输出 8,260 / 缓存读取 19,968） |
| 工具调用总数 | 11（file_find 5、file_read 4、code_comment 1、code_search 1） |
| 工具失败 | 1（`code_search`：`git grep` 不支持 `--max-count`，git 2.37.0 版本过低） |
| 耗时 | 1m36s |

---

## 四、运行过程中的警告

1. **Git 版本过低**：`git 2.37.0 is older than the minimum supported version 2.41.0` — 建议升级 Git。
2. **`code_search` 工具失败**：`git grep` 报错 `unknown option 'max-count'`，因 git 版本过低不支持该选项。
3. **LLM 限流**：第 2 轮 review 遭遇 `429 Too Many Requests`（`rpm exhausted`），共重试 18 次，最终 1 个请求失败、4 个恢复。
4. **排除的文件**：`run.bat` 和 `docs/deterministic-review-pipeline-design.md` 因扩展名不在支持列表中未审查（`.bat`、`.md`）。

---

## 五、后续建议

- **修复 Finding 1**：`git update-index --chmod=+x run.sh`
- **修复 Finding 2**：将 `run.ps1` 第 12 行的 `--config` 参数改为 `"$ScriptDir\.dev-assistant-models.toml"`
- **升级 Git** 至 ≥ 2.41.0，可消除 `code_search` 失败和版本警告
- 若需审查 `.md` 文档，可通过 `--rule` 自定义规则启用扩展名支持

---

**原始 JSON 输出**：`D:\dev-assistant\ocr_review_result.json`
