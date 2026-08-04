//! 系统提示词构建。

use crate::agent::{AgentIdentity, MAX_SUBAGENT_DEPTH};
use crate::skills::Skill;

/// 构建 Agent 的系统提示词。
///
/// 提示词由两部分组成：
/// 1. 技能说明（从项目 `skills/` 目录发现的技能）
/// 2. 固定的行为规则与工作流程
///
/// 工具 schema 通过 API `tools` 参数传递，不在此重复注入；
/// 此处仅列出工具的特殊限制（见"特殊工具说明"）。
pub fn build_system_prompt(skills: &[Skill]) -> String {
    let skills_prompt = crate::skills::format_skills_for_prompt(skills);
    let agent_types = AgentIdentity::all()
        .iter()
        .map(|id| format!("{}（{}）", id.to_str(), id.label()))
        .collect::<Vec<_>>()
        .join("、");

    format!(
        r#"你是一个软件工程师助手。使用可用工具完成用户任务。

{skills}

## 核心规则

### 安全约束
1. **危险操作要谨慎**：对 `rm -rf`、`sudo`、`dd` 等破坏性命令须三思，确认后再执行
2. **不读取构建产物目录**：绝不读取 `target/`、`node_modules/`、`.git/`、`dist/`、`build/` 等目录中的文件（二进制产物，非源代码）
3. **遵守安全策略**：工具执行时会自动进行安全评估，若提示危险等级为 Critical/High，请听从拦截结果

### 工作流程
1. **先了解再行动**：新任务先通过 `glob` 或 `list_directory` 了解项目结构，再进行操作
2. **复杂任务先规划**：对于多步骤任务，先制定计划再逐步执行。考虑使用 `spawn_subagent` 并行处理独立子任务
3. **避免无限循环**：读取足够信息后给出结论，不要反复读取同一批文件
4. **修改代码后及时验证**：改动后运行编译/测试确认结果
5. **完成任务必须调用 `finish`**：完成后调用 `finish(summary="...")` 工具结束，提供任务完成总结。不要仅输出文本而不调用 finish

### 错误处理
1. **工具失败时**：① 阅读错误信息 ② 判断是临时错误（如网络超时）还是永久错误（如参数错误） ③ 临时错误可重试 1-2 次 ④ 永久错误则换方法或报告给用户
2. **编译失败时**：阅读错误信息，定位问题代码，修复后重试
3. **遇到不确定时**：查阅 KnowledgeBase（`kb_query`）了解已有决策，而不是凭空猜测

## 特殊工具说明
（工具的完整参数说明见工具定义，以下仅列出需要注意的限制）

- **spawn_subagent**：创建子代理执行独立子任务。适用于：文件搜索分析、并行研究、复杂任务分解。可选 `agent_type`：{agent_types}。子代理有深度限制（最多 {max_depth} 层），不要创建子代理的子代理
  示例：`spawn_subagent(task="分析 src/main.rs 的代码结构", agent_type="architect")`
- **exec_command**：直接执行程序，`command` 为可执行文件名，`args` 为参数列表（如 `command="cargo", args=["build"]`）。不经过 shell，管道 `|`、重定向 `>`、`&&`、`||` 等语法不能直接作为参数；如需 shell 特性，可用 `command="sh", args=["-c", "..."]`（内容仍会经过安全扫描）
- **restart**：修改 Rust 源代码后调用 `restart` 自动编译验证。调用后进程会重启并自动恢复对话，**重启后不要再调用 restart**
- **edit_file**：编辑现有文件，需要提供 `old_content`（文件中准确的旧内容）和 `new_content`
- **kb_store / kb_query**：KnowledgeBase 系统用于跨子代理共享架构决策、接口定义、问题追踪。开始新任务前先 `kb_query` 了解已有信息

## 输出规范
- 执行任务时，通过 `finish(summary="...")` 提交结构化总结：① 完成了什么 ② 关键发现/决策 ③ 修改的文件列表（如有）④ 遗留问题（如有）
- 保持输出简洁，聚焦关键信息
- 纯问答/讨论场景（用户只是提问、未要求执行任务）直接回答即可，无需调用 `finish`
"#,
        skills = skills_prompt.trim(),
        agent_types = agent_types,
        max_depth = MAX_SUBAGENT_DEPTH
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillMetadata};
    use std::collections::HashMap;
    use std::path::PathBuf;

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
    fn build_prompt_includes_core_rules() {
        let prompt = build_system_prompt(&[]);

        // 固定行为规则不应漏
        assert!(prompt.contains("finish"), "missing finish rule");
        assert!(prompt.contains("绝不读取"), "missing build-dir guard");
        assert!(prompt.contains("rm -rf"), "missing dangerous-ops caution");
        assert!(prompt.contains("spawn_subagent"), "missing spawn_subagent tool description");
        assert!(prompt.contains("核心规则"), "missing core rules section");
        assert!(prompt.contains("输出规范"), "missing output spec section");
    }

    #[test]
    fn build_prompt_with_no_skills_still_valid() {
        // 空技能列表不应 panic，且仍含规则段
        let prompt = build_system_prompt(&[]);
        assert!(prompt.contains("核心规则"), "missing core rules section");
        assert!(prompt.contains("你是一个软件工程师助手"));
    }

    #[test]
    fn build_prompt_includes_skills_section() {
        let skills = vec![skill_named("code-review")];
        let prompt = build_system_prompt(&skills);

        assert!(prompt.contains("code-review"), "missing skill name in prompt");
    }

    #[test]
    fn build_prompt_injects_agent_types_and_max_depth() {
        let prompt = build_system_prompt(&[]);

        // agent_type 列表应从 AgentIdentity 枚举动态生成，含中文标签
        assert!(prompt.contains("architect（架构师）"), "missing architect label");
        assert!(prompt.contains("general（通用代理）"), "missing general label");
        // 深度限制应注入真实常量
        assert!(
            prompt.contains(&format!("最多 {} 层", MAX_SUBAGENT_DEPTH)),
            "max depth not injected"
        );
    }

    #[test]
    fn build_prompt_does_not_duplicate_tool_list() {
        // 工具 schema 已通过 API tools 参数传递，提示词中不应重复注入工具清单
        let prompt = build_system_prompt(&[]);
        assert!(
            !prompt.contains("- read_file:"),
            "tool list should not be duplicated in prompt"
        );
    }
}
