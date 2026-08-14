# 编码规范

> 本文件由 desk 改造工具自动生成，用于约束 AI 辅助编程（Cursor / Copilot / Cline / Claude 等）的输出质量。可在项目演进中持续修订。

## 语言要求

- 所有回答、代码说明、Git Commit Message 默认使用简体中文。
- 除非用户明确要求英文，否则不要生成英文说明。
- 技术名词、类名、方法名、字段名、框架名保持英文原文。

---

# SOURCE FILE ENCODING

- All source files (.java, .vue, .ts, .js, .xml, .sql, .md, .yml, .properties, etc.) MUST be UTF-8.
- 禁止使用 GBK / ANSI 保存源码文件。
- 中文注释、中文字符串必须正常显示。
- 禁止提交乱码内容，例如：`鍐呭規爣绛` / `????` / `口口口`。
- 修改文件前，如果发现原文件存在编码异常，必须优先提示并修复编码问题。

---

# GIT COMMIT MESSAGE RULES

IMPORTANT: These rules apply to the "Generate with AI" in the Git panel.

- Git commit message 必须使用简体中文，不要使用英文描述。
- 使用 Conventional Commits 格式：`type:中文描述`（冒号后不要加空格）。

可用 type：

- `feat:` 新功能
- `fix:` 修补错误
- `docs:` 文档改变
- `style:` 代码格式改变
- `refactor:` 已有功能重构
- `perf:` 性能优化
- `test:` 增加测试
- `build:` 改变构建流程
- `ci:` 改变持续集成配置
- `chore:` 辅助工具的变动

示例：

- `feat:完善用户登录逻辑`
- `fix:解决诗词详情页加载异常`
- `docs:更新README文件`
- `refactor:优化任务管理页面结构`
- `perf:优化批量查询性能`

<!-- SUB_AGENTS_RULES_BEGIN -->
# 子智能体协作与调度规则

## 基本原则

主 Agent 负责理解用户目标、维护整体任务上下文、协调子任务和最终汇总。

对于能够独立完成的代码探索、架构分析、开发、代码审查、数据库审查和 UI/UX 审查任务，应优先考虑委派给对应子智能体，避免所有工作都由主 Agent 在同一上下文中完成。

子智能体完成任务后，由主 Agent 结合其结果继续推进当前任务。

不要为了使用子智能体而机械拆分简单任务。
对于非常简单、局部、明确且无需独立上下文的修改，可以直接处理。

---

## architect

当需求涉及新功能、复杂业务、多个模块、架构调整或较大范围修改，需要分析现有实现、影响范围和制定实施方案时优先调用。负责架构分析、业务方案、技术设计和风险识别，只分析不修改代码。

---

## code-reviewer

当功能开发、代码修改或 Bug 修复完成后优先调用，独立审查本次代码变更。用户要求自检、检查、代码审查、Review、交付检查时应优先调用本智能体。负责检查业务逻辑、权限、安全、事务、数据一致性、异常处理、边界条件和潜在回归，只审查不修改代码。

---

## database-reviewer

当需求涉及数据库表结构、字段、索引、SQL 升级、数据迁移、查询性能或数据库风险时调用。负责数据库和 SQL 专项审查，检查结构设计、索引、约束、升级兼容性和数据一致性，只分析不修改。

---

## fullstack-developer

当任务需要实际新增、修改、完善或修复代码时优先调用。负责前端、后端、小程序、数据库、桌面端及其他语言项目的全栈功能实现，适用于功能开发、Bug 修复、页面开发、接口开发和联调，不限定具体技术栈。

---

## project-explorer

项目代码探索与业务结构分析

---

## ui-reviewer

当前端、后台、小程序或桌面端新增或明显修改页面后调用，负责独立检查 UI、UX、布局、信息层级和真实用户操作体验。重点发现页面太乱、太丑、信息堆积、操作反人类、模板感和 AI 生成感，只审查不修改代码。

---

# 推荐工作流

对于简单修改：

主 Agent / fullstack-developer
→ 必要验证

对于普通功能：

Explore
→ fullstack-developer
→ code-reviewer

对于复杂功能：

Explore
→ architect
→ fullstack-developer
→ code-reviewer

涉及重要数据库变更：

Explore
→ architect
→ database-reviewer
→ fullstack-developer
→ code-reviewer

涉及重要页面：

Explore
→ architect（复杂页面时）
→ fullstack-developer
→ ui-reviewer
→ code-reviewer

---

# 调度要求

1. 子智能体适合独立完成的工作，优先委派，减少主会话无意义上下文增长。

2. 不要重复调研。如果 Explore 已经获得充分证据，其他子智能体优先使用已有结论并针对必要部分补充读取。

3. 不要机械调用所有子智能体。根据任务实际影响范围选择。

4. 只读审查角色不得修改代码。

5. 开发角色完成修改后，不应把“自己检查自己”作为大型任务唯一的质量保障。

6. 用户明确指定某个子智能体时，优先按照用户指定执行。

7. 多个互不依赖的只读分析任务可以并行委派。

8. 存在前置依赖的任务按照正确顺序执行，不为了并行而并行。

9. 子智能体返回结果后，主 Agent 负责综合判断，不机械接受所有建议。

10. 所有开发、审查和设计最终仍必须遵循当前 Workspace AGENTS.md 中的项目级规则。
<!-- SUB_AGENTS_RULES_END -->
