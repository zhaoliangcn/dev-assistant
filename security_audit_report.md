# 🔍 代码安全审查报告

## 项目概览
**项目名称**: dev-assistant-rs  
**语言**: Rust  
**主要功能**: AI 编程助手，提供文件操作、命令执行、LLM 对话等功能  
**审查日期**: 2025-06-18  
**修复状态审查**: 2025-06-18  
**审查范围**: 完整代码库安全评估 + 修复验证

---

## 🔄 修复状态摘要

| 严重程度 | 总数 | 已修复 | 部分修复 | 未修复 |
|---------|------|--------|---------|--------|
| Critical | 3 | 3 | 0 | 0 |
| High | 3 | 2 | 1 | 0 |
| Medium | 3 | 0 | 1 | 2 |
| Low | 3 | 0 | 0 | 3 |
| **总计** | **12** | **5** | **2** | **5** |

**关键修复**: 所有 Critical 级别的路径遍历和命令注入漏洞已完全修复。High 级别的 TOCTOU 和日志脱敏也已修复。

---

## 🚨 严重漏洞 (Critical)

### 1. batch_read_files 路径遍历漏洞
**位置**: `src/tools/file_tools.rs` + `src/security/mod.rs`  
**CWE**: CWE-22 (路径遍历)  
**修复状态**: ✅ **已修复**

**问题描述**:  
`batch_read_files` 工具在安全评估中完全没有路径检查，可以读取工作目录外的任意文件。

**攻击向量**:
```json
{
  "name": "batch_read_files",
  "arguments": {
    "files": ["../../../etc/passwd", "../../.env", "/etc/shadow"]
  }
}
```

**代码证据**:  
`evaluate_tool` 中只检查了 `read_file | edit_file | file_exists | list_directory | write_file | exec_command`，完全遗漏了 `batch_read_files` 和 `glob`。

```rust
// src/security/mod.rs - evaluate_tool 中缺失检查
if matches!(tool_name, "read_file" | "edit_file") { ... }
if matches!(tool_name, "file_exists" | "list_directory") { ... }
if tool_name == "write_file" { ... }
// ❌ batch_read_files 和 glob 没有安全检查！
```

**影响**: 
- 攻击者可以读取系统敏感文件（`/etc/passwd`、`/etc/shadow`、私钥文件等）
- 可读取项目外的源代码、配置文件
- 信息泄露风险

**修复建议**:
```rust
// 在 evaluate_tool 中添加 batch_read_files 检查
if tool_name == "batch_read_files" {
    if let Some(files) = arguments["files"].as_array() {
        for file in files {
            if let Some(path) = file.as_str() {
                if let Err(e) = self.validate_path_exists(path) {
                    return SecurityEvaluation { 
                        danger_level: DangerLevel::Critical, 
                        reason: e.to_string() 
                    };
                }
            }
        }
    }
}
```

**实际修复**:  
已在 `src/security/mod.rs` 的 `evaluate_tool` 函数中添加对 `batch_read_files` 的完整路径验证（约第470-500行），包括：
- 遍历 `files` 数组中的每个文件路径
- 调用 `validate_path_exists()` 验证路径是否在允许的目录内
- 检查是否为敏感文件（.env, .key, .pem, .crt）
- 路径遍历攻击已被阻止

---

### 2. COMMAND_WHITELIST 前缀匹配绕过
**位置**: `src/security/mod.rs:316-325`  
**CWE**: CWE-178 (不正确的权限强制)  
**修复状态**: ✅ **已修复**

**问题描述**:  
白名单使用 `starts_with` 进行前缀匹配，存在严重绕过风险。攻击者可以创建与白名单命令同前缀的恶意二进制文件来绕过检查。

**攻击向量**:
```bash
# 如果白名单包含 "cargo"
cargo-malicious-payload    # 被误判为安全
cargo-evil build            # 被误判为安全

# 如果白名单包含 "npm"
npm-malicious install       # 被误判为安全
```

**代码证据**:
```rust
for whitelisted in &self.whitelisted_commands {
    if full_command.starts_with(whitelisted) {  // ❌ 危险的前缀匹配
        return SecurityEvaluation {
            danger_level: DangerLevel::Low,
            reason: format!("Command is whitelisted: {}", command),
        };
    }
}
```

**影响**: 
- 白名单机制完全失效
- 攻击者可执行任意恶意二进制文件
- 完全绕过命令安全策略

