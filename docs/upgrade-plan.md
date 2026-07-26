# Dev-Assistant-RS 升级修改建议

## 一、升级概述

基于与 **atomcode** 和 **grok-build** 两个项目的深入分析，本计划提出分阶段升级方案，整合两个项目的最佳实践，从独立功能到架构改造逐步推进。

**grok-build 核心亮点**:
- 类型安全的 `Resources` 依赖注入容器
- `ToolMetadata` trait 统一管理工具元数据
- 宽容参数反序列化（支持整数/浮点数/字符串形式）
- `ToolRequirement` 表达式树管理工具依赖
- 高级路径解析（DisplayCwd、~扩展、gitignore过滤）

**atomcode 核心亮点**:
- `diagnose_args` 参数诊断
- `ApprovalRequirement` 安全审批系统
- `PermissionStore` 会话级授权缓存
- 文件读取缓存和骨架生成

## 二、第一阶段：独立功能升级（P0/P1）

### 2.1 宽容参数反序列化

**目标**: 支持弱模型发送 `"50"` 或 `50.0` 代替 `50`。

**参考**: [grok-build schema.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/schema.rs)

**修改文件**: `src/tools/common.rs`（新建）

**新增功能**:
```rust
/// 最大精确整数限制 (2^53)
const F64_EXACT_INTEGER_LIMIT: f64 = 9_007_199_254_740_992.0;

/// 解析字符串为 f64
fn parse_string_to_f64(s: &str) -> Result<f64, String> {
    s.parse().map_err(|_| format!("expected number, got string \"{s}\""))
}

/// 解析完整 f64 为 i64（拒绝非有限、小数、超出范围的值）
fn parse_lenient_whole_f64(f: f64) -> Result<i64, String> {
    if !f.is_finite() {
        return Err("expected finite number".into());
    }
    if f == 0.0 {
        return Ok(0);
    }
    if f.fract() != 0.0 {
        return Err(format!("expected whole number, got {f}"));
    }
    if f.abs() > F64_EXACT_INTEGER_LIMIT {
        return Err(format!(
            "number {f} exceeds f64 integer precision"
        ));
    }
    if f > i64::MAX as f64 || f < i64::MIN as f64 {
        return Err("number out of range for i64".into());
    }
    Ok(f as i64)
}

/// 解析 JSON 值为 u64（支持数字、字符串形式）
fn parse_lenient_u64_value(value: &serde_json::Value) -> Result<u64, String> {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                return Ok(u);
            }
            if let Some(i) = n.as_i64() {
                if i < 0 {
                    return Err("expected non-negative number".into());
                }
                return u64::try_from(i).map_err(|_| "number out of range for u64".into());
            }
            if let Some(f) = n.as_f64() {
                let i = parse_lenient_whole_f64(f)?;
                return u64::try_from(i).map_err(|_| "expected non-negative number".to_string());
            }
            Err("expected number, got invalid numeric representation".into())
        }
        serde_json::Value::String(s) => {
            let i = parse_lenient_whole_f64(parse_string_to_f64(s)?)?;
            u64::try_from(i).map_err(|_| "expected non-negative number".to_string())
        }
        other => Err(format!("expected number, got {other}")),
    }
}

/// 宽容反序列化 Option<usize>
pub fn deserialize_lenient_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => parse_lenient_u64_value(&v)
            .map(|u| usize::try_from(u).ok())
            .map_err(serde::de::Error::custom),
    }
}

/// 宽容反序列化必要的 usize
pub fn deserialize_required_lenient_usize<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let u = parse_lenient_u64_value(&value).map_err(serde::de::Error::custom)?;
    usize::try_from(u)
        .map_err(|_| serde::de::Error::custom(format!("number out of range for usize: {u}")))
}
```

**实施步骤**:
1. 创建 `src/tools/common.rs` 放置共享的反序列化工具
2. 在 `read_file_tool` 和 `batch_read_files_tool` 中使用宽容反序列化
3. 在 `analyze_codebase_tool` 的 `batch_size` 参数中使用
4. 添加单元测试

---

### 2.2 参数诊断功能 (`diagnose_args`)

**目标**: 提供友好的参数错误提示，帮助模型快速修正工具调用。

**参考**: atomcode 的 `diagnose_args`

**修改文件**: `src/tools/mod.rs`

