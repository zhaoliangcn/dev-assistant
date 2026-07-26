use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::debug;

use super::DangerLevel;

type Timestamp = u64;

fn now_timestamp() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 审批状态
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

/// 审批需求定义
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ApprovalRequirement {
    /// 审批类型
    pub approval_type: ApprovalType,
    /// 危险级别阈值（超过此级别需要审批）
    pub danger_threshold: DangerLevel,
    /// 是否需要用户显式确认
    pub requires_user_confirmation: bool,
    /// 审批有效期（秒），0 表示永久有效
    pub validity_seconds: u64,
    /// 审批范围（路径、命令等）
    pub scope: ApprovalScope,
}

impl ApprovalRequirement {
    /// 创建默认的审批需求
    pub fn default_for_danger(level: &DangerLevel) -> Self {
        match level {
            DangerLevel::Critical => Self {
                approval_type: ApprovalType::OneTime,
                danger_threshold: DangerLevel::Critical,
                requires_user_confirmation: true,
                validity_seconds: 0,
                scope: ApprovalScope::Command,
            },
            DangerLevel::High => Self {
                approval_type: ApprovalType::Session,
                danger_threshold: DangerLevel::High,
                requires_user_confirmation: true,
                validity_seconds: 3600, // 1小时
                scope: ApprovalScope::Path,
            },
            DangerLevel::Medium => Self {
                approval_type: ApprovalType::Session,
                danger_threshold: DangerLevel::Medium,
                requires_user_confirmation: true,
                validity_seconds: 1800, // 30分钟
                scope: ApprovalScope::Path,
            },
            DangerLevel::Low => Self {
                approval_type: ApprovalType::Auto,
                danger_threshold: DangerLevel::Low,
                requires_user_confirmation: false,
                validity_seconds: 0,
                scope: ApprovalScope::None,
            },
        }
    }
}

/// 审批类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalType {
    /// 自动审批（无需用户确认）
    Auto,
    /// 一次性审批（只对当前操作有效）
    OneTime,
    /// 会话级审批（在有效期内对同类操作有效）
    Session,
}

/// 审批范围
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ApprovalScope {
    /// 无限制
    None,
    /// 按命令类型
    Command,
    /// 按路径
    Path,
    /// 按工具
    Tool,
}

/// 已审批的权限记录
#[derive(Debug, Clone)]
pub struct PermissionEntry {
    /// 审批的工具名
    pub tool_name: String,
    /// 审批的范围标识符
    pub scope_id: String,
    /// 危险级别
    pub danger_level: DangerLevel,
    /// 审批状态
    pub status: ApprovalStatus,
    /// 审批时间（Unix时间戳）
    pub approved_at: Timestamp,
    /// 有效期（秒）
    pub validity_seconds: u64,
}

impl PermissionEntry {
    /// 检查权限是否仍有效
    pub fn is_valid(&self) -> bool {
        if self.status != ApprovalStatus::Approved {
            return false;
        }
        if self.validity_seconds == 0 {
            return true; // 永久有效
        }
        let now = now_timestamp();
        (now - self.approved_at) < self.validity_seconds
    }
}

