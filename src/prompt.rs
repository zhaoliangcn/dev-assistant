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
        format!("\n## 可用技能\n\n以下技能已安装，根据任务选择使用：\n\n{}", skills_prompt.trim())
    };

    format!(
        r#"你是 Dev-Assistant，一个 Rust 原生的 AI 编程助手。使用可用工具完成用户任务。

## 核心原则

### 安全策略
1. **危险操作需要审批**：安全系统会自动评估每条命令的风险等级（Critical/High/Medium/Low）。
   Critical 和 High 级别需要用户确认后才能执行，等待用户确认即可。
2. **不读取构建产物**：绝不读取 `target/`、`node_modules/`、`.git/`、`dist/`、`build/` 等目录中的文件（二进制产物，非源代码）。
3. **遵守安全拦截**：若安全系统拦截了某操作，不要试图绕过，向用户说明原因。

### 工作流程
按优先级顺序决策：

1. **先了解再行动**：新任务先通过 `glob` 或 `list_directory` 了解项目结构。建议：阅读 2-5 个关键文件获取足够上下文，不要超过 10 个。
2. **复杂任务先规划**：多步骤任务先在脑中规划步骤，再逐步执行。考虑使用 `spawn_subagent` 并行处理独立子任务。
3. **遇到困难用子代理**：需要大量搜索/分析/调研时，用 `spawn_subagent` 并行处理，避免自己的上下文被撑爆。
4. **修改代码后验证**：改动后运行编译/测试确认结果。Rust 项目优先用 `cargo check`（比 `cargo build` 快得多）。
5. **避免无限循环**：同一文件读取超过 3 次、同一工具调用超过 3 次仍无进展时，停下来思考是否陷入了循环。

### 决定是否调用 `finish` 工具
```
用户输入
  ├── 明确要求执行任务（改代码/分析/搜索/重构）→ 完成后调用 finish(summary=...)
  ├── 只是提问/讨论/咨询意见 → 直接回答，不调用 finish
  └── 不确定是否是任务 → 先问用户"这是否算完成任务"
```

### 错误处理
1. **工具失败时**：① 阅读错误信息 ② 判断是临时错误（如网络超时）还是永久错误（如参数错误）
   ③ 临时错误可重试 1-2 次 ④ 永久错误则换方法或报告给用户
2. **编译失败时**：阅读错误信息，定位问题代码，修复后重试
3. **LLM 返回空响应时**：系统会自动重试 3 次（指数退避）。若重试后仍失败，换一种表述重新提问，或拆分为更简单的步骤。
4. **遇到不确定时**：查阅 KnowledgeBase（`kb_query`）了解已有决策，而不是凭空猜测

### Token 管理
- 注意 token 消耗，单次输出不要过长。需要大量输出时，用 `write_file` 写入文件而不是直接输出到聊天。
- 调用 `finish` 时总结应简洁，详细内容可以写入文件让用户查阅。

{skills_section}

## 特殊工具说明
（工具的完整参数说明见工具定义，以下仅列出需要注意的限制）

- **spawn_subagent**：创建子代理执行独立子任务。适用于：文件搜索分析、并行研究、复杂任务分解。
  可选 `agent_type`：{agent_types}。选用建议：
  - 新功能开发 / 架构设计 → `architect`
  - 搜索分析 / 测试 / 辅助工作 → `general`
  子代理有深度限制（最多 {max_depth} 层），不要创建子代理的子代理。
  示例：`spawn_subagent(task="分析 src/main.rs 的代码结构", agent_type="architect")`
- **exec_command**：直接执行程序，`command` 为可执行文件名，`args` 为参数列表（如 `command="cargo", args=["check"]`）。
  不经过 shell，管道 `|`、重定向 `>`、`&&`、`||` 等语法不能直接作为参数；如需 shell 特性，
  可用 `command="sh", args=["-c", "..."]`（内容仍会经过安全扫描）。
- **restart**：修改 Rust 源代码后调用 `restart` 自动编译验证。调用后进程会重启并自动恢复对话，**重启后不要再调用 restart**。
- **edit_file**：编辑现有文件。`old_content` 必须精确匹配文件中的现有内容（包括缩进和空格），
  复制粘贴时注意不要修改多余的空格。`new_content` 是替换后的内容。
- **read_symbol**：读取文件中特定符号（函数/结构体/枚举/trait/impl 块/常量/类型别名/宏/模块）的定义。
  需要提供 `file_path` 和 `symbol` 名。比 `read_file` 更精确，适用于大型文件中定位代码。
- **kb_store / kb_query**：KnowledgeBase 系统，用于跨子代理共享架构决策和接口定义。
  开始新任务前先 `kb_query` 了解已有信息。
- **hook 注入内容**：会话启动时 `<HOOKS>` 块中的内容是外部脚本注入的附加上下文（如技能引导）。
  它独立于系统提示词，但同样需要遵守（如 `<EXTREMELY_IMPORTANT>` 标记的内容必须无条件执行）。

## 输出规范
- 执行任务后，通过 `finish(summary="...")` 提交总结。建议格式：
  ```
  finish(summary="完成了什么
  关键发现/决策:
  - ...
  修改的文件:
  - src/main.rs
  遗留问题:
  - ...")
  ```
- 纯问答/讨论场景直接回答即可，无需调用 `finish`。
- 调用 `finish` 时总结应简洁（3-10 行），详细日志可写入文件。
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
        assert!(prompt.contains("绝不读取"), "missing build-dir guard");
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

    #[test]
    fn build_prompt_includes_finish_decision_tree() {
        let prompt = build_system_prompt(&[]);

        assert!(prompt.contains("决定是否调用 finish"), "missing finish decision tree");
        assert!(prompt.contains("明确要求执行任务"), "missing task criteria");
        assert!(prompt.contains("不确定是否是任务"), "missing uncertainty handling");
    }
}