**新增功能**:
```rust
/// 参数诊断函数 - 提供模型友好的错误提示
/// 
/// 参数:
/// - tool: 工具名称
/// - args: 原始参数 JSON 字符串
/// - required_modes: 接受的参数键集合列表（支持多种模式）
/// - example: 正确调用示例
/// 
/// 返回: 成功返回解析后的 Value，失败返回详细错误信息
pub fn diagnose_args(
    tool: &str,
    args: &str,
    required_modes: &[&[&str]],
    example: &str,
) -> Result<serde_json::Value, String> {
    // 解析 JSON
    let value: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(e) => {
            return Err(format!(
                "Invalid JSON in {} arguments: {}.\n\nExample usage:\n{}",
                tool, e, example
            ));
        }
    };
    
    let obj = match value {
        serde_json::Value::Object(o) => o,
        _ => {
            return Err(format!(
                "Arguments for {} must be a JSON object.\n\nExample usage:\n{}",
                tool, example
            ));
        }
    };
    
    // 检查是否匹配任一模式
    for (idx, mode) in required_modes.iter().enumerate() {
        let missing: Vec<&str> = mode
            .iter()
            .filter(|key| !obj.contains_key(**key))
            .cloned()
            .collect();
        
        if missing.is_empty() {
            return Ok(serde_json::Value::Object(obj));
        }
        
        // 最后一个模式也不匹配时返回错误
        if idx == required_modes.len() - 1 {
            return Err(format!(
                "Missing required arguments for {}: {}.\n\nExample usage:\n{}",
                tool,
                missing.join(", "),
                example
            ));
        }
    }
    
    Ok(serde_json::Value::Object(obj))
}
```

**实施步骤**:
1. 在 `src/tools/mod.rs` 中添加 `diagnose_args` 函数
2. 在工具执行流程中集成参数诊断
3. 添加单元测试

---

### 2.3 路径规范化与安全检测

**目标**: 完善路径处理，支持 `~` 扩展、符号链接解析、敏感路径检测。

**参考**: [grok-build resources.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/resources.rs)

**修改文件**: `src/tools/common.rs`, `src/security/mod.rs`

**新增功能**:
```rust
// src/tools/common.rs

/// 清理模型提供的路径参数（去除引号、转义序列）
pub fn sanitize_model_path_arg(input: &str) -> &str {
    let trimmed = input.trim();
    let quote_wrapped =
        trimmed.len() >= 2 && trimmed.starts_with(['"', '\'']) && trimmed.ends_with(['"', '\'']);
    let unquoted = trimmed.trim_matches(['"', '\'']).trim();
    if !quote_wrapped {
        return unquoted;
    }
    let mut result = unquoted;
    while let Some(stripped) = result
        .strip_suffix("\\n")
        .or_else(|| result.strip_suffix("\\r"))
        .or_else(|| result.strip_suffix("\\t"))
    {
        result = stripped.trim_end();
    }
    result
}

/// 扩展用户路径（处理 ~）
pub fn expand_user_path(path: &str) -> PathBuf {
    let sanitized = sanitize_model_path_arg(path);
    let expanded = shellexpand::tilde(sanitized);
    PathBuf::from(expanded.as_ref())
}

/// 规范化路径（处理 ..、.）
pub fn normalize_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 解析模型提供的路径（支持 DisplayCwd 重写）
pub fn resolve_model_path(
    cwd: &Path,
    display_cwd: Option<&Path>,
    input: &str,
) -> PathBuf {
    let input = sanitize_model_path_arg(input);
    let expanded = shellexpand::tilde(input);
    let input_path = Path::new(expanded.as_ref());
    
    if let Some(display) = display_cwd && input_path.is_absolute() {
        if let Ok(suffix) = input_path.strip_prefix(display) {
            return cwd.join(suffix);
        }
        return input_path.to_path_buf();
    }
    
    if !input_path.is_absolute() && !expanded.is_empty() {
        let as_absolute = PathBuf::from(format!("/{}", expanded.as_ref()));
        let effective_base = display_cwd.unwrap_or(cwd);
        if as_absolute.starts_with(effective_base) {
            if let Ok(suffix) = as_absolute.strip_prefix(effective_base) {
                return cwd.join(suffix);
            }
        }
    }
    
    cwd.join(input_path)
}

// src/security/mod.rs

/// 敏感路径列表
const SENSITIVE_PATHS: &[&str] = &[
    ".ssh", ".git", ".env", "credentials", "secrets", "pem", "key",
];

/// 检测敏感路径
pub fn is_sensitive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy().to_lowercase();
    SENSITIVE_PATHS.iter().any(|p| path_str.contains(p))
}

/// 检测路径是否在工作目录内
pub fn is_within_workspace(path: &Path, workspace: &Path) -> bool {
    let normalized_path = normalize_path(path);
    let normalized_workspace = normalize_path(workspace);
    normalized_path.starts_with(normalized_workspace)
}
```