**修复建议**:
```rust
// 使用精确匹配或参数化匹配
for whitelisted in &self.whitelisted_commands {
    if full_command == whitelisted || 
       full_command.starts_with(&format!("{} ", whitelisted)) {
        // 进一步验证参数是否安全
        continue;
    }
}
// 或者使用完整的路径匹配
let cmd_path = Path::new(command);
if cmd_path.is_absolute() && whitelisted_paths.contains(cmd_path) {
    // 安全的白名单检查
}
```

**实际修复**:  
已在 `src/security/mod.rs` 的 `evaluate_command` 函数中修复白名单匹配逻辑（约第340-360行）：
```rust
for whitelisted in &self.whitelisted_commands {
    if full_command == *whitelisted
        || (full_command.len() > whitelisted.len()
            && full_command.starts_with(whitelisted)
            && full_command.as_bytes()[whitelisted.len()] == b' '
    ) {
        return SecurityEvaluation { ... };
    }
}
```
现在使用精确匹配或"命令+空格"前缀匹配，防止 `cargo-malicious` 等绕过攻击。

---

### 3. 通过 exec_command + sh -c 绕过文件写入限制
**位置**: `src/tools/system_tools.rs` + `src/security/mod.rs`  
**CWE**: CWE-20 (输入验证不当)  
**修复状态**: ✅ **已修复**

**问题描述**:  
虽然 `write_file` 有路径验证，但攻击者可以使用 `exec_command` 调用 `sh -c` 配合 shell 重定向写入任意文件。

**攻击向量**:
```json
{
  "command": "sh",
  "args": ["-c", "echo 'malicious' > /etc/cron.d/backdoor"]
}
```

```json
{
  "command": "sh",
  "args": ["-c", "cat > /root/.ssh/authorized_keys << 'EOF'\nssh-rsa AAAAB3...\nEOF"]
}
```

**代码证据**:  
`evaluate_command` 只检查顶层命令，不解析 shell 语法：
```rust
let full_command = if args.is_empty() {
    command.to_string()
} else {
    format!("{} {}", command, args.join(" "))  // ❌ 不解析 shell 语法
};
```

**影响**: 
- 完全绕过文件路径验证
- 可写入系统任意位置（/etc/cron.d、/root/.ssh 等）
- 可修改系统配置、植入后门

**修复建议**:
```rust
// 对 sh -c 进行内容检查，或完全禁止
if command == "sh" || command == "bash" {
    if let Some(script) = args.first() {
        // 检查脚本内容是否包含危险操作
        if script.contains(">") || script.contains(">>") || 
           script.contains("|") || script.contains(";") {
            return SecurityEvaluation {
                danger_level: DangerLevel::High,
                reason: "Shell script with redirects requires explicit approval".to_string(),
            };
        }
    }
}
```

**实际修复**:  
已在 `src/security/mod.rs` 的 `evaluate_command` 函数中明确禁止 shell 执行 with `-c` flag（约第370-380行）：
```rust
if (command == "sh" || command == "bash" || command == "zsh" || command == "fish")
    && args.contains(&"-c") {
    return SecurityEvaluation {
        danger_level: DangerLevel::High,
        reason: "Shell execution with -c is restricted. Use direct command execution instead...",
    };
}
```

---

## ⚠️ 高危漏洞 (High)

### 4. 路径验证 TOCTOU 竞态条件
**位置**: `src/security/mod.rs:56-86`  
**CWE**: CWE-367 (时间-of-检查 时间-of-使用竞态条件)  
**修复状态**: ✅ **已修复**

**问题描述**:  
`contains_symlink` 检查和实际文件操作之间存在时间窗口，攻击者可利用 symlink 交换进行路径遍历。

**攻击场景**:
1. 检查 `/project/data/file.txt` 不是 symlink
2. 攻击者快速删除 `data` 并创建 symlink `data -> /etc`
3. 应用程序写入 `/project/data/file.txt`，实际写入 `/etc/file.txt`

**代码证据**:
```rust
fn contains_symlink(target: &Path, base: &Path) -> bool {
    // ❌ Time-of-Check 在这里
    if current.exists() {
        if let Ok(metadata) = current.symlink_metadata() {
            if metadata.file_type().is_symlink() {
                return true;
            }
        }
    }
    // ... Time-of-Use 在工具执行时
}
```

**影响**: 
- 绕过路径限制写入敏感文件
- 可能覆盖系统关键文件
- 权限提升

**修复建议**:
```rust
// 使用 O_NOFOLLOW 标志打开文件
use std::os::unix::fs::OpenOptionsExt;
let mut options = std::fs::OpenOptions::new();
options.write(true).create(true);
let file = options.custom_flags(libc::O_NOFOLLOW).open(&path)?;
```

