use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[derive(Default)]
pub enum AgentIdentity {
    Architect,
    Implementer,
    Reviewer,
    Tester,
    Debugger,
    #[default]
    General,
}

impl AgentIdentity {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "architect" => Some(Self::Architect),
            "implementer" => Some(Self::Implementer),
            "reviewer" => Some(Self::Reviewer),
            "tester" => Some(Self::Tester),
            "debugger" => Some(Self::Debugger),
            "general" => Some(Self::General),
            _ => None,
        }
    }

    pub fn to_str(&self) -> &str {
        match self {
            Self::Architect => "architect",
            Self::Implementer => "implementer",
            Self::Reviewer => "reviewer",
            Self::Tester => "tester",
            Self::Debugger => "debugger",
            Self::General => "general",
        }
    }

    pub fn system_prompt(&self) -> String {
        match self {
            Self::Architect => {
                r#"你是一位资深架构师。

职责：
- 设计软件系统的整体架构和模块划分
- 定义模块间的接口和数据交互
- 制定技术选型和设计决策
- 识别潜在的风险和优化机会

工作方式：
1. 分析需求和约束
2. 设计模块结构和接口定义
3. 记录架构决策（使用 kb_store 工具）
4. 为实现者提供清晰的规范和指导
5. 使用 finish 工具提交设计方案

输出格式：
- 架构图（文本描述）
- 模块列表和职责说明
- 接口定义（输入输出）
- 数据流向图
- 关键决策和理由

注意：你不负责实现代码，只负责设计和规划。"#
                    .to_string()
            }
            Self::Implementer => {
                r#"你是一位专业的软件工程师。

职责：
- 按照架构师提供的规范实现代码
- 编写高质量、可维护的代码
- 遵循编码规范和最佳实践
- 完成后进行基础测试验证

工作方式：
1. 阅读接口定义和设计文档（使用 kb_query 工具）
2. 实现功能代码
3. 编写单元测试
4. 记录实现进展（使用 kb_store 工具）
5. 使用 finish 工具提交实现成果

输出格式：
- 修改的文件列表
- 关键代码片段说明
- 测试结果
- 实现中遇到的问题和解决方案

注意：严格遵循设计规范，不擅自修改接口定义。"#
                    .to_string()
            }
            Self::Reviewer => {
                r#"你是一位严谨的代码审查员。

职责：
- 审查代码质量和安全性
- 检查是否符合设计规范
- 识别潜在的 bug 和性能问题
- 提供改进建议

工作方式：
1. 阅读相关设计文档和代码（使用 kb_query 工具）
2. 审查代码逻辑、安全性、性能
3. 列出问题清单和改进建议
4. 使用 kb_store 工具记录审查结果
5. 使用 finish 工具提交审查报告

输出格式：
- 问题清单（严重程度分级）
- 改进建议
- 代码优化示例
- 审查总结

注意：保持客观中立，提供可操作的建议。"#
                    .to_string()
            }
            Self::Tester => {
                r#"你是一位专业的测试工程师。

职责：
- 编写全面的测试用例
- 运行测试并分析结果
- 报告测试覆盖率和问题
- 确保代码质量符合标准

工作方式：
1. 阅读接口定义和实现代码（使用 kb_query 工具）
2. 编写单元测试和集成测试
3. 运行测试并收集结果
4. 使用 kb_store 工具记录测试结果和问题
5. 使用 finish 工具提交测试报告

输出格式：
- 测试用例列表
- 测试结果摘要
- 问题清单（bug 报告）
- 测试覆盖率统计

注意：覆盖边界情况和异常场景。"#
                    .to_string()
            }
            Self::Debugger => {
                r#"你是一位经验丰富的调试专家。

职责：
- 分析编译错误和测试失败
- 定位问题根因
- 提供修复方案
- 验证修复效果

工作方式：
1. 分析错误信息和堆栈跟踪
2. 定位问题代码位置
3. 设计修复方案
4. 实施修复并验证
5. 使用 kb_store 工具记录问题和解决方案
6. 使用 finish 工具提交修复结果

输出格式：
- 问题分析（根因）
- 修复方案
- 修改的文件和代码
- 验证结果

注意：先定位根因再修复，避免盲目修改。"#
                    .to_string()
            }
            Self::General => {
                r#"你是一个子代理。你的职责是完成分配给你的任务。

规则：
1. 专注完成分配的任务
2. 完成后必须使用 finish 工具结束，提供任务完成总结
3. 重要信息写入输出中返回给父代理
4. 遵守安全策略
5. 不要调用 spawn_subagent 工具（它不可用）
6. 不要调用 restart 工具（它不可用）"#
                    .to_string()
            }
        }
    }

    pub fn default_tools(&self) -> HashSet<String> {
        match self {
            Self::Architect => HashSet::from([
                "read_file".to_string(),
                "write_file".to_string(),
                "glob".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Implementer => HashSet::from([
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Reviewer => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "glob".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Tester => HashSet::from([
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Debugger => HashSet::from([
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::General => HashSet::from([
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
        }
    }
}


/// 流水线阶段定义
#[derive(Debug, Clone)]
pub struct PipelineStage {
    pub name: String,
    pub agent_type: AgentIdentity,
    pub task_template: String,
    pub max_iterations: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_returns_correct_identity() {
        assert_eq!(AgentIdentity::from_str("architect"), Some(AgentIdentity::Architect));
        assert_eq!(AgentIdentity::from_str("IMPLEMENTER"), Some(AgentIdentity::Implementer));
        assert_eq!(AgentIdentity::from_str("Reviewer"), Some(AgentIdentity::Reviewer));
        assert_eq!(AgentIdentity::from_str("tester"), Some(AgentIdentity::Tester));
        assert_eq!(AgentIdentity::from_str("debugger"), Some(AgentIdentity::Debugger));
        assert_eq!(AgentIdentity::from_str("general"), Some(AgentIdentity::General));
        assert_eq!(AgentIdentity::from_str("unknown"), None);
    }

    #[test]
    fn to_str_returns_correct_string() {
        assert_eq!(AgentIdentity::Architect.to_str(), "architect");
        assert_eq!(AgentIdentity::Implementer.to_str(), "implementer");
        assert_eq!(AgentIdentity::General.to_str(), "general");
    }

    #[test]
    fn default_is_general() {
        assert_eq!(AgentIdentity::default(), AgentIdentity::General);
    }

    #[test]
    fn system_prompt_is_non_empty() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            assert!(!identity.system_prompt().is_empty());
        }
    }

    #[test]
    fn default_tools_contains_finish() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            assert!(identity.default_tools().contains("finish"));
        }
    }

    #[test]
    fn architect_has_kb_tools() {
        let tools = AgentIdentity::Architect.default_tools();
        assert!(tools.contains("kb_store"));
        assert!(tools.contains("kb_query"));
    }

    #[test]
    fn reviewer_has_batch_read_files() {
        let tools = AgentIdentity::Reviewer.default_tools();
        assert!(tools.contains("batch_read_files"));
    }
}