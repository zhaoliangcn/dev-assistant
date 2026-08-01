# 技能导入系统设计文档

> 状态：设计中 | 创建日期：2026-07-26

## 一、背景

当前 dev-assistant 的技能系统为静态文件目录模式，仅支持从 `<cwd>/skills/` 读取。
用户无法便捷地安装社区或团队共享的技能包，限制了我们与 [npx skills](https://github.com/vercel-labs/agent-skills) 生态的互通性。

参考：[npx skills 生态分析](https://cloud.tencent.com/developer/article/2656427)

## 二、目标

1. 支持从 Git 仓库、本地目录导入技能
2. 与 npx skills 格式完全兼容（SKILL.md + frontmatter）
3. 提供全局和项目两级安装范围
4. 在 REPL 和 CLI 中提供便捷操作命令

## 三、非目标

- 不支持 npm 包（npx 方式）作为运行时依赖，仅在安装时支持 tarball 下载
- 不实现 npx skills CLI 的全部命令（find、check 等），只保留核心 install/remove/list
- 不支持技能间依赖声明（留作后续版本）

## 四、SKILL.md 格式

与 npx skills 完全兼容，字段定义：

```yaml
---
name: my-skill          # 必填：小写字母、数字、连字符
description: 简短描述   # 必填：说明适用场景
when_to_use: 触发关键词 # 可选：逗号分隔的关键词
author: someuser        # 可选
version: 1.0.0          # 可选
source: https://...     # 可选：来源追踪，用于 update 命令
---

## 正文内容
...
```

## 五、安装源格式

| 格式 | 示例 | 说明 |
|------|------|------|
| Git 仓库（完整 URL） | `https://github.com/user/skills` | clone 整个仓库 |
| Git 仓库（owner/repo） | `vercel-labs/agent-skills` | 展开为完整 URL |
| Git 仓库（SSH） | `git@github.com:user/skills.git` | SSH 协议 |
| 本地目录 | `./my-local-skills` 或 `/abs/path` | 直接复制 |
| Git 分支/子路径 | `owner/repo:branch/path/to/skills` | 可选分支和子目录 |

## 六、安装目标路径

| 范围 | 路径 | 用途 |
|------|------|------|
| 项目级（默认） | `./.dev-assistant/skills/` | 随项目提交，团队共享 |
| 全局级 | `~/.dev-assistant/skills/` | 所有项目可用 |

## 七、命令设计

### CLI 命令

```bash
# 安装技能（默认项目级）
dev-assistant skill add <source> [--skill <name>] [--global]

# 示例
dev-assistant skill add vercel-labs/agent-skills --skill frontend-design -g
dev-assistant skill add https://github.com/user/skills --skill my-review
dev-assistant skill add ./my-local-skills
dev-assistant skill add vercel-labs/agent-skills --skill frontend-design --skill skill-creator

# 列出已安装技能
dev-assistant skill list [--global]

# 移除技能
dev-assistant skill remove <name> [--global]

# 更新技能（仅 Git 来源）
dev-assistant skill update [--global]
```

### REPL 斜杠命令

```
/skill add <source> [--skill <name>] [--global]
/skill list [--global]
/skill remove <name>
/skill update [--global]
```

## 八、架构设计

### 模块划分

```
src/skills/
├── mod.rs          # 现有：Skill 结构体、parse_skill_file、discover_skills
└── installer.rs    # 新建：安装/卸载/更新逻辑

src/utils/
└── git.rs          # 新建：clone_repo、fetch_refs 等辅助函数
```

### 安装流程

```
install_skill(source, target, options)
├── 1. 解析 source 类型（Git / 本地）
├── 2. 获取技能列表（source 仓库中的所有 SKILL.md）
│   ├── Git: clone 到临时目录 → discover_skills()
│   └── 本地: discover_skills()
├── 3. 过滤 --skill 参数指定的技能（可选）
├── 4. 校验目标位置无同名冲突（或提示覆盖）
├── 5. 复制到目标目录
│   ├── Git 来源: 复制 + 记录 git_url / branch 到 .skill-meta.json
│   └── 本地来源: 复制 + 记录 source_path 到 .skill-meta.json
└── 6. 返回已安装的技能列表
```

### .skill-meta.json 格式

每个安装的技能目录内包含元数据文件：

```json
{
  "source": "git",
  "git_url": "https://github.com/user/skills",
  "git_branch": "main",
  "installed_at": "2026-07-26T08:00:00Z",
  "version": "1.0.0"
}
```

### 更新流程

```
update_skills(global)
├── 1. 扫描目标目录，读取每个技能的 .skill-meta.json
├── 2. 筛选 Git 来源的技能（本地来源跳过）
├── 3. 对每个 Git 技能：
│   ├── git fetch origin <branch>
│   ├── git log HEAD..origin/<branch> 检查是否有新 commit
│   └── 若有更新：git reset --hard origin/<branch> + discover + 返回差异
└── 4. 返回更新列表
```

## 九、实现计划

### Phase 1 — 核心安装逻辑

- [ ] `src/skills/installer.rs`：`install_skill()`、`remove_skill()`
- [ ] `src/utils/git.rs`：`clone_repo()` 辅助函数
- [ ] `discover_skills()` 支持从全局和项目两级目录合并
- [ ] 单元测试：解析 Git URL、复制目录、冲突检测

### Phase 2 — CLI 集成

- [ ] `src/main.rs`：新增 `skill` 子命令
- [ ] `InstallCommand` / `ListCommand` / `RemoveCommand` / `UpdateCommand`
- [ ] 错误处理：网络失败、权限不足、磁盘空间等

### Phase 3 — REPL 集成

- [ ] `src/repl.rs`：新增 `/skill` slash 命令
- [ ] 输出格式化：技能名称、来源、安装路径
- [ ] 全局/项目范围提示

### Phase 4 — 增强功能

- [ ] `skill update` 增量更新（仅更新有变更的技能）
- [ ] `skill list` 显示来源和版本信息
- [ ] 全局技能与项目技能的合并优先级规则
- [ ] 从 tar.gz URL 安装（可选，后续）

## 十、注意事项

1. **Git clone 性能**：首次 clone 可能较慢，建议使用浅克隆 `--depth=1`
2. **冲突处理**：同名技能已存在时，默认覆盖并打印警告
3. **全局路径**：`~/.dev-assistant/` 需在用户 home 目录下创建，注意权限
4. **SKILL.md 解析**：复用现有 `parse_skill_file()`，无需修改格式
5. **离线场景**：Git 来源依赖网络，本地来源不受影响