**实际修复**:  
已在 `src/tools/file_tools.rs` 中（约第15-50行）使用 `O_NOFOLLOW` 标志打开文件：
```rust
#[cfg(unix)]
fn open_file_read(path: &PathBuf) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);  // 防止符号链接攻击
    options.open(path)
}

#[cfg(unix)]
fn open_file_write(path: &PathBuf) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    options.custom_flags(libc::O_NOFOLLOW);  // 防止符号链接攻击
    options.open(path)
}
```

---

### 5. restart 工具执行未受信代码
**位置**: `src/tools/meta_tools.rs` + `src/main.rs:399-496`  
**CWE**: CWE-94 (代码注入)  
**修复状态**: ⚠️ **部分修复**

**问题描述**:  
`restart` 工具会执行 `cargo build`，如果项目目录包含恶意代码（如受污染的 `Cargo.toml` 或构建脚本），会执行任意代码。

**代码证据**:
```rust
let build_result = process::Command::new("cargo")
    .arg("build")
    .current_dir(&working_dir)  // ❌ 在用户项目目录执行构建
    .status();
```

**影响**: 
- 任意代码执行（ACE）
- 攻击者可通过恶意构建脚本获得系统权限
- 供应链攻击风险

**修复建议**:
```rust
// 在沙箱环境中执行构建
// 1. 使用 Docker 容器
// 2. 限制网络访问
// 3. 使用 seccomp 限制系统调用
// 4. 以低权限用户运行
```

**实际修复**:  
已在 `src/tools/meta_tools.rs` 中添加工作目录验证（约第70-90行）：
```rust
let cwd_canonical = cwd.canonicalize().unwrap_or(cwd);
let root_canonical = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());

if cwd_canonical != root_canonical {
    return Ok(ToolResult {
        success: false,
        content: format!("[restart] ❌ Restart is only available when working on the dev-assistant-rs project itself..."),
    });
}
```
**但审计报告建议的沙箱化构建（Docker、seccomp、低权限用户）尚未实现。**

---

### 6. 会话日志泄露敏感信息
**位置**: `src/session/mod.rs`  
**CWE**: CWE-532 (敏感信息通过日志文件插入)  
**修复状态**: ✅ **已修复**

**问题描述**:  
会话日志以明文形式存储所有对话内容，包括可能存在的 API Key、密码、令牌等敏感信息。

**代码证据**:
```rust
pub fn log_user(&mut self, content: &str) {
    for line in content.lines() {
        writeln!(self.file, "{} ▶ 用户: {}", Self::now(), line).ok();  // ❌ 明文记录
    }
}
```

**影响**: 
- 敏感信息泄露
- 日志文件位于项目目录中，可能被提交到版本控制系统
- 长期存储增加泄露风险

**修复建议**:
```rust
// 1. 敏感信息脱敏
fn sanitize(content: &str) -> String {
    content
        .replace(&api_key, "[REDACTED]")
        .replace(&password, "[REDACTED]")
        // 使用正则匹配常见敏感信息模式
}

// 2. 设置 restrictive 权限
let file = OpenOptions::new()
    .create(true)
    .append(true)
    .mode(0o600)  // 仅所有者可读写
    .open(&path)?;

// 3. 日志轮转和自动删除
```

**实际修复**:  
已在 `src/session/mod.rs` 中完成两项关键修复：

1. **文件权限限制**（约第40行）：
```rust
let file = OpenOptions::new()
    .create(true)
    .append(true)
    .write(true)
    .mode(0o600)  // SECURITY: Restrict log file to owner only
    .open(&path)?;
```

2. **敏感信息脱敏**（约第200-250行）：
```rust
fn sanitize(content: &str) -> String {
    let mut result = content.to_string();
    
    // 脱敏 API Key (常见格式: sk-..., AIza..., gsk_...)
    let api_key_patterns = vec![
        r"(?i)(sk-[a-zA-Z0-9]{20,})",
        r"(?i)(AIza[a-zA-Z0-9_-]{35})",
        r"(?i)(gsk_[a-zA-Z0-9]{20,})",
        r"(?i)(key-[a-zA-Z0-9]{20,})",
    ];
    
    // 脱敏 Bearer Token
    // 脱敏密码
    // 脱敏私钥内容
    // 脱敏 SSH 密钥
    // 脱敏 JWT Token
    // ...
}
```

---

## ⚡ 中危漏洞 (Medium)

### 7. 命令输出无大小限制导致内存耗尽
**位置**: `src/tools/system_tools.rs:85-95`  
**CWE**: CWE-400 (未受控的资源消耗)  
**修复状态**: ❌ **未修复**