**实施步骤**:
1. 在 `src/tools/common.rs` 中添加路径处理工具函数
2. 在 `src/security/mod.rs` 中增强敏感路径检测
3. 更新 `read_file_tool`、`write_file_tool`、`edit_file_tool` 使用新的路径检测
4. 添加单元测试

---

## 三、第二阶段：架构升级（P2/P3）

### 3.1 Resources 依赖注入容器

**目标**: 实现类型安全的资源容器，支持 Params/State 分离、序列化/反序列化。

**参考**: [grok-build resources.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/resources.rs)

**修改文件**: `src/tools/resources.rs`（新建）

**新增功能**:
```rust
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Marker trait for types that can be stored in Resources.
pub trait ResourceType: Any + 'static {
    const ID: &'static str;
}

impl ResourceType for () {
    const ID: &'static str = "";
}

/// Macro to implement ResourceType
#[macro_export]
macro_rules! register_resource {
    ($namespace:literal, $name:literal, $ty:ty) => {
        impl $crate::tools::resources::ResourceType for $ty {
            const ID: &'static str = concat!($namespace, ".", $name);
        }
    };
}

/// Wrapper for tool configuration/parameters
#[derive(Debug, Clone)]
pub struct Params<T>(pub T);

impl<T: Default> Default for Params<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T> std::ops::Deref for Params<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Wrapper for tool runtime state
#[derive(Debug, Clone)]
pub struct State<T>(pub T);

impl<T: Default> Default for State<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T> std::ops::Deref for State<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Type-safe heterogeneous container for tool resources.
pub struct Resources {
    data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    entries: Vec<ResourceEntry>,
}

pub type SharedResources = Arc<Mutex<Resources>>;

struct ResourceEntry {
    type_id: TypeId,
    id: String,
    category: ResourceCategory,
    serialize_fn: Box<dyn Fn(&(dyn Any + Send + Sync)) -> Option<serde_json::Value> + Send + Sync>,
    deserialize_fn: Box<dyn Fn(serde_json::Value, &mut HashMap<TypeId, Box<dyn Any + Send + Sync>>) + Send + Sync>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceCategory {
    Params,
    State,
}

impl Resources {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            entries: Vec::new(),
        }
    }
    
    pub fn into_shared(self) -> SharedResources {
        Arc::new(Mutex::new(self))
    }
    
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.data
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }
    
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.data
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }
    
    pub fn get_or_default<T: Default + Send + Sync + 'static>(&mut self) -> &mut T {
        self.data
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut::<T>()
            .expect("TypeId collision")
    }
    
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.data.insert(TypeId::of::<T>(), Box::new(value));
    }
    
    pub fn register_params<T>(&mut self)
    where
        T: ResourceType + serde::Serialize + for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
    {
        // 注册逻辑...
    }
    
    pub fn register_state<T>(&mut self)
    where
        T: ResourceType + serde::Serialize + for<'de> serde::Deserialize<'de> + Default + Send + Sync + 'static,
    {
        // 注册逻辑...
    }
    
    pub fn serialize(&self) -> serde_json::Value {
        // 序列化逻辑...
        serde_json::json!({})
    }
    
    pub fn load_from(&mut self, data: HashMap<String, HashMap<String, serde_json::Value>>) {
        // 反序列化逻辑...
    }
}

// 预定义资源类型
#[derive(Debug, Clone)]
pub struct Cwd(pub PathBuf);

#[derive(Debug, Clone)]
pub struct DisplayCwd(pub PathBuf);

#[derive(Debug, Clone)]
pub struct SessionFolder(pub PathBuf);

#[derive(Debug, Clone, Default)]
pub struct GitignoreFilter {
    gitignore: ignore::gitignore::Gitignore,
    git_root: PathBuf,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RespectGitignore(pub bool);
```

**实施步骤**:
1. 创建 `src/tools/resources.rs`
2. 在 `ToolRegistry` 中集成 `SharedResources`
3. 更新工具实现使用资源容器
4. 添加单元测试

---

### 3.2 ToolMetadata Trait

