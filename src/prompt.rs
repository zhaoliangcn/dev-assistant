//! 系统提示词构建。

use crate::llm::ToolSchema;
use crate::skills::Skill;

/// 构建 Agent 的系统提示词。
///
/// 提示词由三部分组成：
/// 1. 工具说明（从注册的工具 schema 动态生成）
/// 2. 技能说明（从项目 `skills/` 目录发现的技能）
/// 3. 固定的行为规则与工作流程
pub fn build_system_prompt(tool_schemas: &[ToolSchema], skills: &[Skill]) -> String {
    let tool_descriptions = format_tool_descriptions(tool_schemas);
    let skills_prompt = crate::skills::format_skills_for_prompt(skills);

    format!(
        r#"你是一个软件工程师助手。使用可用工具完成用户任务。

## 可用工具
{tools}

{skills}

## 核心规则

### 安全约束
1. **危险操作要谨慎**：对 `rm -rf`、`sudo`、`dd` 等破坏性命令须三思，确认后再执行
2. **不读取构建产物目录**：绝不读取 `target/`、`node_modules/`、`.git/`、`dist/`、`build/` 等目录中的文件（二进制产物，非源代码）
3. **遵守安全策略**：工具执行时会自动进行安全评估，若提示危险等级为 Critical/High，请听从拦截结果

### 工作流程
4. **先了解再行动**：新任务先通过 `glob` 或 `list_directory` 了解项目结构，再进行操作
5. **复杂任务先规划**：对于多步骤任务，先制定计划再逐步执行。考虑使用 `spawn_subagent` 并行处理独立子任务
6. **每次读取后记录**：读取文件后，简要记录你看到了什么，避免后续遗忘
7. **避免无限循环**：读取足够信息后给出结论，不要反复读取同一批文件
8. **完成任务必须调用 `finish`**：完成后调用 `finish(summary="...")` 工具结束，提供任务完成总结。不要仅输出文本而不调用 finish

### 错误处理
9. **工具失败时**：① 阅读错误信息 ② 判断是临时错误（如网络超时）还是永久错误（如参数错误） ③ 临时错误可重试 1-2 次 ④ 永久错误则换方法或报告给用户
10. **编译失败时**：阅读错误信息，定位问题代码，修复后重试
11. **遇到不确定时**：查阅 KnowledgeBase（`kb_query`）了解已有决策，而不是凭空猜测

### 特殊工具说明
- **spawn_subagent**：创建子代理执行独立子任务。适用于：文件搜索分析、并行研究、复杂任务分解。可选 `agent_type`：architect（架构师）、implementer（实现者）、reviewer（审查员）、tester（测试员）、debugger（调试专家）、general（通用代理）。子代理有深度限制（最多 3 层），不要创建子代理的子代理
  示例：`spawn_subagent(task="分析 src/main.rs 的代码结构", agent_type="architect")`
- **exec_command**：直接执行程序，`command` 为可执行文件名，`args` 为参数列表（如 `command="cargo", args=["build"]`）。**不支持** shell 语法（管道 `|`、重定向 `>`、`&&`、`||` 等），也不支持 `sh -c`。每个调用只能执行一个命令
- **restart**：修改 Rust 源代码后调用 `restart` 自动编译验证。调用后进程会重启并自动恢复对话，**重启后不要再调用 restart**
- **edit_file**：编辑现有文件，需要提供 `old_content`（文件中准确的旧内容）和 `new_content`
- **kb_store / kb_query**：KnowledgeBase 系统用于跨子代理共享架构决策、接口定义、问题追踪。开始新任务前先 `kb_query` 了解已有信息

## 工作提示
- 对于复杂任务，先分析问题、制定步骤，再逐步执行
- 使用 `spawn_subagent` 分解独立子任务，提高效率
- 修改代码后应及时验证（编译/测试）
- 保持输出简洁，聚焦关键信息
"#,
        tools = tool_descriptions.trim(),
        skills = skills_prompt.trim()
    )
}

fn format_tool_descriptions(tool_schemas: &[ToolSchema]) -> String {
    let mut buf = String::new();
    for schema in tool_schemas {
        buf.push_str(&format!(
            "- {}: {}\n",
            schema.function.name, schema.function.description
        ));
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolFunctionSchema, ToolSchema};
    use crate::skills::{Skill, SkillMetadata};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn tool_schema(name: &str, desc: &str) -> ToolSchema {
        ToolSchema {
            tool_type: "function".to_string(),
            function: ToolFunctionSchema {
                name: name.to_string(),
                description: desc.to_string(),
                parameters: serde_json::json!({}),
            },
        }
    }

    fn skill_named(name: &str) -> Skill {
        let meta = SkillMetadata {
            name: name.to_string(),
            description: "test".to_string(),
            when_to_use: None,
            version: None,
            author: None,
            metadata: HashMap::new(),
        };
        let keywords = Skill::compute_keywords(&meta);
        Skill {
            meta,
            body: String::new(),
            source_path: PathBuf::new(),
            keywords,
        }
    }

    #[test]
    fn build_prompt_includes_tool_descriptions() {
        let schemas = vec![
            tool_schema("read_file", "Read a file"),
            tool_schema("write_file", "Write a file"),
        ];
        let prompt = build_system_prompt(&schemas, &[]);

        assert!(prompt.contains("- read_file: Read a file"), "missing read_file entry");
        assert!(prompt.contains("- write_file: Write a file"), "missing write_file entry");
    }

    #[test]
    fn build_prompt_includes_core_rules() {
        let prompt = build_system_prompt(&[], &[]);

        // 固定行为规则不应漏
        assert!(prompt.contains("finish"), "missing finish rule");
        assert!(prompt.contains("绝不读取"), "missing build-dir guard");
        assert!(prompt.contains("rm -rf"), "missing dangerous-ops caution");
        assert!(prompt.contains("spawn_subagent"), "missing spawn_subagent tool description");
        assert!(prompt.contains("核心规则"), "missing core rules section");
    }

    #[test]
    fn build_prompt_with_no_tools_still_valid() {
        // 空工具列表不应 panic，且仍含规则段
        let prompt = build_system_prompt(&[], &[]);
        assert!(prompt.contains("核心规则"), "missing core rules section");
        assert!(prompt.contains("你是一个软件工程师助手"));
    }

    #[test]
    fn build_prompt_includes_skills_section() {
        let skills = vec![skill_named("code-review")];
        let prompt = build_system_prompt(&[], &skills);

        assert!(prompt.contains("code-review"), "missing skill name in prompt");
    }

    #[test]
    fn format_tool_descriptions_each_on_its_own_line() {
        let schemas = vec![
            tool_schema("a", "desc a"),
            tool_schema("b", "desc b"),
            tool_schema("c", "desc c"),
        ];
        let desc = format_tool_descriptions(&schemas);
        let lines: Vec<&str> = desc.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 3, "expected 3 lines, got {}: {:?}", lines.len(), lines);
        assert!(lines[0].starts_with("- a:"));
        assert!(lines[1].starts_with("- b:"));
        assert!(lines[2].starts_with("- c:"));
    }
}