/// 权限存储（线程安全）
#[derive(Debug, Clone)]
pub struct PermissionStore {
    permissions: Arc<RwLock<HashMap<String, Vec<PermissionEntry>>>>,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self {
            permissions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 添加审批权限
    pub fn add_permission(&self, entry: PermissionEntry) {
        let mut permissions = self.permissions.write().unwrap();
        let key = Self::make_key(&entry.tool_name, &entry.scope_id);
        debug!(
            tool = entry.tool_name,
            scope = entry.scope_id,
            "Permission added"
        );
        permissions.entry(key).or_insert_with(Vec::new).push(entry);
    }

    /// 检查是否存在有效审批
    pub fn has_permission(
        &self,
        tool_name: &str,
        scope_id: &str,
        danger_level: &DangerLevel,
    ) -> bool {
        let permissions = self.permissions.read().unwrap();
        let key = Self::make_key(tool_name, scope_id);
        
        if let Some(entries) = permissions.get(&key) {
            for entry in entries {
                if entry.is_valid() && entry.danger_level == *danger_level {
                    return true;
                }
            }
        }
        false
    }

    /// 移除过期权限
    #[allow(dead_code)]
    pub fn cleanup_expired(&self) {
        let mut permissions = self.permissions.write().unwrap();
        permissions.retain(|_, entries| {
            entries.retain(|entry| entry.is_valid());
            !entries.is_empty()
        });
    }

    /// 撤销权限
    #[allow(dead_code)]
    pub fn revoke_permission(&self, tool_name: &str, scope_id: &str) {
        let mut permissions = self.permissions.write().unwrap();
        let key = Self::make_key(tool_name, scope_id);
        if let Some(entries) = permissions.get_mut(&key) {
            entries.retain(|entry| entry.status != ApprovalStatus::Approved);
        }
        debug!(tool = tool_name, scope = scope_id, "Permission revoked");
    }

    /// 获取所有有效权限
    #[allow(dead_code)]
    pub fn get_all_permissions(&self) -> Vec<PermissionEntry> {
        let permissions = self.permissions.read().unwrap();
        permissions
            .values()
            .flat_map(|entries| entries.iter().filter(|e| e.is_valid()).cloned())
            .collect()
    }

    fn make_key(tool_name: &str, scope_id: &str) -> String {
        format!("{}:{}", tool_name, scope_id)
    }
}

impl Default for PermissionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 审批请求
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ApprovalRequest {
    /// 请求ID
    pub request_id: String,
    /// 工具名
    pub tool_name: String,
    /// 工具参数（脱敏后）
    pub arguments: String,
    /// 危险级别
    pub danger_level: DangerLevel,
    /// 审批原因
    pub reason: String,
    /// 请求时间（Unix时间戳）
    pub requested_at: Timestamp,
}

/// 审批管理器
#[derive(Debug, Clone)]
pub struct ApprovalManager {
    permission_store: PermissionStore,
    pending_requests: Arc<RwLock<HashMap<String, ApprovalRequest>>>,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            permission_store: PermissionStore::new(),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 检查是否需要审批
    pub fn requires_approval(
        &self,
        tool_name: &str,
        scope_id: &str,
        danger_level: &DangerLevel,
    ) -> bool {
        // Low 级别不需要审批
        if *danger_level == DangerLevel::Low {
            return false;
        }

        // 检查是否已有有效审批
        if self.permission_store.has_permission(tool_name, scope_id, danger_level) {
            return false;
        }

        true
    }

    /// 创建审批请求
    pub fn create_request(
        &self,
        tool_name: &str,
        arguments: &str,
        danger_level: DangerLevel,
        reason: &str,
    ) -> ApprovalRequest {
        let request_id = format!("{}-{}", tool_name, now_timestamp());
        let request = ApprovalRequest {
            request_id: request_id.clone(),
            tool_name: tool_name.to_string(),
            arguments: arguments.to_string(),
            danger_level,
            reason: reason.to_string(),
            requested_at: now_timestamp(),
        };

        let mut pending = self.pending_requests.write().unwrap();
        pending.insert(request_id, request.clone());

        request
    }

    /// 审批通过
    pub fn approve(&self, request_id: &str) -> bool {
        let mut pending = self.pending_requests.write().unwrap();
        if let Some(request) = pending.remove(request_id) {
            let requirement = ApprovalRequirement::default_for_danger(&request.danger_level);
            
            self.permission_store.add_permission(PermissionEntry {
                tool_name: request.tool_name.clone(),
                scope_id: self.extract_scope_id(&request),
                danger_level: request.danger_level,
                status: ApprovalStatus::Approved,
                approved_at: now_timestamp(),
                validity_seconds: requirement.validity_seconds,
            });

            debug!(request_id, tool = request.tool_name, "Approval granted");
            true
        } else {
            false
        }
    }

    /// 拒绝审批
    pub fn reject(&self, request_id: &str) -> bool {
        let mut pending = self.pending_requests.write().unwrap();
        if pending.remove(request_id).is_some() {
            debug!(request_id, "Approval rejected");
            true
        } else {
            false
        }
    }

    /// 获取待审批请求
    pub fn get_pending_requests(&self) -> Vec<ApprovalRequest> {
        let pending = self.pending_requests.read().unwrap();
        pending.values().cloned().collect()
    }