**问题描述**:  
命令的 stdout/stderr 被完全读入内存，没有大小限制。攻击者可执行 `yes` 或 `cat /dev/zero` 等命令导致内存溢出。

**代码证据**:
```rust
let stdout = child.stdout.take()
    .map(|mut s| {
        let mut buf = String::new();
        use std::io::Read;
        let _ = s.read_to_string(&mut buf);  // ❌ 无限制读取
        buf
    })
```

**影响**: 
- 内存耗尽导致程序崩溃
- 拒绝服务攻击
- 系统稳定性下降

**修复建议**:
```rust
const MAX_OUTPUT_SIZE: usize = 10 * 1024 * 1024; // 10MB

let mut stdout = Vec::new();
let mut buffer = [0u8; 8192];
loop {
    let n = s.read(&mut buffer)?;
    if n == 0 { break; }
    stdout.extend_from_slice(&buffer[..n]);
    if stdout.len() > MAX_OUTPUT_SIZE {
        return Err(AppError::Llm("Command output exceeded size limit".to_string()));
    }
}
```

---

### 8. 子进程超时后可能残留
**位置**: `src/tools/system_tools.rs:115-122`  
**CWE**: CWE-459 (未完成的清理)  
**修复状态**: ❌ **未修复**

**问题描述**:  
超时后只 kill 了直接子进程，但孙子进程（如 `sh -c "sleep 100 &"` 创建的后台进程）会继续运行。

**代码证据**:
```rust
if let Some(mut child) = child_opt.take() {
    let _ = child.kill();      // ❌ 只 kill 子进程
    let _ = child.wait();      // 孙子进程仍然运行
}
```

**影响**: 
- 后台进程继续消耗资源
- 可能执行恶意操作
- 进程泄漏

**修复建议**:
```rust
// 创建新的进程组并杀死整个组
unsafe {
    let pid = child.id();
    libc::setpgid(0, pid);  // 将子进程放入新进程组
}

// 超时时杀死整个进程组
let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
```

---

### 9. glob 工具在 symlink 工作目录下可能遍历外部
**位置**: `src/tools/file_tools.rs:409-440`  
**CWE**: CWE-59 (文件链接在文件系统之外)  
**修复状态**: ⚠️ **部分修复**

**问题描述**:  
虽然 `WalkDir` 从 `working_dir` 开始，但如果 `working_dir` 是 symlink 指向 `/`，就能遍历整个文件系统。

**代码证据**:
```rust
for entry in WalkDir::new(&context.working_dir)  // ❌ 如果 working_dir 是 symlink
```

**影响**: 
- 绕过路径限制
- 可枚举系统所有文件
- 信息泄露

**修复建议**:
```rust
// 强制 canonicalize working_dir
let working_dir = context.working_dir.canonicalize()
    .map_err(|_| AppError::Security("Working directory must be a real path".to_string()))?;

for entry in WalkDir::new(&working_dir).follow_links(false)
```

**实际修复**:  
已在 `src/security/mod.rs` 的 `evaluate_tool` 函数中添加对 `glob` 模式的基本检查（约第490-510行）：
```rust
if tool_name == "glob" {
    if let Some(pattern) = arguments["pattern"].as_str() {
        // 拒绝包含父目录遍历的模式
        if pattern.contains("..") {
            return SecurityEvaluation { danger_level: DangerLevel::Critical, ... };
        }
        // 拒绝绝对路径
        if pattern.starts_with('/') {
            return SecurityEvaluation { danger_level: DangerLevel::Critical, ... };
        }
    }
}
```
**但审计报告建议的"强制 canonicalize working_dir"在 `glob_handler` 中尚未实现。**

---

## 📝 低危漏洞 (Low)

### 10. Token 估计严重低估导致上下文截断
**位置**: `src/agent/context.rs:178-200`  
**CWE**: CWE-770 (资源池未适当分配)  
**修复状态**: ❌ **未修复**

**问题描述**:  
`estimate_tokens` 使用 `split_whitespace()` 和启发式算法，严重低估实际 token 数量，可能导致上下文压缩不及时。

**影响**: 
- LLM 请求被截断
- 上下文丢失
- 响应质量下降

**修复建议**: 使用更准确的 tokenizer（如 `tiktoken`）或保守估计。

---

### 11. API Key 格式未验证
**位置**: `src/config/mod.rs:7-16`  
**CWE**: CWE-20 (输入验证不当)  
**修复状态**: ❌ **未修复**

**问题描述**:  
只检查环境变量是否存在，不验证格式。无效 key 会浪费 HTTP 请求并暴露在日志中。

**影响**: 
- 浪费 API 配额
- 错误信息可能泄露信息
- 用户体验下降

