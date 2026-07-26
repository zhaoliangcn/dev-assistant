//! 工具资源容器
//! 
//! 简化的资源管理，只保留当前实际使用的资源类型。

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// 类型安全的资源容器
pub struct Resources {
    data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

pub type SharedResources = Arc<RwLock<Resources>>;

impl Resources {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }
    
    pub fn into_shared(self) -> SharedResources {
        Arc::new(RwLock::new(self))
    }
    
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.data
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }
    
    pub fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.data.insert(TypeId::of::<T>(), Box::new(value));
    }
    
    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.data.contains_key(&TypeId::of::<T>())
    }
    
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> bool {
        self.data.remove(&TypeId::of::<T>()).is_some()
    }
}

impl Default for Resources {
    fn default() -> Self {
        Self::new()
    }
}

// 预定义资源类型

/// 当前工作目录
#[derive(Debug, Clone)]
pub struct Cwd(pub PathBuf);

/// 显示用的工作目录（用于路径重写）
#[derive(Debug, Clone)]
pub struct DisplayCwd(pub PathBuf);

/// 是否尊重 .gitignore
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RespectGitignore(pub bool);

/// Gitignore 过滤器
#[derive(Debug)]
pub struct GitignoreFilter {
    gitignore: ignore::gitignore::Gitignore,
    git_root: PathBuf,
}

impl GitignoreFilter {
    pub fn new(gitignore: ignore::gitignore::Gitignore, git_root: PathBuf) -> Self {
        Self { gitignore, git_root }
    }
    
    /// 从工作目录创建 GitignoreFilter
    pub fn from_path(working_dir: &Path) -> Self {
        // 查找 .git 目录作为 git root
        let git_root = working_dir
            .ancestors()
            .find(|p| p.join(".git").exists())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| working_dir.to_path_buf());
        
        let gitignore_path = git_root.join(".gitignore");
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&git_root);
        
        if gitignore_path.exists() {
            let _ = builder.add(&gitignore_path);
        }
        
        // 始终添加默认忽略规则
        let _ = builder.add_line(None, ".git/");
        
        let gitignore = builder.build().unwrap_or_else(|_| {
            // 如果构建失败，返回空的 gitignore（不忽略任何东西）
            ignore::gitignore::GitignoreBuilder::new(&git_root).build().unwrap()
        });
        
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
    
    pub fn git_root(&self) -> &Path {
        &self.git_root
    }
}

/// 工具重试配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolRetries {
    pub enabled: bool,
    pub max_attempts: usize,
}

impl Default for ToolRetries {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
        }
    }
}