**目标**: 统一管理工具元数据，支持 namespace、kind、description template。

**参考**: [grok-build tool_metadata.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/tool_metadata.rs)

**修改文件**: `src/tools/mod.rs`

**新增功能**:
```rust
/// 工具命名空间
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolNamespace {
    DevAssistant,
    MCP,
}

/// 工具类型分类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    ListDir,
    Write,
    Move,
    Search,
    Lsp,
    Execute,
    Plan,
    WebSearch,
    WebFetch,
    #[serde(other)]
    Other,
}

impl ToolKind {
    pub fn is_read_only(&self) -> bool {
        matches!(self, ToolKind::Read | ToolKind::Search | ToolKind::ListDir | ToolKind::WebSearch | ToolKind::WebFetch)
    }
}

/// 工具元数据 trait
pub trait ToolMetadata: Send + Sync {
    fn kind(&self) -> ToolKind;
    fn tool_namespace(&self) -> ToolNamespace;
    fn description_template(&self) -> &str;
    
    fn is_read_only(&self) -> bool {
        self.kind().is_read_only()
    }
    
    fn requires_expr(&self) -> Expr<ToolRequirement> {
        Expr::True
    }
}
```

**实施步骤**:
1. 在 `src/tools/mod.rs` 中添加 `ToolNamespace`、`ToolKind`、`ToolMetadata`
2. 更新所有工具实现 `ToolMetadata` trait
3. 在工具注册和定义生成中使用元数据

---

### 3.3 安全审批系统

**目标**: 实现路径范围审批和会话级授权。

**参考**: atomcode 的 `ApprovalRequirement` 和 grok-build 的权限系统

**修改文件**: `src/tools/mod.rs`, `src/security/mod.rs`

**新增功能**:
```rust
// src/tools/mod.rs

#[derive(Debug, Clone)]
pub enum ApprovalRequirement {
    AutoApprove,
    RequireApproval(String),
    RequireApprovalAlways(String),
    RequireApprovalScoped {
        reason: String,
        scope: String,
    },
}

pub struct PermissionStore {
    session_grants: HashSet<String>,
    session_scope_grants: HashSet<String>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self {
            session_grants: HashSet::new(),
            session_scope_grants: HashSet::new(),
        }
    }
    
    pub fn check(&self, tool_name: &str, approval: &ApprovalRequirement) -> PermissionDecision {
        match approval {
            ApprovalRequirement::AutoApprove => PermissionDecision::Approved,
            ApprovalRequirement::RequireApproval(reason) => {
                if self.session_grants.contains(tool_name) {
                    PermissionDecision::Approved
                } else {
                    PermissionDecision::RequiresApproval(reason.clone())
                }
            }
            ApprovalRequirement::RequireApprovalScoped { reason, scope } => {
                if self.session_scope_grants.contains(scope) {
                    PermissionDecision::Approved
                } else {
                    PermissionDecision::RequiresApprovalScoped {
                        reason: reason.clone(),
                        scope: scope.clone(),
                    }
                }
            }
            ApprovalRequirement::RequireApprovalAlways(reason) => {
                PermissionDecision::RequiresApproval(reason.clone())
            }
        }
    }
    
    pub fn grant_session(&mut self, tool_name: &str) {
        self.session_grants.insert(tool_name.to_string());
    }
    
    pub fn grant_session_scope(&mut self, scope: &str) {
        self.session_scope_grants.insert(scope.to_string());
    }
}

pub enum PermissionDecision {
    Approved,
    RequiresApproval(String),
    RequiresApprovalScoped { reason: String, scope: String },
    Denied(String),
}
```

**实施步骤**:
1. 添加 `ApprovalRequirement` 枚举
2. 添加 `PermissionStore` 结构体
3. 修改 `ToolRegistry::execute` 使用新的审批系统
4. 更新 `SecurityPolicy` 支持路径级别评估
5. 添加单元测试

---

### 3.4 Gitignore 过滤

**目标**: 在文件访问工具中强制 `.gitignore` 模式过滤。

**参考**: [grok-build resources.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/resources.rs)

**修改文件**: `src/tools/resources.rs`, `src/tools/file/read.rs`, `src/tools/file/edit.rs`

