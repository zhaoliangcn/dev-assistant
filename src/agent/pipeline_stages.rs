use super::identity::AgentIdentity;

/// 6 个流水线阶段的配置常量数组。
///
/// 每个条目包含：
/// - 阶段名称
/// - 子代理身份
/// - 任务模板（含 `{task_ref}`、`{context}`、`{finish}` 占位符）
/// - 输出描述（用于 finish 提示）
/// - 示例摘要（用于 finish 提示）
///
/// 模板顺序与 `STAGE_WEIGHTS` 权重数组一一对应：
/// 设计(0) → 编码(1) → 测试(2) → 审查(3) → 修复(4) → 记录(5)
pub const STAGE_TEMPLATES: [(&'static str, AgentIdentity, &'static str, &'static str, &'static str); 6] = [
    (
        "🏗 架构设计",
        AgentIdentity::Architect,
        "这是流水线第一阶段：架构设计。\n\n\
         原始需求：\n\
         {task_ref}\n\n\
         {context}\n\n\
         请用 kb_store 将架构决策保存到 pipeline/stage-0/。\n\n\
         {finish}",
        "架构设计",
        "架构设计完成：模块划分、接口定义、数据流、决策理由",
    ),
    (
        "💻 代码实现",
        AgentIdentity::Implementer,
        "这是流水线第二阶段：代码实现。\n\n\
         原始需求：\n\
         {task_ref}\n\n\
         上一阶段输出（架构设计）：\n\
         {context}\n\n\
         请用 kb_store 将修改的文件列表保存到 pipeline/stage-1/。\n\n\
         {finish}",
        "代码实现",
        "代码实现完成：新增文件、关键接口、测试覆盖",
    ),
    (
        "🧪 测试验证",
        AgentIdentity::Tester,
        "这是流水线第三阶段：测试验证。\n\n\
         原始需求：\n\
         {task_ref}\n\n\
         上一阶段输出（代码实现）：\n\
         {context}\n\n\
         请用 kb_store 将测试结果保存到 pipeline/stage-2/。\n\n\
         {finish}",
        "测试报告",
        "测试完成：N 个测试用例，M 个通过，K 个失败",
    ),
    (
        "🔍 代码审查",
        AgentIdentity::Reviewer,
        "这是流水线第四阶段：代码审查。\n\n\
         原始需求：\n\
         {task_ref}\n\n\
         上一阶段输出（测试结果和代码实现）：\n\
         {context}\n\n\
         请用 kb_store 将审查结果保存到 pipeline/stage-3/。\n\n\
         {finish}",
        "审查报告",
        "审查完成：发现 N 个问题，其中严重 X 个，建议 Y 项",
    ),
    (
        "🔧 问题修复",
        AgentIdentity::Debugger,
        "这是流水线第五阶段：问题修复。\n\n\
         原始需求：\n\
         {task_ref}\n\n\
         上一阶段输出（审查报告）：\n\
         {context}\n\n\
         注意：只修复审查中提出的问题，不要引入新的功能变更。\n\
         请用 kb_store 将修复记录保存到 pipeline/stage-4/。\n\n\
         {finish}",
        "修复",
        "修复完成：处理 N 个问题，编译通过",
    ),
    (
        "📋 进度记录",
        AgentIdentity::General,
        "这是流水线最终阶段：进度记录。\n\n\
         原始需求：\n\
         {task_ref}\n\n\
         已完成的工作：\n\
         {context}\n\n\
         请用 kb_store 记录到 pipeline/stage-5/：\n\
         1. 完成的功能列表\n\
         2. 修改的文件清单\n\
         3. 测试结果概要\n\
         4. 未解决的问题\n\n\
         如有代码变更，尝试 `exec_command` 执行 git add 和 git commit。\n\
         若 git 不可用，记录到 KB 即可。\n\n\
         {finish}",
        "进度记录",
        "进度已记录：功能列表、文件清单、测试结果",
    ),
];
