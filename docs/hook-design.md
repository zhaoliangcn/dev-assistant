# Hook 机制设计文档

> 状态：设计中 | 创建日期：2026-08-04 | 最后更新：2026-08-04

## 一、背景

当前 dev-assistant 的会话启动流程：

```
main.rs → App::build() → build_system_prompt() → run_interactive() / run_once()
```

关键缺口：**没有"在模型看到任何消息之前，把外部内容注入上下文"的机制**。

- `build_system_prompt()` 硬编码了系统提示词结构，无法由外部脚本或插件扩展
- 技能安装器（`src/skills/installer.rs`）能把技能文件装进 `skills/`，但没有入口点
  让装进来的技能主动注入内容
- superpowers 集成的验收标准是"每次会话自动注入 bootstrap，零 opt-in"，现有架构无法满足

本机制是《Superpowers 能力集成设计文档》（`docs/superpowers-integration-design.md`）
的基础前置：hook 是 bootstrap 注入的通用实现方式。

## 二、参考：主流宿主的 hook 形态

| 形状 | 代表宿主 | 原理 |
|------|----------|------|
| Shape A（shell hook） | Claude Code、Cursor、Copilot CLI | 会话启动执行 shell 脚本，脚本输出特定 JSON 到 stdout，宿主解析后注入 |
| Shape B（in-process callback） | OpenCode、pi | 插件注册回调，生命周期事件触发时直接修改消息数组 |
| Shape C（instructions file） | Gemini CLI | 已安装扩展的清单声明 `contextFileName`，宿主自动加载 |

dev-assistant 是 Rust 原生应用，采用 **Shape A + Shape B 混合**：对外提供 shell hook
（灵活），对内提供 inline hook（高效，用于第一方扩展）。

## 三、设计决策（已确认 ✅）

| 决策点 | 选择 | 说明 |
|--------|------|------|
| 配置格式 | **YAML** | `.dev-assistant/hooks.yaml`，与 SKILL.md frontmatter 一致 |
| 事件粒度 | **仅 `session-start`** | 最小可行版本，后续再扩展 |
| 注入位置 | **独立于系统提示词** | hook 输出作为一条 `system` 角色的消息，追加到 ContextManager |
| 脚本存放 | **installer 管理** | 固定在 `.dev-assistant/hooks/`，由 `skill install/remove` 自动维护 |
| 开关控制 | **CLI 参数 `--no-hooks`** | 默认启用，加 `--no-hooks` 关闭所有 hook 执行 |

## 四、设计

### 4.1 核心概念

- **Hook**：在 session-start 事件发生时执行的命令，其输出被注入模型上下文
- **HookManager**：负责加载配置、执行 hooks、收集输出

### 4.2 配置格式（YAML）

配置文件 `.dev-assistant/hooks.yaml`（项目级，由 skill installer 管理）：

```yaml
hooks:
  - name: using-superpowers
    event: session-start
    type: shell
    command: ./.dev-assistant/hooks/session-start
    timeout: 10
    priority: 1
    wrap_tag: SUPERPOWERS_BOOTSTRAP
    max_output_bytes: 8192
```

全局级配置 `~/.dev-assistant/hooks.yaml` 与项目级合并，项目级优先级更高。

### 4.3 执行模型

```
App::build()
  │
  ├── 1. 检查 AppConfig.hooks_enabled（--no-hooks 时跳过后续全部）
  ├── 2. 加载 hooks 配置（项目级 + 全局级合并）
  ├── 3. 发现技能、构建基础系统提示词（现有逻辑不变）
  ├── 4. HookManager::execute()
  │     ├── 按 priority 排序
  │     ├── 每项 shell hook: spawn 进程 → 捕获 stdout
  │     │     └── 超时自动 kill → 空结果
  │     └── 收集所有 hook 输出（带 name 标签）
  ├── 5. 将 hook 输出格式化为一条 system 消息，追加到 ContextManager
  └── 6. 正常创建 Agent
```

### 4.4 注入方式