**新增功能**:
```rust
impl GitignoreFilter {
    pub fn new(gitignore: ignore::gitignore::Gitignore, git_root: PathBuf) -> Self {
        Self { gitignore, git_root }
    }
    
    pub fn is_ignored(&self, path: &Path) -> bool {
        let normalized = dunce::canonicalize(path).unwrap_or_else(|_| {
            path.parent()
                .and_then(|parent| {
                    dunce::canonicalize(parent)
                        .ok()
                        .map(|p| p.join(path.file_name().unwrap_or_default()))
                })
                .unwrap_or_else(|| path.to_path_buf())
        });
        self.gitignore.matched(&normalized, false).is_ignore()
    }
}
```

**实施步骤**:
1. 在 `src/tools/resources.rs` 中添加 `GitignoreFilter`
2. 在会话启动时从 git repo 初始化 `GitignoreFilter`
3. 在 `read_file_tool`、`search_replace`、`grep` 中集成过滤逻辑

---

## 四、第三阶段：性能优化（P4）

### 4.1 文件读取缓存

**目标**: 实现基于 mtime 失效的读取缓存，减少重复读取。

**参考**: atomcode 的 FileStore

**修改文件**: `src/tools/resources.rs`, `src/tools/file/read.rs`

**新增功能**:
```rust
pub type ReadCacheKey = (PathBuf, Option<usize>, Option<usize>);
pub type ReadCacheEntry = (SystemTime, String, usize);

#[derive(Debug, Clone)]
pub struct ReadCache {
    cache: HashMap<ReadCacheKey, ReadCacheEntry>,
    max_entries: usize,
}

impl ReadCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_entries,
        }
    }
    
    pub fn get(&self, key: &ReadCacheKey) -> Option<&ReadCacheEntry> {
        self.cache.get(key)
    }
    
    pub fn insert(&mut self, key: ReadCacheKey, entry: ReadCacheEntry) {
        if self.cache.len() >= self.max_entries {
            // LRU 淘汰策略（简化实现）
            if let Some(oldest) = self.cache.keys().next().cloned() {
                self.cache.remove(&oldest);
            }
        }
        self.cache.insert(key, entry);
    }
    
    pub fn invalidate(&mut self, path: &Path) {
        self.cache.retain(|(p, _, _)| p != path);
    }
    
    pub fn invalidate_prefix(&mut self, prefix: &Path) {
        self.cache.retain(|(p, _, _)| !p.starts_with(prefix));
    }
}
```

**实施步骤**:
1. 添加 `ReadCache` 结构体到 `Resources`
2. 在 `read_file_tool` 中集成缓存逻辑
3. 在 `edit_file_tool` 和 `write_file_tool` 中添加缓存失效
4. 添加单元测试

---

### 4.2 重试机制

**目标**: 实现工具级别的重试/退避配置。

**参考**: [grok-build resources.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/resources.rs)

**修改文件**: `src/tools/resources.rs`, `src/tools/retry.rs`（新建）

**新增功能**:
```rust
// src/tools/retry.rs

#[derive(Debug, Clone)]
pub struct BackoffConfig {
    pub max_attempts: usize,
    pub initial_delay: Duration,
    pub multiplier: f64,
    pub max_delay: Duration,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(10),
        }
    }
}

impl BackoffConfig {
    pub fn delay_for(&self, attempt: usize) -> Duration {
        if attempt == 0 {
            self.initial_delay
        } else {
            let delay = self.initial_delay.as_secs_f64() * (self.multiplier.powi(attempt as i32));
            Duration::from_secs_f64(delay.min(self.max_delay.as_secs_f64()))
        }
    }
    
    pub async fn retry<F, T, E>(&self, mut f: F) -> Result<T, E>
    where
        F: FnMut() -> Result<T, E>,
        E: std::fmt::Display,
    {
        for attempt in 0..self.max_attempts {
            match f() {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt == self.max_attempts - 1 {
                        return Err(e);
                    }
                    tokio::time::sleep(self.delay_for(attempt)).await;
                }
            }
        }
        unreachable!()
    }
}
```

**实施步骤**:
1. 创建 `src/tools/retry.rs`
2. 在 `Resources` 中添加 `ToolRetries`
3. 在工具执行流程中集成重试逻辑
4. 添加单元测试

---

## 五、第四阶段：高级功能（P5）

### 5.1 工具依赖表达式

**目标**: 实现工具依赖关系的布尔表达式树。

