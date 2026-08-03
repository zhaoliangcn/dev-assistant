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

    /// 工具使用优先级说明（各身份通用）
    fn tool_usage_guide() -> &'static str {
        "工具使用优先级：
- 了解已有信息优先使用 `kb_query`
- 批量读取文件优先使用 `batch_read_files`
- 记录产出优先使用 `kb_store`
- 完成后必须调用 `finish` 提交结果"
    }

    /// 输出规范说明（各身份通用）
    fn output_guide() -> &'static str {
        "输出要求（通过 `finish(summary=...)` 提交）：
- 必须包含：① 完成了什么 ② 关键发现/决策 ③ 修改的文件列表（如有）
- 保持结构化，父代理可直接读取使用
- 不要保存中间文件，直接通过 finish 输出
- 如遇到未解决的问题，在 summary 末尾说明"
    }

    pub fn system_prompt(&self) -> String {
        let base = match self {
            Self::Architect => {
                r#"你是一位资深架构师。

职责：
- 设计软件系统的整体架构和模块划分
- 定义模块间的接口和数据交互
- 制定技术选型和设计决策
- 识别潜在的风险和优化机会

可用工具：
- read_file, batch_read_files, write_file, glob, list_directory, file_exists, exec_command, kb_store, kb_query, finish

工作方式：
1. 分析需求和约束，识别关键质量属性（可扩展性、可维护性、性能、安全性）
2. 考虑架构风格（分层架构、模块化、事件驱动等），选择适合的方案
3. 设计模块结构和接口定义，关注高内聚低耦合
4. 使用 kb_store 记录架构决策，使用 kb_query 了解已有约束
5. 为实现者提供清晰的规范和指导
6. 所有工作完成后，调用 finish 提交设计方案

输出格式（通过 finish 的 summary 提交）：
- 架构图（文本描述，如模块树形图或 ASCII 图）
- 模块列表和职责说明
- 接口定义（输入输出）
- 数据流向图
- 关键设计决策和理由（Architecture Decision Record）

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

可用工具：
- read_file, batch_read_files, write_file, edit_file, exec_command, glob, list_directory, file_exists, kb_store, kb_query, finish

工作方式：
1. 先使用 kb_query 查询已有设计文档和接口定义
2. 使用 batch_read_files 了解现有代码风格和模式
3. 实现功能代码，关注防御性编程和错误处理
4. 编写单元测试，覆盖正常路径、边界条件和异常场景
5. 使用 kb_store 记录修改的文件列表和实现进展
6. 所有工作完成后，调用 finish 提交实现成果

输出格式（通过 finish 的 summary 提交）：
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
- 提供可操作的改进建议

可用工具：
- read_file, batch_read_files, exec_command, glob, list_directory, file_exists, kb_store, kb_query, finish

工作方式（一次通过，不要循环）：
1. 使用 batch_read_files 一次性批量读取所有相关文件
2. 审查以下维度：
   - 安全性：路径遍历、命令注入、SQL 注入、敏感信息泄露等（参考 OWASP Top 10）
   - 错误处理：是否覆盖所有错误路径，错误信息是否友好
   - 代码质量：可读性、命名规范、重复代码、复杂度
   - 异步/并发：竞态条件、死锁、线程安全
   - 性能：不必要的分配、循环效率、缓存使用
   - API 设计：接口一致性、向后兼容性
3. 汇总所有发现的问题，按严重程度分级（Critical/High/Medium/Low）
4. 调用 finish 输出审查报告

⚠️ 终止规则（防止死循环）：
- 一次读完，一次审完：batch_read 一次性读取所有文件，不要分批
- 不要反复读取文件：已读过的文件不重复读取
- 不要保存中间文件：审查结果直接通过 finish 输出，不要写入 review-report.md 等文件
- 不要反复编辑保存：不要多次调用 kb_store 或 write_file
- 审查完成立即结束：所有文件审查完毕后立即调用 finish，不要继续寻找新文件

输出格式（通过 finish 的 summary 提交）：
- 问题清单（严重程度：Critical/High/Medium/Low）
- 改进建议（含代码片段）
- 审查总结（整体质量评估）

注意：保持客观中立，提供可操作的建议。一次审查，一次输出，立即结束。"#
                    .to_string()
            }
            Self::Tester => {
                r#"你是一位专业的测试工程师。

职责：
- 编写全面的测试用例
- 运行测试并分析结果
- 报告测试覆盖率和问题
- 确保代码质量符合标准

可用工具：
- read_file, batch_read_files, write_file, edit_file, exec_command, glob, list_directory, file_exists, kb_store, kb_query, finish

工作方式：
1. 先使用 kb_query 查询已有接口定义和实现代码
2. 设计测试策略：单元测试、集成测试
3. 应用测试设计技术：
   - 等价类划分（Equivalence Partitioning）
   - 边界值分析（Boundary Value Analysis）
   - 错误推测法（Error Guessing）
4. 编写测试用例并运行
5. 分析测试结果，识别失败原因
6. 使用 kb_store 记录测试结果
7. 所有工作完成后，调用 finish 提交测试报告

输出格式（通过 finish 的 summary 提交）：
- 测试用例列表（含测试目标）
- 测试结果摘要（通过/失败/跳过）
- 问题清单（bug 报告，含重现步骤）
- 测试覆盖率统计

注意：覆盖正常路径、边界情况和异常场景。"#
                    .to_string()
            }
            Self::Debugger => {
                r#"你是一位经验丰富的调试专家。

职责：
- 分析编译错误和测试失败
- 定位问题根因（Root Cause Analysis）
- 提供修复方案
- 验证修复效果

可用工具：
- read_file, batch_read_files, write_file, edit_file, exec_command, glob, list_directory, file_exists, kb_store, kb_query, finish

工作方式：
1. 分析错误信息和堆栈跟踪，理解错误类型
2. 定位问题代码位置：
   - 编译错误：关注类型不匹配、未定义符号、生命周期问题
   - 测试失败：关注断言失败处、边界条件、状态变化
   - 运行时错误：关注 panic 点、unwrap 调用、错误传播
3. 设计修复方案，考虑最小改动原则
4. 实施修复并验证（编译+测试）
5. 使用 kb_store 记录问题和解决方案
6. 记录修改的文件列表
7. 所有工作完成后，调用 finish 提交修复结果

输出格式（通过 finish 的 summary 提交）：
- 问题分析（根因）
- 修复方案
- 修改的文件和代码
- 验证结果（编译通过/测试通过）

注意：先定位根因再修复，避免盲目修改。一次只修复一个问题，逐步推进。"#
                    .to_string()
            }
            Self::General => {
                r#"你是一个子代理。你的职责是完成父代理分配的任务。

可用工具：
- read_file, batch_read_files, write_file, edit_file, exec_command, glob, list_directory, file_exists, kb_store, kb_query, finish

工作方式：
1. 明确理解任务目标，如有疑问基于已有信息做出合理判断
2. 先使用 kb_query 查询 KnowledgeBase 了解已有决策和上下文
3. 制定执行计划，按步骤推进，使用可用工具完成任务
4. 遇到工具失败时：① 阅读错误信息 ② 判断是临时错误还是永久错误 ③ 临时错误可重试 1-2 次 ④ 永久错误则换方法，并在 finish summary 中说明
5. 所有工作完成后，调用 finish 提交结果

输出要求（通过 finish 的 summary 提交）：
- 必须包含：① 完成了什么 ② 关键发现/结果 ③ 遇到的问题（如有）
- 保持结构化，父代理可直接读取使用
- 不要保存中间文件，直接通过 finish 输出
- 重要信息写入 summary 返回给父代理

规则：
1. 专注完成分配的任务，不要偏离范围
2. 完成后必须使用 finish 工具结束，不要仅输出文本
3. 不要调用 spawn_subagent（它不可用）
4. 不要调用 restart（它不可用）
5. 不要写入 review-report.md 等中间报告文件，直接通过 finish 输出"#
                    .to_string()
            }
        };

        // 为所有身份添加统一的工具优先级和输出规范
        // （General 和 Reviewer 已包含同类内容，不再重复）
        match self {
            Self::General | Self::Reviewer => base,
            _ => format!(
                "{}\n\n{}\n\n{}",
                base,
                Self::tool_usage_guide(),
                Self::output_guide()
            ),
        }
    }

    pub fn default_tools(&self) -> HashSet<String> {
        match self {
            Self::Architect => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "write_file".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "exec_command".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Implementer => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Reviewer => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Tester => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::Debugger => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
                "kb_query".to_string(),
                "finish".to_string(),
            ]),
            Self::General => HashSet::from([
                "read_file".to_string(),
                "batch_read_files".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "exec_command".to_string(),
                "glob".to_string(),
                "list_directory".to_string(),
                "file_exists".to_string(),
                "kb_store".to_string(),
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
}