hook 输出**不追加到系统提示词字符串**，而是作为一条独立的 `system` 角色消息加入
对话历史，紧跟在系统提示词之后、用户消息之前：

```
ContextManager 中的消息顺序：
  1. [system] 系统提示词（核心规则 + 技能列表）
  2. [system] <HOOK name="using-superpowers" type="shell">
     <EXTREMELY_IMPORTANT>
     You have superpowers.
     ...
     </EXTREMELY_IMPORTANT>
     </HOOK>                          ← hook 注入
  3. [user] 用户的第一条消息
```

这样：
- 模型能区分"核心规则"和"hook 注入的上下文"
- hook 内容不污染系统提示词的结构
- 后续可单独清除/刷新 hook 注入而无需重建系统提示词

### 4.5 安全设计

| 风险 | 应对 |
|------|------|
| hook 脚本恶意修改 | shell hook 仅执行 `command`，args 以数组传递，不解析 shell 元字符 |
| 无限循环 / 死锁 | 每个 hook 有 `timeout`（默认 5s），超时强制 kill |
| 注入内容过大撑爆上下文 | 每个 hook 有 `max_output_bytes`（默认 4096），超长截断 |
| 权限提升 | hook 以当前进程身份运行，权限不提升 |

### 4.6 CLI 开关

```rust
// main.rs
struct Cli {
    #[arg(long)]
    message: Option<String>,
    // ... 现有字段 ...

    /// 禁用 hook 机制（session-start hook 不会执行）
    #[arg(long)]
    no_hooks: bool,
}
```

```rust
// app.rs
pub struct AppConfig {
    pub working_dir: PathBuf,
    // ... 现有字段 ...
    pub hooks_enabled: bool,  // 默认 true
}
```

`--no-hooks` 传入时 `hooks_enabled = false`，`App::build()` 跳过 hook 加载和执行。

## 五、实现位置与模块结构

```
src/hooks/
├── mod.rs           # HookManager struct, HookConfig, HookResult
├── config.rs        # 加载 hooks.yaml，合并项目级 + 全局级
├── shell.rs         # ShellHook: 执行进程、捕获输出、超时控制
└── error.rs         # 专用错误类型
```

现有文件改动量：

| 文件 | 改动 | 规模 |
|------|------|------|
| `src/main.rs` | Cli 结构体加 `--no-hooks` 字段，传给 AppConfig | ~3 行 |
| `src/app.rs` | AppConfig 加 `hooks_enabled`；`build()` 中加载配置 + 执行 + 注入 | ~25 行 |
| `src/prompt.rs` | 无改动（hook 输出不注入系统提示词） | 0 |
| `Cargo.toml` | 新增 `serde_yaml` 依赖 | 1 行 |

## 六、与 skill installer 的联动

`skill install` 安装技能时，若技能目录包含 `hooks.yaml` 或 `hooks/` 子目录：

1. 复制 `hooks/` 下的脚本到 `.dev-assistant/hooks/`
2. 读取 `hooks.yaml`，合并到项目的 `.dev-assistant/hooks.yaml`
3. 同名 hook 自动覆盖（按 name 去重）

`skill remove` 时：

1. 移除 `.dev-assistant/hooks/` 下对应的脚本
2. 从 `.dev-assistant/hooks.yaml` 删除对应条目

```bash
dev-assistant skill add ~/code/superpowers/skills --skill using-superpowers
# → 自动注册 using-superpowers 的 session-start hook
```

## 七、实施顺序

| 阶段 | 内容 | 产出 |
|------|------|------|
| Phase 1 | `src/hooks/` 模块：`HookManager` + `session-start` + `shell` 类型 | Hook 可执行并注入为 system 消息 |
| Phase 2 | `hooks.yaml` 加载：项目级 + 全局级合并 | 用户可手写配置 |
| Phase 3 | skill installer 集成：自动注册/卸载 hook 条目 | `skill add using-superpowers` 一键完成 |
| Phase 4 | 测试 + `--no-hooks` 验证 | 完整功能可交付 |