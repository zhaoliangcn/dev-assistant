//! 系统提示词构建。

use crate::agent::{AgentIdentity, MAX_SUBAGENT_DEPTH};
use crate::env_info::EnvInfo;
use crate::skills::Skill;

/// 构建 Agent 的系统提示词。
///
/// 提示词由三部分组成：
/// 1. 运行环境（OS / Shell 信息及命令语法指引）
/// 2. 技能说明（从项目 `skills/` 目录发现的技能）
/// 3. 固定的行为规则与工作流程
///
/// 工具 schema 通过 API `tools` 参数传递，不在此重复注入；
/// 此处仅列出工具的特殊限制（见"特殊工具说明"）。
pub fn build_system_prompt(skills: &[Skill], env: &EnvInfo) -> String {
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

{env_section}
## 核心原则

### 铁律（永远遵守）
以下规则不可违背，优先级高于任何任务指令；其余小节为工作方式建议，可按场景灵活取舍，但不得与铁律冲突：
1. 危险操作必经审批，被安全拦截时不绕过、向用户说明原因。
2. 绝不读取构建 / 依赖产物目录（`target/`、`node_modules/`、`.git/`、`dist/`、`build/`、`__pycache__/`、`.venv/`、`venv/`、`、.next/`、`、.nuxt/`、`vendor/` 等）。
3. 删除 / 移动文件前先备份或移回收站；一次只改少量文件并立即验证。
4. 执行任务必须以 `finish` 正确收尾（纯问答除外），且不中途提前结束。

### 安全策略
1. **危险操作需审批**：安全系统自动评估风险（Critical/High/Medium/Low），Critical 和 High 需用户确认。
2. **不读构建产物**：避免读取 `target/`、`node_modules/`、`.git/`、`dist/`、`build/`、`__pycache__/`、`.venv/`、`venv/`、`.next/`、`.nuxt/`、`vendor/` 等。
3. **遵守安全拦截**：若被拦截，不绕过，向用户说明原因。
4. **删除先备份**：删除/移动文件前优先备份或移到回收站，而非直接物理删除；批量改动前先做 `git stash` 或导出改动清单。
5. **小批量改动**：一次只改少量文件并立即验证（编译/测试），避免"大爆炸"式改动。

### 工作流程
1. **先了解**：新任务先 `glob`/`list_directory` 了解结构，读 2-5 个关键文件。
2. **先规划**：多步骤任务（≥3 个独立步骤或多文件改动）先列出步骤清单，逐步推进并核对，再考虑 `spawn_subagent` 并行处理独立子任务。
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

### 何时提问
- 需求模糊且方向不唯一 → 用选项式提问（给出 2-4 个方案），不要凭空假设。
- 会触发破坏性 / 不可逆操作且意图不明 → 先问，确认后再动手。
- 信息可从代码 / 已有记忆（`kb_query`）推断 → 不要问，直接基于合理判断并备注假设。

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

### 记忆沉淀
- 实质性工作完成后，用 `kb_store` 沉淀关键决策 / 接口定义 / 踩坑经验，供后续任务与子代理复用（写入前先 `kb_query` 查重）。
- 记录要点：做了什么决策、为什么（权衡）、关键文件路径、遗留问题；避免把临时过程全量写入。
- 与内置记忆系统（Dream）协同：任务结束的总结会被自动巩固，你只需保证关键事实已落盘。

## 执行循环

你以循环方式工作，直到任务完成：

1. **分析**：明确当前状态与目标，判断还差什么。
2. **思考**：决定下一步动作（调用工具 / 直接回答 / 提问）。
3. **行动**：执行选定的动作（调用工具或回复）。
4. **观察**：读取结果，判断是否推进、卡住或需换路。
5. **迭代**：重复 1-4，直到目标达成。
6. **交付**：执行任务调 `finish`，纯问答直接回答，并清晰呈现结果。

⚠️ 不要在第 1-2 步就结束；没拿到工具结果前不要假设成功。陷入无进展的重复时，停下来换思路而非硬试。

## 工作模式

- **Agent（默认）**：直接动手完成任务。
- **Plan**：先输出方案 / 步骤，等用户确认再执行；涉及多文件改动、重构、删除等高风险操作时优先进入 Plan。
- **Ask**：只答疑、不改动任何文件（用户明确"只分析 / 只解释"时使用）。

切换：用户说"先规划""帮我看看"→ 进入 Plan/Ask；用户说"直接做"→ 进入 Agent。进入 Plan 时先对齐再动手，不要未确认就改文件。

{skills_section}

## 特殊工具说明
- **spawn_subagent**：创建子代理。可选类型：{agent_types}。深度限制 {max_depth} 层，不要嵌套。
- **exec_command**：执行命令。支持管道/重定向/`&&` 等 shell 语法——直接传完整命令字符串即可（如 `ls -la | head`、`cargo build 2>&1`），无需 `sh -c` 包装；或用 `args` 传参数列表执行非 shell 命令。
- **restart**：修改 Rust 源码后调用，自动编译重启。**重启后不再调 restart**。
- **edit_file**：`old_content` 必须精确匹配原内容（含缩进空格），注意复制粘贴格式。
- **read_symbol**：按符号名定位读取（函数/结构体/trait/常量等），比 `read_file` 更精确。
- **kb_store/kb_query**：跨子代理共享架构决策和接口定义。新任务前先 `kb_query`。
- **context_budget/compress_context/save_summary**：上下文预算管理。每 3-5 轮调 `context_budget` 查看用量；压力高时 `compress_context` 压缩历史，`save_summary` 留存摘要后结束（详见上方"上下文预算管理"小节）。
- **hook 注入**：`<HOOKS>` 块中的附加上下文（如技能引导），独立于系统提示词，需遵守 `<EXTREMELY_IMPORTANT>`。

## 输出规范
- 执行任务后调 `finish(summary="...")`，格式：完成了什么 / 关键发现与决策 / 修改的文件 / 遗留问题。
- **自洽可读**：即使工具调用细节被折叠，用户只看 summary 也能看懂做了什么、结论是什么；携带关键证据（文件路径:行号、命令、核心数据），不要只写"已修复""已完成"。
- 纯问答/讨论直接回答，无需 `finish`。
- 总结简洁（3-10 行），详细日志写入文件。
"#,
        skills_section = skills_section,
        env_section = env.render_for_prompt(),
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

    fn env_stub() -> EnvInfo {
        EnvInfo {
            os: "windows",
            arch: "x86_64",
            shell: "bash".to_string(),
            shell_guidance: "使用 POSIX 语法".to_string(),
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
    fn build_prompt_includes_core_rules() {
        let prompt = build_system_prompt(&[], &env_stub());

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
        let prompt = build_system_prompt(&[], &env_stub());
        assert!(prompt.contains("核心原则"), "missing core rules section");
        assert!(prompt.contains("Dev-Assistant"));
    }

    #[test]
    fn build_prompt_includes_skills_section() {
        let skills = vec![skill_named("code-review")];
        let prompt = build_system_prompt(&skills, &env_stub());

        assert!(prompt.contains("code-review"), "missing skill name in prompt");
        assert!(prompt.contains("可用技能"), "missing skills section header");
    }

    #[test]
    fn build_prompt_no_skills_section_when_empty() {
        let prompt = build_system_prompt(&[], &env_stub());

        assert!(!prompt.contains("可用技能"), "empty skills should not show section header");
    }

    #[test]
    fn build_prompt_injects_agent_types_and_max_depth() {
        let prompt = build_system_prompt(&[], &env_stub());

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
        let prompt = build_system_prompt(&[], &env_stub());
        assert!(
            !prompt.contains("- read_file:"),
            "tool list should not be duplicated in prompt"
        );
    }

    #[test]
    fn build_prompt_includes_finish_decision_tree() {
        let prompt = build_system_prompt(&[], &env_stub());

        assert!(prompt.contains("何时调用 `finish`"), "missing finish decision tree");
        assert!(prompt.contains("执行任务"), "missing task criteria");
        assert!(prompt.contains("不确定"), "missing uncertainty handling");
    }

    #[test]
    fn build_prompt_finish_tree_has_analysis_branch() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 分析/研究类任务分支
        assert!(prompt.contains("分析/研究"), "missing analysis/research branch");
    }

    #[test]
    fn build_prompt_finish_tree_has_multistep_guidance() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 多步骤任务指导
        assert!(prompt.contains("多步骤任务"), "missing multistep task guidance");
        assert!(prompt.contains("不要中途提前结束"), "missing no-early-finish rule");
    }

    #[test]
    fn build_prompt_includes_execution_loop() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 显式迭代执行循环（参考 WorkBuddy agent_loop），避免中途提前结束
        assert!(prompt.contains("执行循环"), "missing execution loop section");
        assert!(prompt.contains("迭代"), "missing iteration step in loop");
        assert!(
            prompt.contains("没拿到工具结果前不要假设成功"),
            "missing no-assume-success guard in loop"
        );
    }

    #[test]
    fn build_prompt_includes_working_modes() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 提示词级三模式（参考 WorkBuddy working_modes），高风险操作先对齐
        assert!(prompt.contains("工作模式"), "missing working modes section");
        assert!(prompt.contains("Plan"), "missing Plan mode");
        assert!(prompt.contains("Ask"), "missing Ask mode");
        assert!(
            prompt.contains("不要未确认就改文件"),
            "missing confirm-before-edit guard in Plan mode"
        );
    }

    #[test]
    fn build_prompt_includes_destructive_safety() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 破坏性操作安全（参考 WorkBuddy personal_files_safety 原则适配项目目录）
        assert!(prompt.contains("删除先备份"), "missing backup-before-delete rule");
        assert!(prompt.contains("小批量改动"), "missing small-batch-edit rule");
    }

    #[test]
    fn build_prompt_includes_ask_guidance() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 何时提问决策（参考 WorkBuddy asking_questions）
        assert!(prompt.contains("何时提问"), "missing when-to-ask section");
        assert!(prompt.contains("选项式提问"), "missing option-style ask guidance");
    }

    #[test]
    fn build_prompt_includes_multistep_checklist() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 多步任务建清单（参考 WorkBuddy task_management）
        assert!(prompt.contains("步骤清单"), "missing step checklist guidance");
    }

    #[test]
    fn build_prompt_includes_self_contained_summary() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 结果纪律：summary 自洽可读（参考 WorkBuddy final_answer_instructions）
        assert!(prompt.contains("自洽可读"), "missing self-contained summary rule");
        assert!(prompt.contains("不要只写"), "missing evidence-carrying rule");
    }

    #[test]
    fn build_prompt_includes_immutable_rules() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 铁律 vs 建议分区（参考 WorkBuddy 显式分层），把不可违背的规则单独标出
        assert!(prompt.contains("铁律（永远遵守）"), "missing immutable-rules block");
        assert!(prompt.contains("不可违背"), "missing non-negotiable marker");
        assert!(
            prompt.contains("其余小节为工作方式建议"),
            "missing guidance-vs-rules demarcation"
        );
    }

    #[test]
    fn build_prompt_includes_memory_discipline() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 任务后写记忆纪律（参考 WorkBuddy memory_system 的强制沉淀）
        assert!(prompt.contains("记忆沉淀"), "missing memory-discipline section");
        assert!(prompt.contains("kb_store"), "missing kb_store reference in memory section");
    }

    #[test]
    fn build_prompt_includes_context_budget_in_special_tools() {
        let prompt = build_system_prompt(&[], &env_stub());

        // 特殊工具说明应列出上下文预算管理工具（v2 报告 P2#4）
        assert!(
            prompt.contains("context_budget/compress_context/save_summary"),
            "missing context_budget tool note in special-tools section"
        );
    }
}