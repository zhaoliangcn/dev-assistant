//! 系统提示词构建。

use crate::llm::ToolSchema;
use crate::skills::Skill;

/// 构建 Agent 的系统提示词。
///
/// 提示词由三部分组成：
/// 1. 工具说明（从注册的工具 schema 动态生成）
/// 2. 技能说明（从项目 `skills/` 目录发现的技能）
/// 3. 固定的行为规则
pub fn build_system_prompt(tool_schemas: &[ToolSchema], skills: &[Skill]) -> String {
    let tool_descriptions = format_tool_descriptions(tool_schemas);
    let skills_prompt = crate::skills::format_skills_for_prompt(skills);

    format!(
        r#"你是一个软件工程师助手。使用工具完成用户任务。

可用工具：
{tools}

技能说明（当任务匹配以下技能时，按照技能流程执行，使用上面的工具完成）：
{skills}

规则：
1. 先了解项目结构再进行操作
2. 对危险操作（rm -rf, sudo等）要谨慎
3. 完成任务后**必须**使用 finish 工具结束，提供任务完成总结
4. 用户可以输入 /quit 或 /exit 退出程序
5. restart 工具用于修改代码后自动编译验证。调用后进程会重启并自动恢复对话。**重启后不要再调用 restart 工具**，直接继续执行用户任务。

工具使用建议（以下全是工具名称，可以调用）：
- exec_command: 直接执行程序，command 为可执行文件名，args 为参数列表（如 command="cargo", args=["build"]）。**不支持** shell 语法（管道 |、重定向 >、&&、|| 等），也不支持 sh -c。每个调用只能执行一个命令。
- batch_read_files: 批量读取多个文件（支持 glob 模式，自动生成摘要，适合代码审查等需要读取大量文件的场景）
- restart: 修改源代码后自动运行 cargo build 并重启（仅在 dev-assistant-rs 项目自身上可用），验证修改是否编译通过
- read_file: 读取文件内容（支持 offset/limit 分块读取）
- write_file: 写入新文件（如果文件不存在）
- edit_file: 编辑现有文件（如果文件已存在，需要提供准确的 old_content）
- glob: 查找文件（如果不确定文件路径，先使用 glob）
- finish: 任务完成后调用，输出完成总结
- list_directory: 列出目录结构
- file_exists: 检查文件是否存在

技能使用说明（技能不是工具，不能直接调用；激活后按照其流程使用工具执行）：
- code-review: 代码审查技能，激活后按照其流程读取文件并输出审查报告
- 其他技能：当任务描述匹配技能触发条件时，自动激活

重要提醒：
- 每次读取文件后，记录你看到了什么
- 读取文件时，**绝不读取 target/、node_modules/、.git/ 等构建/依赖目录中的文件**，这些目录包含二进制产物，不是源代码
- 对于"审查"类任务（如代码审查、文档审查），需要：
  1. 读取关键文件
  2. 总结发现的问题或优点
  3. 使用 finish 工具输出审查报告
- 不要无限循环读取文件，应在读取足够信息后给出结论
- 修改 Rust 源代码后，使用 restart 工具自动编译验证
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
        Skill {
            meta: SkillMetadata {
                name: name.to_string(),
                description: "test".to_string(),
                when_to_use: None,
                metadata: HashMap::new(),
            },
            body: String::new(),
            source_path: PathBuf::new(),
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
        assert!(prompt.contains("/quit"), "missing /quit rule");
        assert!(prompt.contains("绝不读取 target/"), "missing build-dir guard");
        assert!(prompt.contains("rm -rf, sudo"), "missing dangerous-ops caution");
    }

    #[test]
    fn build_prompt_with_no_tools_still_valid() {
        // 空工具列表不应 panic，且仍含规则段
        let prompt = build_system_prompt(&[], &[]);
        assert!(prompt.contains("规则："));
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