    /// 添加权限（直接，用于会话恢复或预授权）
    pub fn add_permission_directly(
        &self,
        tool_name: &str,
        scope_id: &str,
        danger_level: DangerLevel,
        validity_seconds: u64,
    ) {
        self.permission_store.add_permission(PermissionEntry {
            tool_name: tool_name.to_string(),
            scope_id: scope_id.to_string(),
            danger_level,
            status: ApprovalStatus::Approved,
            approved_at: now_timestamp(),
            validity_seconds,
        });
    }

    fn extract_scope_id(&self, request: &ApprovalRequest) -> String {
        // 简化实现：使用工具名作为 scope_id
        // 实际应用中可以从 arguments 中提取路径等信息
        request.tool_name.clone()
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn approval_requirement_default_for_danger() {
        let critical = ApprovalRequirement::default_for_danger(&DangerLevel::Critical);
        assert_eq!(critical.approval_type, ApprovalType::OneTime);
        assert!(critical.requires_user_confirmation);

        let high = ApprovalRequirement::default_for_danger(&DangerLevel::High);
        assert_eq!(high.approval_type, ApprovalType::Session);
        assert_eq!(high.validity_seconds, 3600);

        let low = ApprovalRequirement::default_for_danger(&DangerLevel::Low);
        assert_eq!(low.approval_type, ApprovalType::Auto);
        assert!(!low.requires_user_confirmation);
    }

    #[test]
    fn permission_entry_is_valid() {
        let entry = PermissionEntry {
            tool_name: "test".to_string(),
            scope_id: "test".to_string(),
            danger_level: DangerLevel::High,
            status: ApprovalStatus::Approved,
            approved_at: now_timestamp(),
            validity_seconds: 3600,
        };
        assert!(entry.is_valid());

        let expired = PermissionEntry {
            validity_seconds: 1,
            ..entry.clone()
        };
        // 等待1秒让权限过期
        std::thread::sleep(Duration::from_secs(2));
        assert!(!expired.is_valid());
    }

    #[test]
    fn permission_store_add_and_check() {
        let store = PermissionStore::new();
        
        let entry = PermissionEntry {
            tool_name: "test_tool".to_string(),
            scope_id: "test_scope".to_string(),
            danger_level: DangerLevel::High,
            status: ApprovalStatus::Approved,
            approved_at: now_timestamp(),
            validity_seconds: 3600,
        };
        
        store.add_permission(entry);
        assert!(store.has_permission("test_tool", "test_scope", &DangerLevel::High));
        assert!(!store.has_permission("test_tool", "other_scope", &DangerLevel::High));
    }

    #[test]
    fn approval_manager_requires_approval() {
        let manager = ApprovalManager::new();
        
        // Low 级别不需要审批
        assert!(!manager.requires_approval("test", "scope", &DangerLevel::Low));
        
        // High 级别需要审批
        assert!(manager.requires_approval("test", "scope", &DangerLevel::High));
        
        // 添加权限后不需要审批
        manager.add_permission_directly("test", "scope", DangerLevel::High, 3600);
        assert!(!manager.requires_approval("test", "scope", &DangerLevel::High));
    }

    #[test]
    fn approval_manager_create_and_approve() {
        let manager = ApprovalManager::new();
        
        let request = manager.create_request(
            "test_tool",
            "{\"path\": \"/etc/passwd\"}",
            DangerLevel::High,
            "Access sensitive file",
        );
        
        assert_eq!(manager.get_pending_requests().len(), 1);
        
        assert!(manager.approve(&request.request_id));
        assert_eq!(manager.get_pending_requests().len(), 0);
        
        assert!(!manager.requires_approval("test_tool", "test_tool", &DangerLevel::High));
    }

    #[test]
    fn approval_manager_reject() {
        let manager = ApprovalManager::new();
        
        let request = manager.create_request(
            "test_tool",
            "{\"command\": \"rm -rf /\"}",
            DangerLevel::Critical,
            "Dangerous command",
        );
        
        assert!(manager.reject(&request.request_id));
        assert_eq!(manager.get_pending_requests().len(), 0);
        
        // 拒绝后仍然需要审批
        assert!(manager.requires_approval("test_tool", "test_tool", &DangerLevel::Critical));
    }
}
