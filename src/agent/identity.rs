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

    /// 身份的中文显示名（与 `to_str` 一一对应）。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Architect => "架构师",
            Self::Implementer => "实现者",
            Self::Reviewer => "审查员",
            Self::Tester => "测试员",
            Self::Debugger => "调试专家",
            Self::General => "通用代理",
        }
    }

    /// 全部身份，按固定顺序排列（用于提示词中的 `agent_type` 列表等）。
    pub fn all() -> [Self; 6] {
        [
            Self::Architect,
            Self::Implementer,
            Self::Reviewer,
            Self::Tester,
            Self::Debugger,
            Self::General,
        ]
    }

    /// 通用规则（工具优先级、输出规范、终止规则合并，各身份共用）
    fn shared_guide() -> &'static str {
        "通用规则\n工具：kb_query→batch_read_files→kb_store→finish\n输出：finish(summary=...)含完成内容、关键发现、修改文件\n终止：批量读、不重读、不写中间文件、完成即finish"
    }

    /// 提示词中工具的固定展示顺序（阅读友好，`finish` 保持在末尾）。
    fn tool_display_order() -> &'static [&'static str] {
        &[
            "read_file",
            "batch_read_files",
            "read_symbol",
            "write_file",
            "edit_file",
            "exec_command",
            "glob",
            "list_directory",
            "file_exists",
            "context_budget",
            "compress_context",
            "save_summary",
            "kb_store",
            "kb_query",
            "finish",
        ]
    }

    /// 从 `default_tools()` 生成工具列表字符串（单一数据源，与工具注册保持一致）。
    fn tool_list(&self) -> String {
        let tools = self.default_tools();
        let mut extra: Vec<&str> = tools
            .iter()
            .map(String::as_str)
            .filter(|t| !Self::tool_display_order().contains(t))
            .collect();
        extra.sort_unstable();
        Self::tool_display_order()
            .iter()
            .copied()
            .filter(|t| tools.contains(*t))
            .chain(extra)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn system_prompt(&self) -> String {
        let base = match self {
            Self::Architect => {
                r#"你是一位资深架构师。

职责：设计架构、定义模块接口、制定技术选型、识别风险。

工作方式：
1. 分析需求，识别关键质量属性（可扩展性、性能、安全性）
2. 选择架构风格（分层、模块化、事件驱动等）
3. 设计模块结构，关注高内聚低耦合
4. 用 kb_store 记录决策，kb_query 了解约束
5. 为实现者提供清晰的规范

输出：架构图、模块列表、接口定义、数据流向图、关键决策记录（ADR）

注意：不负责实现代码，只负责设计和规划。"#
                    .to_string()
            }
            Self::Implementer => {
                r#"你是一位专业的软件工程师。

职责：按架构规范实现代码，编写高质量、可维护的代码。

工作方式：
1. kb_query 查询设计文档和接口定义
2. batch_read_files 了解现有代码风格
3. 实现功能，关注防御性编程和错误处理
4. 编写单元测试（正常路径、边界条件、异常场景）
5. kb_store 记录修改的文件和进展

输出：修改的文件列表、关键代码说明、测试结果、遇到的问题

注意：严格遵循设计规范，不擅自修改接口定义。"#
                    .to_string()
            }
            Self::Reviewer => {
                r#"你是一位严谨的代码审查员。

职责：审查代码质量与安全，检查设计规范遵循情况，识别 bug 和性能问题。

工作方式：
1. batch_read_files 批量读取所有相关文件
2. 审查维度：安全性（OWASP Top 10）、错误处理、代码质量、并发安全、性能、API 设计
3. 汇总问题，按严重程度分级（Critical/High/Medium/Low）

输出：问题清单（含严重程度）、改进建议（含代码片段）、整体质量评估

注意：保持客观中立，提供可操作的建议。"#
                    .to_string()
            }
            Self::Tester => {
                r#"你是一位专业的测试工程师。

职责：编写全面测试用例，运行测试并分析结果，确保代码质量。

工作方式：
1. kb_query 查询接口定义和实现代码
2. 设计测试策略（单元测试、集成测试）
3. 应用测试技术：等价类划分、边界值分析、错误推测法
4. 编写并运行测试，分析失败原因
5. kb_store 记录测试结果

输出：测试用例列表、结果摘要（通过/失败/跳过）、bug 报告、覆盖率统计

注意：覆盖正常路径、边界情况和异常场景。"#
                    .to_string()
            }
            Self::Debugger => {
                r#"你是一位经验丰富的调试专家。

职责：分析编译错误和测试失败，定位根因，提供修复方案并验证。

工作方式：
1. 分析错误信息和堆栈跟踪
2. 定位问题：编译错误（类型/生命周期）、测试失败（断言/边界）、运行时错误（panic/unwrap）
3. 设计修复方案，遵循最小改动原则
4. 实施修复并验证（编译+测试）
5. kb_store 记录问题和解决方案

输出：问题分析（根因）、修复方案、修改的文件、验证结果

注意：先定位根因再修复，一次只修复一个问题。"#
                    .to_string()
            }
            Self::General => {
                r#"你是一个通用子代理，负责完成父代理分配的任务。

职责：按任务描述完成工作，输出结构化结果，专注当前任务。

工作方式：
1. 理解任务目标，基于已有信息做出合理判断
2. 制定计划，按步骤推进
3. 定期 kb_query 了解已有信息
4. 完成后 kb_store 保存关键信息

输出：finish(summary=...) 包含：完成了什么、关键发现/决策、修改的文件、未解决的问题。
不要保存中间文件，结果直接通过 finish 输出。

错误处理：工具失败时重试 1-2 次，永久错误换方法并注明；需求不明确时基于已有信息判断并备注假设。"#
                    .to_string()
            }
        };

        // 为所有身份统一附加：工具列表（从 default_tools 生成）、通用规则（工具优先级、输出规范、防死循环终止规则）
        format!(
            "{}\n\n可用工具：\n- {}\n\n{}",
            base,
            self.tool_list(),
            Self::shared_guide()
        )
    }

    pub fn default_tools(&self) -> HashSet<String> {
        match self {
            Self::Architect => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "read_symbol".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "exec_command".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "context_budget".to_string(),
                "compress_context".to_string(),
                "save_summary".to_string(),
                "finish".to_string(),
            ]),
            Self::Implementer => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "read_symbol".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "context_budget".to_string(),
                "compress_context".to_string(),
                "save_summary".to_string(),
                "finish".to_string(),
            ]),
            Self::Reviewer => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "read_symbol".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "context_budget".to_string(),
                "compress_context".to_string(),
                "save_summary".to_string(),
                "finish".to_string(),
            ]),
            Self::Tester => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "read_symbol".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "context_budget".to_string(),
                "compress_context".to_string(),
                "save_summary".to_string(),
                "finish".to_string(),
            ]),
            Self::Debugger => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "read_symbol".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "context_budget".to_string(),
                "compress_context".to_string(),
                "save_summary".to_string(),
                "finish".to_string(),
            ]),
            Self::General => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "read_symbol".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "context_budget".to_string(),
                "compress_context".to_string(),
                "save_summary".to_string(),
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
    fn system_prompt_includes_tools_section() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            let prompt = identity.system_prompt();
            assert!(prompt.contains("可用工具"), "{} missing 可用工具 section", identity.to_str());
        }
    }

    #[test]
    fn system_prompt_includes_finish_instruction() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            let prompt = identity.system_prompt();
            assert!(prompt.contains("finish"), "{} missing finish instruction", identity.to_str());
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
    fn architect_has_kb_and_exec_tools() {
        let tools = AgentIdentity::Architect.default_tools();
        assert!(tools.contains("kb_store"));
        assert!(tools.contains("kb_query"));
        assert!(tools.contains("exec_command"));
    }

    #[test]
    fn reviewer_has_batch_read_files() {
        let tools = AgentIdentity::Reviewer.default_tools();
        assert!(tools.contains("batch_read_files"));
    }

    #[test]
    fn implementer_has_kb_store() {
        let tools = AgentIdentity::Implementer.default_tools();
        assert!(tools.contains("kb_store"));
    }

    #[test]
    fn debugger_has_batch_read_files() {
        let tools = AgentIdentity::Debugger.default_tools();
        assert!(tools.contains("batch_read_files"));
    }

    #[test]
    fn tester_has_exec_command() {
        let tools = AgentIdentity::Tester.default_tools();
        assert!(tools.contains("exec_command"));
        assert!(tools.contains("kb_store"));
    }

    #[test]
    fn general_has_kb_tools() {
        let tools = AgentIdentity::General.default_tools();
        assert!(tools.contains("kb_store"), "General should have kb_store");
        assert!(tools.contains("kb_query"), "General should have kb_query");
    }

    #[test]
    fn tool_list_matches_default_tools() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            let tool_list = identity.tool_list();
            let list: std::collections::HashSet<&str> = tool_list.split(", ").collect();
            let default_tools = identity.default_tools();
            let expected: std::collections::HashSet<&str> =
                default_tools.iter().map(String::as_str).collect();
            assert_eq!(
                list, expected,
                "{} tool_list diverges from default_tools",
                identity.to_str()
            );
        }
    }

    #[test]
    fn tool_list_is_deterministic_and_finish_last() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            let first = identity.tool_list();
            let second = identity.tool_list();
            assert_eq!(first, second, "{} tool_list is not deterministic", identity.to_str());
            assert!(
                first.trim_end().ends_with("finish"),
                "{} tool_list should end with finish: {}",
                identity.to_str(),
                first
            );
        }
    }

    #[test]
    fn system_prompt_includes_guard_guide_for_all() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            let prompt = identity.system_prompt();
            assert!(prompt.contains("终止"), "{} missing guard guide", identity.to_str());
            assert!(
                prompt.contains("不写中间文件"),
                "{} missing no-intermediate-files rule",
                identity.to_str()
            );
        }
    }

    #[test]
    fn system_prompt_tool_section_matches_default_tools() {
        for identity in [
            AgentIdentity::Architect,
            AgentIdentity::Implementer,
            AgentIdentity::Reviewer,
            AgentIdentity::Tester,
            AgentIdentity::Debugger,
            AgentIdentity::General,
        ] {
            let prompt = identity.system_prompt();
            assert!(
                prompt.contains(&format!("可用工具：\n- {}", identity.tool_list())),
                "{} prompt tool section mismatch",
                identity.to_str()
            );
        }
    }
}