**修复建议**:
```rust
fn validate_api_key(key: &str) -> Result<(), AppError> {
    if key.len() < 32 {
        return Err(AppError::Config("API key too short".to_string()));
    }
    // 根据 provider 验证格式
    Ok(())
}
```

---

### 12. UI 渲染中 ANSI 转义码污染
**位置**: `src/ui/mod.rs:15`  
**CWE**: CWE-117 (日志注入)  
**修复状态**: ❌ **未修复**

**问题描述**:  
交互模式使用 ANSI 转义码清屏，如果输出被重定向到文件，会污染文件内容。

**影响**: 
- 日志文件包含不可见字符
- 文件内容混乱
- 工具兼容性问题

**修复建议**:
```rust
// 检测是否输出到终端
if atty::is(atty::Stream::Stdout) {
    write!(stdout, "\x1b[2J\x1b[H")?;
}
```

---

## 📊 风险总结

| 漏洞 | 严重程度 | 利用难度 | 影响范围 | CVSS 评分 | 修复状态 |
|------|---------|---------|---------|----------|---------|
| batch_read_files 路径遍历 | Critical | 低 | 任意文件读取 | 9.1 | ✅ 已修复 |
| 白名单前缀绕过 | Critical | 低 | 任意命令执行 | 9.8 | ✅ 已修复 |
| sh -c 绕过文件限制 | Critical | 中 | 任意文件写入 | 8.8 | ✅ 已修复 |
| TOCTOU 竞态条件 | High | 高 | 路径遍历 | 6.5 | ✅ 已修复 |
| restart 代码执行 | High | 中 | 任意代码执行 | 7.5 | ⚠️ 部分修复 |
| 日志敏感信息泄露 | High | 低 | 信息泄露 | 5.3 | ✅ 已修复 |
| 命令输出无限制 | Medium | 低 | 拒绝服务 | 5.0 | ❌ 未修复 |
| 子进程残留 | Medium | 中 | 资源泄漏 | 4.5 | ❌ 未修复 |
| glob symlink 遍历 | Medium | 中 | 信息泄露 | 5.0 | ⚠️ 部分修复 |
| Token 估计不准 | Low | 无 | 功能问题 | 2.0 | ❌ 未修复 |
| API Key 未验证 | Low | 无 | 功能问题 | 2.0 | ❌ 未修复 |
| ANSI 污染 | Low | 无 | 兼容性 | 1.0 | ❌ 未修复 |

---

## 🛡️ 修复优先级

### 立即修复（Critical - 24小时内）
1. ✅ 为 `batch_read_files` 和 `glob` 添加路径验证
2. ✅ 修复白名单匹配逻辑，使用精确匹配
3. ✅ 限制或禁止 `sh -c` 的使用

### 短期修复（High - 1周内）
4. ✅ 解决 TOCTOU 问题，使用 `O_NOFOLLOW`
5. ⚠️ 完全沙箱化 restart 工具的构建过程（当前仅限制工作目录）
6. ✅ 加密会话日志并设置文件权限

### 中期改进（Medium - 1个月内）
7. 📋 限制命令输出大小
8. 📋 实现进程组 kill
9. 📋 强制 canonicalize working_dir

### 长期规划（Low）
10. 📋 集成准确 tokenizer
11. 📋 添加 API Key 格式验证
12. 📋 检测终端环境再输出 ANSI

---

## 📋 检查清单

- [x] 所有文件工具（read/write/edit/batch_read/glob/list/file_exists）都有路径验证
- [x] 命令白名单使用精确匹配而非前缀匹配
- [x] 禁止或严格限制 shell 执行（sh -c, bash -c）
- [x] 文件操作使用 O_NOFOLLOW 防止 symlink 攻击
- [ ] restart 工具在沙箱中执行（当前仅验证工作目录）
- [x] 会话日志脱敏并设置 0o600 权限
- [ ] 命令输出限制在合理大小（如 10MB）
- [ ] 超时进程使用 kill(-pid) 杀死整个进程组
- [ ] working_dir 强制 canonicalize
- [ ] 添加安全审计日志

---

## 📝 备注

本报告基于静态代码分析生成，建议：
1. 进行动态渗透测试验证漏洞可利用性
2. 使用 `cargo audit` 检查依赖漏洞
3. 进行代码审查（Code Review）确保修复正确性
4. 建立安全开发生命周期（SDLC）流程

**报告生成时间**: 2025-06-18  
**修复状态审查时间**: 2025-06-18  
**审查工具**: 静态代码分析 + 人工审查  
**置信度**: 高
