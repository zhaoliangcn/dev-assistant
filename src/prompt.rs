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

    let skills_section = if skills_prompt.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n## 可用技能\n\n以下技能已安装。当用户请求匹配技能关键词时会自动激活，技能内容将附加到输入中。根据任务选择使用：\n\n{}",
            skills_prompt.trim()
        )
    };

    format!(
        r#"你是 Dev-Assistant，一个 Rust 原生的 AI 编程助手。使用可用工具完成用户任务。

## 核心原则

### 安全策略
1. **危险操作需审批**：安全系统自动评估风险（Critical/High/Medium/Low），Critical 和 High 需用户确认。
2. **不读构建产物**：避免读取 `target/`、`node_modules/`、`.git/`、`dist/`、`build/`、`__pycache__/`、`.venv/`、`venv/`、`.next/`、`.nuxt/`、`vendor/` 等。
3. **遵守安全拦截**：若被拦截，不绕过，向用户说明原因。

### 工作流程
1. **先了解**：新任务先 `glob`/`list_directory` 了解结构，读 2-5 个关键文件。
2. **先规划**：多步骤任务先规划再执行，考虑 `spawn_subagent` 并行处理独立子任务。
3. **用子代理解耦**：大量搜索/分析时用 `spawn_subagent` 避免撑爆上下文。
4. **改后验证**：改动后编译/测试确认（Rust 优先 `cargo check`）。
5. **防循环**：同一文件/工具调用超 3 次无进展时，停下来思考。

### 何时调用 `finish`
- ❓ **提问/闲聊**：直接回答，**不调 finish**。
- ✅ **执行任务**（改代码/搜索/分析/审查/调试）：完成后调 finish。
- ✅ **多步骤任务**：全部完成再调 finish，**不要中途提前结束**。
- 📊 **分析/研究**：输出报告或口头解释后调 `finish(summary="...")`。
- ❓ **不确定**：问用户"是否算完成任务"。

**关键**：分析/研究类属于执行任务，完成后必须调 finish。总结简洁（3-10 行），详细日志写入文件。

### 错误处理
1. **工具失败**：读错误信息，临时错误重试 1-2 次，永久错误换方法或报告。
2. **编译失败**：读错误，定位修复后重试。
3. **LLM 空响应**：系统自动重试 3 次（指数退避），仍失败则换表述或拆解步骤。
4. **不确定时**：`kb_query` 查阅已有决策，不凭空猜测。

### Token 管理
- 大量输出用 `write_file` 写入文件而非直接输出到聊天。
- `finish` 总结应简洁。

### 上下文预算管理
每 3-5 轮工具调用后检查 `context_budget`。压力等级：
- ✅ Normal (<60%)：正常执行
- ⚠️ Warning (60-80%)：考虑压缩，`kb_store` 保存关键信息
- 🔴 Critical (80-90%)：主动 `compress_context`
- 🚨 Exhausted (>90%)：`save_summary` 后 `finish` 结束

{skills_section}

## 特殊工具说明
- **spawn_subagent**：创建子代理。可选类型：{agent_types}。深度限制 {max_depth} 层，不要嵌套。
- **exec_command**：`command` 为可执行文件，`args` 为参数列表。不经过 shell，管道/重定向需 `sh -c "..."`。
- **restart**：修改 Rust 源码后调用，自动编译重启。**重启后不再调 restart**。
- **edit_file**：`old_content` 必须精确匹配原内容（含缩进空格），注意复制粘贴格式。
- **read_symbol**：按符号名定位读取（函数/结构体/trait/常量等），比 `read_file` 更精确。
- **kb_store/kb_query**：跨子代理共享架构决策和接口定义。新任务前先 `kb_query`。
- **hook 注入**：`<HOOKS>` 块中的附加上下文（如技能引导），独立于系统提示词，需遵守 `<EXTREMELY_IMPORTANT>`。

## 输出规范
- 执行任务后调 `finish(summary="...")`，格式：完成了什么 / 关键发现与决策 / 修改的文件 / 遗留问题。
- 纯问答/讨论直接回答，无需 `finish`。
- 总结简洁（3-10 行），详细日志写入文件。
"#,
        skills_section = skills_section,
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
        assert!(prompt.contains("不读构建产物"), "missing build-dir guard");
        assert!(prompt.contains("Dev-Assistant"), "missing brand identity");
        assert!(prompt.contains("spawn_subagent"), "missing spawn_subagent tool description");
        assert!(prompt.contains("核心原则"), "missing core rules section");
        assert!(prompt.contains("输出规范"), "missing output spec section");
        assert!(prompt.contains("read_symbol"), "missing read_symbol tool description");
        assert!(prompt.contains("hook 注入"), "missing hook injection note");
        assert!(prompt.contains("Token 管理"), "missing token management section");
        assert!(prompt.contains("cargo check"), "missing cargo check preference");
    }

    #[test]
    fn build_prompt_with_no_skills_still_valid() {
        // 空技能列表不应 panic，且仍含规则段
        let prompt = build_system_prompt(&[]);
        assert!(prompt.contains("核心原则"), "missing core rules section");
        assert!(prompt.contains("Dev-Assistant"));
    }

    #[test]
    fn build_prompt_includes_skills_section() {
        let skills = vec![skill_named("code-review")];
        let prompt = build_system_prompt(&skills);

        assert!(prompt.contains("code-review"), "missing skill name in prompt");
        assert!(prompt.contains("可用技能"), "missing skills section header");
    }

    #[test]
    fn build_prompt_no_skills_section_when_empty() {
        let prompt = build_system_prompt(&[]);

        assert!(!prompt.contains("可用技能"), "empty skills should not show section header");
    }

    #[test]
    fn build_prompt_injects_agent_types_and_max_depth() {
        let prompt = build_system_prompt(&[]);

        // agent_type 列表应从 AgentIdentity 枚举动态生成，含中文标签
        assert!(prompt.contains("architect（架构师）"), "missing architect label");
        assert!(prompt.contains("general（通用代理）"), "missing general label");
        // 深度限制应注入真实常量
        assert!(
            prompt.contains(&format!("{} 层", MAX_SUBAGENT_DEPTH)),
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

    #[test]
    fn build_prompt_includes_finish_decision_tree() {
        let prompt = build_system_prompt(&[]);

        assert!(prompt.contains("何时调用 `finish`"), "missing finish decision tree");
        assert!(prompt.contains("执行任务"), "missing task criteria");
        assert!(prompt.contains("不确定"), "missing uncertainty handling");
    }

    #[test]
    fn build_prompt_finish_tree_has_analysis_branch() {
        let prompt = build_system_prompt(&[]);

        // 分析/研究类任务分支
        assert!(prompt.contains("分析/研究"), "missing analysis/research branch");
    }

    #[test]
    fn build_prompt_finish_tree_has_multistep_guidance() {
        let prompt = build_system_prompt(&[]);

        // 多步骤任务指导
        assert!(prompt.contains("多步骤任务"), "missing multistep task guidance");
        assert!(prompt.contains("不要中途提前结束"), "missing no-early-finish rule");
    }
}