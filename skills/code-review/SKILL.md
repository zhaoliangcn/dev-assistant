---
name: code-review
description: >
  通用代码审查技能。当用户要求代码审查、代码检查、code review、
  review code、检查代码质量、安全审查时触发。支持 Rust、C/C++、JavaScript/TypeScript、
  Python、Go、Java 等常见编程语言的系统化代码审查工作流程。
when_to_use: >
  用户请求涉及代码审查、代码质量检查、安全审计、最佳实践审查、
  性能分析等任务时使用。
metadata:
  author: dev-assistant
  version: "1.1"
---

# 通用代码审查工作流程

## 审查前准备

1. 使用 `glob` 或 `list_directory` 了解项目结构
2. **绝对不要**读取 `target/`、`node_modules/`、`dist/`、`build/`、`__pycache__/`、`.git/` 等构建/依赖目录中的文件
3. 使用 `batch_read_files` **一次性**批量读取所有核心源代码文件
4. 根据项目语言识别技术栈（通过文件扩展名、配置文件等）

## 语言特定配置参考

| 语言 | 构建目录 | 包管理 | 关键配置文件 |
|------|---------|--------|------------|
| Rust | `target/` | Cargo | `Cargo.toml`, `Cargo.lock` |
| C/C++ | `build/`, `cmake-build/` | CMake/Make | `CMakeLists.txt`, `Makefile`, `*.cmake` |
| JavaScript/TS | `node_modules/`, `dist/` | npm/yarn/pnpm | `package.json`, `tsconfig.json` |
| Python | `__pycache__/`, `.venv/` | pip/poetry | `pyproject.toml`, `requirements.txt`, `setup.py` |
| Go | — | go mod | `go.mod`, `go.sum` |
| Java | `target/`, `build/` | Maven/Gradle | `pom.xml`, `build.gradle` |

## 审查要点清单

### 安全
- 路径遍历：所有用户输入路径必须经过验证（canonicalize、starts_with 检查）
- 命令注入：exec_command 应通过 shell 执行，避免直接拼接用户输入到命令行
- 敏感文件：`.env`、`.key`、`.pem`、`.crt`、密码、token 等不应被硬编码或意外提交
- 构建产物：**绝不读取构建目录中的文件**，这些是编译产物，不是源代码

### 错误处理
- 避免裸 `panic`、`assert` 在生产代码中
- 错误类型应该有意义，提供足够的上下文信息
- 所有 I/O 操作、外部调用应该有错误处理，不要吞掉原始错误

### 代码质量
- 函数/方法长度合理（建议 < 50 行）
- 避免深层嵌套（建议 < 4 层）
- 命名清晰，遵循语言惯用命名规范
- 避免重复代码（DRY 原则）
- 魔法数字应提取为常量

### 异步/并发
- 避免在异步上下文中执行阻塞操作
- 并发访问共享资源应该有适当的同步机制
- 避免死锁和竞态条件

### 性能
- 避免在热路径中不必要的分配
- 大对象使用适当的数据结构
- 注意循环中的重复计算

### API 设计
- 接口清晰，职责单一
- 参数合理（建议少于 5 个，过多应考虑结构体）
- 公共 API 有文档注释

## 输出格式

审查报告应包含：
1. 总体评价（通过 / 需改进）
2. 技术栈概述（检测到的语言、框架、构建工具）
3. 发现的问题列表（严重程度：Critical / High / Medium / Low）
4. 具体修复建议（附代码片段）
5. 代码亮点（做得好的地方）

## ⚠️ 重要规则（防止死循环）

1. **一次读完，一次审完**：使用 `batch_read_files` 一次性读取所有文件，不要分批读取、分批审查
2. **不要反复读取文件**：已经读取过的文件不要重复读取
3. **优先通过 finish 输出**：审查结果优先通过 `finish` 的 summary 输出。**如果结果太大（超过 2000 字符）**，可以写入文件（如 `code-review-report.md`）并在 finish summary 中引用文件路径
4. **不要反复编辑保存**：不要多次调用 `kb_store` 或 `write_file` 保存同一份报告
5. **审查完成立即结束**：所有文件审查完毕后，立即调用 `finish` 工具输出报告，**不要继续寻找新文件或循环**

使用 `finish` 工具输出审查报告。小型报告直接内联，大型报告写入文件后引用路径。