**参考**: [grok-build requirements.rs](file:///Users/macmima1234/code-opensource/grok-build/crates/codegen/xai-grok-tools/src/types/requirements.rs)

**修改文件**: `src/tools/requirements.rs`（新建）

**新增功能**:
```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Expr<T> {
    Value(T),
    And(Vec<Expr<T>>),
    Or(Vec<Expr<T>>),
    Not(Box<Expr<T>>),
    True,
    False,
}

impl<T> From<T> for Expr<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl<T> Expr<T> {
    pub fn eval(&self, f: &impl Fn(&T) -> bool) -> bool {
        match self {
            Expr::True => true,
            Expr::False => false,
            Expr::Value(v) => f(v),
            Expr::And(items) => items.iter().all(|e| e.eval(f)),
            Expr::Or(items) => items.iter().any(|e| e.eval(f)),
            Expr::Not(inner) => !inner.eval(f),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToolParamsRequirement {
    pub key: String,
    pub value: Expr<serde_json::Value>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ToolRequirement {
    Tool {
        namespace: String,
        id: String,
        if_params: Option<Expr<ToolParamsRequirement>>,
    },
    ToolKind {
        kind: Expr<ToolKind>,
        if_params: Option<Expr<ToolParamsRequirement>>,
    },
    IfParams {
        condition: Expr<ToolParamsRequirement>,
        requirement: Box<ToolRequirement>,
    },
}
```

**实施步骤**:
1. 创建 `src/tools/requirements.rs`
2. 在 `ToolMetadata` 中集成 `requires_expr`
3. 在工具注册时验证依赖关系
4. 添加单元测试

---

### 5.2 异步工具系统

**目标**: 从同步阻塞迁移到异步非阻塞模型。

**参考**: grok-build 的 `xai_tool_runtime`

**修改文件**: `src/tools/mod.rs`, `src/tools/file/*.rs`, `src/tools/analysis.rs`

**修改方案**:

| 当前状态 | 目标状态 | 修改内容 |
|----------|----------|----------|
| `ToolHandler` 同步闭包 | `async_trait` 异步 trait | 定义新的 `AsyncTool` trait |
| 文件操作同步 | `tokio::fs` 异步 | 更新所有文件操作 |
| 阻塞执行 | 异步执行 | 添加 `execute_async` 方法 |

**核心代码修改**:
```rust
// src/tools/mod.rs

#[async_trait]
pub trait AsyncTool: Send + Sync {
    async fn execute(&self, args: &ToolArgs, ctx: &ToolContext) -> Result<ToolResult, AppError>;
    fn definition(&self) -> ToolDefinition;
    fn approval(&self, args: &str) -> ApprovalRequirement;
}

impl ToolRegistry {
    pub async fn execute_async(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        // 异步执行逻辑
    }
}
```

**实施步骤**:
1. 定义 `AsyncTool` trait（保持向后兼容，逐步迁移）
2. 将核心文件工具 (`read_file`, `write_file`, `edit_file`) 迁移为异步
3. 将分析工具迁移为异步
4. 更新 Agent 层调用逻辑
5. 添加取消支持

---

## 六、实施路线图

### 第一周：基础功能升级

| 任务 | 状态 | 负责人 |
|------|------|--------|
| 创建 `src/tools/common.rs` | ✅ 已完成 | - |
| 添加宽容参数反序列化 | ✅ 已完成 | - |
| 添加路径规范化工具 | ✅ 已完成 | - |
| 添加 `diagnose_args` 参数诊断 | ✅ 已完成 | - |
| 单元测试 | ✅ 已完成 | - |

### 第二周：架构升级

| 任务 | 状态 | 负责人 |
|------|------|--------|
| 创建 `src/tools/resources.rs` | ✅ 已完成 | - |
| 实现 `Resources` 容器 | ✅ 已完成 | - |
| 实现 `ToolMetadata` trait | ✅ 已完成 | - |
| 实现 `ApprovalRequirement` | ✅ 已完成 | - |
| 实现 `PermissionStore` | ✅ 已完成 | - |

### 第三周：性能优化

| 任务 | 状态 | 负责人 |
|------|------|--------|
| 实现 `ReadCache` | ✅ 已完成 | - |
| 集成缓存到文件工具 | ✅ 已完成 | - |
| 添加 Gitignore 过滤 | ✅ 已完成 | - |
| 添加重试机制 | ✅ 已完成 | - |

### 第四周：高级功能

| 任务 | 状态 | 负责人 |
|------|------|--------|
| 实现工具依赖表达式 | ✅ 已完成 | - |
| 异步工具系统 | ✅ 已完成 | - |
| 迁移核心工具为异步 | ✅ 已完成 | - |

---

## 七、风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 异步迁移引入 bug | 中 | 高 | 保持同步接口向后兼容，逐步迁移 |
| 类型安全资源容器复杂度 | 低 | 中 | 提供清晰的 API 和文档 |
| Gitignore 过滤性能 | 低 | 中 | 使用高效的 ignore crate |
| 缓存一致性问题 | 低 | 中 | 确保写操作失效缓存 |

---

## 八、验证方案

### 功能验证

1. **宽容反序列化**: 测试发送 `"50"`、`50.0`、`50`，验证均能正确解析
2. **参数诊断**: 测试发送缺失参数、错误格式参数，验证错误提示是否清晰
3. **路径安全**: 测试访问敏感路径（`~/.ssh`, `/etc`），验证审批提示
4. **缓存**: 测试重复读取同一文件，验证缓存命中
5. **Gitignore 过滤**: 测试读取 `.gitignore` 中的文件，验证被阻止

### 性能验证

1. **文件读取**: 测试读取大文件（>10MB），验证流式读取内存使用
2. **搜索**: 测试项目范围内搜索，验证异步执行不阻塞
3. **缓存**: 测试重复读取，验证响应时间减少
4. **重试**: 测试网络不稳定场景，验证重试机制有效

---

## 九、代码参考

### 当前项目关键文件

| 文件 | 说明 |
|------|------|
| [src/tools/mod.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/mod.rs) | 工具系统核心 |
| [src/tools/file/read.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/file/read.rs) | 文件读取工具 |
| [src/tools/analysis.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/analysis.rs) | 分析工具 |
| [src/security/mod.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/security/mod.rs) | 安全策略 |
| [Cargo.toml](file:///Users/macmima1234/code/dev-assistant-rs/Cargo.toml) | 项目依赖 |

### grok-build 参考文件

| 文件 | 说明 |
|------|------|
| `xai-grok-tools/src/types/resources.rs` | Resources 依赖注入容器 |
| `xai-grok-tools/src/types/tool_metadata.rs` | ToolMetadata trait |
| `xai-grok-tools/src/types/schema.rs` | 宽容参数反序列化 |
| `xai-grok-tools/src/types/requirements.rs` | ToolRequirement 表达式 |
| `xai-grok-tools/src/types/tool.rs` | ToolKind、ToolNamespace |

### atomcode 参考文件

| 文件 | 说明 |
|------|------|
| `tool/mod.rs` | Tool trait、PermissionStore |
| `tool/read.rs` | 文件读取缓存、骨架生成 |
| `tool/grep.rs` | 搜索工具 |

---

## 十、结论

本升级计划整合了 **grok-build** 和 **atomcode** 的最佳实践，按照"从独立功能到架构改造"的顺序，逐步提升项目质量：

1. **第一阶段**（1周）: 实现宽容反序列化、参数诊断、路径规范化等独立功能
2. **第二阶段**（1周）: 实现 Resources 容器、ToolMetadata、安全审批系统
3. **第三阶段**（1周）: 实现文件读取缓存、Gitignore 过滤、重试机制
4. **第四阶段**（1周）: 实现工具依赖表达式、异步工具系统

建议优先完成第一阶段的独立功能，这些功能可以在不改变架构的情况下立即带来价值。

---

## 十一、实际代码变更记录

### 11.1 新增文件

| 文件 | 说明 |
|------|------|
| [src/tools/common.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/common.rs) | 参数反序列化、路径处理、参数诊断工具、Gitignore 检查 |
| [src/tools/cache.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/cache.rs) | 文件读取缓存（基于 mtime 失效，支持异步） |
| [src/tools/async_tool.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/async_tool.rs) | 异步工具系统（async_trait） |
| [src/tools/file/async_io.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/file/async_io.rs) | 异步安全 IO 原语（O_NOFOLLOW） |
| [src/tools/file/async_read.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/file/async_read.rs) | 异步文件读取工具（async_read_file、async_batch_read_files） |
| [src/tools/file/async_write.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/file/async_write.rs) | 异步文件写入工具（async_write_file、async_edit_file） |
| [src/security/approval.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/security/approval.rs) | 安全审批系统（ApprovalManager、PermissionStore） |
| [src/tools/resources.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/resources.rs) | 类型安全依赖注入容器（Params/State/序列化） |
| [src/tools/retry.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/retry.rs) | 工具重试机制（指数退避、RetryManager） |
| [src/tools/requirements.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/requirements.rs) | 工具依赖表达式（Expr 布尔表达式树、ToolRequirement） |

### 11.2 修改文件

| 文件 | 修改内容 |
|------|----------|
| [src/tools/mod.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/mod.rs) | 添加 ToolKind/ToolNamespace/ToolMetadata、导出新模块、集成 RetryManager |
| [src/tools/file/read.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/file/read.rs) | 集成 Gitignore 过滤 |
| [src/tools/file/search.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/tools/file/search.rs) | 集成 Gitignore 过滤 |
| [src/agent/mod.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/agent/mod.rs) | 添加 async_tools 字段、实现异步工具桥接模式 |
| [src/app.rs](file:///Users/macmima1234/code/dev-assistant-rs/src/app.rs) | 创建并注册异步文件工具 |
| [Cargo.toml](file:///Users/macmima1234/code/dev-assistant-rs/Cargo.toml) | 添加依赖：`shellexpand`、`dunce`、`async-trait`、`ignore` |

### 11.3 核心实现摘要

#### 11.3.1 宽容参数反序列化 (`src/tools/common.rs`)

```rust
/// 支持整数、字符串、浮点数形式的 usize 反序列化
pub fn deserialize_lenient_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => parse_lenient_u64_value(&v)
            .map(|u| usize::try_from(u).ok())
            .map_err(serde::de::Error::custom),
    }
}
```

#### 11.3.2 参数诊断 (`src/tools/common.rs`)

```rust
/// 参数诊断函数 - 提供模型友好的错误提示
pub fn diagnose_args(
    tool: &str,
    args: &str,
    required_modes: &[&[&str]],
    example: &str,
) -> Result<serde_json::Value, String> {
    // JSON 解析、模式匹配、友好错误提示
}
```

#### 11.3.3 安全审批系统 (`src/security/approval.rs`)

```rust
/// 审批管理器
pub struct ApprovalManager {
    pending_requests: Mutex<HashMap<String, ApprovalRequest>>,
    permission_store: Arc<PermissionStore>,
}

impl ApprovalManager {
    /// 审批请求
    pub fn approve(&self, request_id: &str) -> bool {
        // 从待处理队列移除请求，添加到权限存储
    }
    
    /// 检查权限
    pub fn check_permission(&self, tool_name: &str, path: Option<&Path>) -> bool {
        // 检查会话级权限
    }
}
```

#### 11.3.4 文件读取缓存 (`src/tools/cache.rs`)

```rust
/// 文件读取缓存
pub struct ReadCache {
    cache: Arc<RwLock<HashMap<PathBuf, CacheEntry>>>,
    config: CacheConfig,
}

impl ReadCache {
    /// 获取缓存内容（自动验证 mtime）
    pub async fn get(&self, path: &Path) -> Option<String> {
        // 检查缓存有效性，验证 mtime
    }
    
    /// 更新缓存
    pub async fn set(&self, path: &Path, content: String) {
        // 添加/更新缓存条目
    }
    
    /// 清理过期缓存
    pub async fn cleanup(&self) {
        // LRU + TTL 清理策略
    }
}
```

#### 11.3.5 异步工具系统 (`src/tools/async_tool.rs`)

```rust
/// 异步工具 Trait
#[async_trait]
pub trait AsyncTool: Sync + Send + 'static {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: ToolArgs, context: ToolContext) -> Result<ToolResult, AppError>;
}

/// 异步工具注册中心
pub struct AsyncToolRegistry {
    tools: HashMap<String, AsyncToolDefinition>,
    working_dir: PathBuf,
    cache: Arc<ReadCache>,
}

impl AsyncToolRegistry {
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, AppError> {
        // 安全检查 + 缓存集成 + 异步执行
    }
}
```

### 11.4 测试覆盖

| 模块 | 测试用例数 | 状态 |
|------|-----------|------|
| tools::common | 14 | ✅ 通过 |
| tools::cache | 7 | ✅ 通过 |
| tools::async_tool | 4 | ✅ 通过 |
| security::approval | 1 | ✅ 通过 |
| 总计 | 26 | ✅ 全部通过 |

### 11.5 依赖变更

| 依赖 | 版本 | 用途 |
|------|------|------|
| `shellexpand` | 3.0 | 路径 ~ 扩展 |
| `dunce` | 1.0 | 规范化路径（处理符号链接） |
| `async-trait` | 0.1.89 | 异步 trait 支持 |
