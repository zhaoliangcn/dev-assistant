//! 工具安全规则。
//!
//! 每个工具绑定一个 [`ToolSecuritySpec`]（参数路径提取 + 校验类型），
//! 由 [`SecurityPolicy::evaluate_tool`] 统一调度。
//!
//! 注意：本模块只描述"工具需要什么校验"，实际的路径校验逻辑
//! 仍然在 `security` 模块中，通过 `SecurityPolicy::validate_path*` 完成。

use std::path::{Path, PathBuf};

use crate::security::SecurityPolicy;
use crate::utils::error::AppError;

/// 路径校验类型。
#[derive(Debug, Clone, Copy)]
pub enum PathValidation {
    /// 路径必须存在且在允许目录内（用于 read_file / edit_file）
    Exists,
    /// 路径可能不存在，但其父目录必须在允许目录内（用于 write_file）
    ExistsParent,
    /// 路径所在目录（可能不存在）必须在允许目录内（用于 file_exists / list_directory / batch_read_files）
    PathExists,
    /// 不做路径校验。保留为未来非路径工具的扩展点。
    #[allow(dead_code)]
    None,
}

impl PathValidation {
    fn validate(
        self,
        policy: &SecurityPolicy,
        path: &str,
    ) -> Result<PathBuf, AppError> {
        match self {
            PathValidation::Exists => policy.validate_path(path),
            PathValidation::ExistsParent => policy.validate_parent_path(path),
            PathValidation::PathExists => policy.validate_path_exists(path),
            PathValidation::None => Ok(PathBuf::from(path)),
        }
    }
}

/// 单个工具的安全规则。
#[derive(Debug, Clone, Copy)]
pub struct ToolSecuritySpec {
    /// JSON 参数中路径字段的名字（如 "file_path" / "dir_path" / "pattern"）
    pub path_field: &'static str,
    /// 路径校验类型
    pub validation: PathValidation,
    /// true 表示路径字段是数组（如 batch_read_files 的 "files"）
    pub multiple: bool,
}

impl ToolSecuritySpec {
    fn extract_paths(&self, arguments: &serde_json::Value) -> Vec<String> {
        if self.multiple {
            arguments[self.path_field]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default()
        } else {
            arguments[self.path_field]
                .as_str()
                .map(|s| vec![s.to_string()])
                .unwrap_or_default()
        }
    }

    /// 对 spec 中声明的每个路径做 `is_dangerous_file` 检查 + `validate_path*` 检查。
    ///
    /// 返回 `Ok(())` 表示所有路径都通过校验；
    /// 返回 `Err` 包含评估失败的原因和级别。
    pub fn evaluate(
        &self,
        policy: &SecurityPolicy,
        arguments: &serde_json::Value,
    ) -> Result<(), SpecEvaluation> {
        let paths = self.extract_paths(arguments);
        for path in paths {
            let file_name = Path::new(&path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&path);

            if policy.is_dangerous_file(file_name) {
                return Err(SpecEvaluation::DangerousFile(path));
            }

            if let Err(e) = self.validation.validate(policy, &path) {
                return Err(SpecEvaluation::InvalidPath(e.to_string()));
            }
        }
        Ok(())
    }
}

/// spec 评估失败的原因。
pub enum SpecEvaluation {
    /// 命中危险文件列表
    DangerousFile(String),
    /// 路径校验失败
    InvalidPath(String),
}

impl SpecEvaluation {
    pub fn into_parts(self) -> (crate::security::DangerLevel, String) {
        match self {
            SpecEvaluation::DangerousFile(path) => (
                crate::security::DangerLevel::High,
                format!("Access to sensitive file '{}' requires approval", path),
            ),
            SpecEvaluation::InvalidPath(reason) => (
                crate::security::DangerLevel::Critical,
                reason,
            ),
        }
    }
}

/// 查表：工具名 → 安全 spec。
///
/// 新增工具只需在此处添加一行，无需修改 `evaluate_tool` 主体。
pub fn tool_security_spec(tool_name: &str) -> Option<ToolSecuritySpec> {
    match tool_name {
        "read_file" | "edit_file" => Some(ToolSecuritySpec {
            path_field: "file_path",
            validation: PathValidation::Exists,
            multiple: false,
        }),
        "file_exists" | "list_directory" => Some(ToolSecuritySpec {
            path_field: "file_path",
            validation: PathValidation::PathExists,
            multiple: false,
        }),
        "write_file" => Some(ToolSecuritySpec {
            path_field: "file_path",
            validation: PathValidation::ExistsParent,
            multiple: false,
        }),
        "batch_read_files" => Some(ToolSecuritySpec {
            path_field: "files",
            validation: PathValidation::PathExists,
            multiple: true,
        }),
        _ => None,
    }
}
