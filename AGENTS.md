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

---

# GIT PUSH RULES

IMPORTANT: 用户要求提交推送时，必须同时推送两个远程仓库：

- `git push`（origin，GitHub，分支 `main`）
- `git push gitee main:master`（gitee，Gitee，分支 `master`）

两条推送都成功后才算完成，任一失败需如实报告。

<!-- SUB_AGENTS_RULES_BEGIN -->
# 子智能体协作与调度规则

## 基本原则

主 Agent 负责理解用户目标、维护整体任务上下文、拆解和调度子任务、传递必要上下文、综合判断结果以及最终汇报。

## 主 Agent 硬性职责边界

1. 主 Agent 不得直接新增、修改或删除项目源码、测试、SQL、配置、样式及其他实现性文件。
2. 所有代码实现和文件修改必须委派给开发类子智能体：简单、局部、低风险任务交给 lightweight-developer，复杂或高风险任务交给 fullstack-developer。
3. 主 Agent 可以进行必要的只读检索、结果核对和验证，但不得以“修改很简单”为由绕过开发类子智能体。
4. 子智能体执行失败或结果不完整时，主 Agent 应补充上下文后重试、继续派发或更换合适的子智能体，不得直接接管代码修改。
5. 如果当前环境中所需子智能体不可用，主 Agent 应明确说明阻塞原因并请求用户处理，不得静默改为自己实现。

对于能够独立完成的代码探索、架构分析、开发、代码审查、数据库审查、UI/UX 审查、视觉内容识别（截图 / 报错截图 / 设计稿 / 录屏）和全项目审计任务，应委派给对应子智能体，避免所有工作都在主会话上下文中完成。

子智能体完成任务后，由主 Agent 结合其结果继续推进当前任务。不要为了使用子智能体而机械拆分只读问答或重复派发同一项工作；但只要任务涉及实际文件修改，就必须使用开发类子智能体。

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

当任务涉及复杂业务、跨模块或跨前后端开发、数据库变更、权限、事务、并发、状态流转、复杂 Bug 修复或较高回归风险时调用。负责形成完整业务闭环并完成必要联调；简单、局部、方案明确且低风险的修改应优先交给 lightweight-developer。

---

## lightweight-developer

当任务属于简单、局部、方案明确且低风险的代码修改时优先调用。适用于文案或样式调整、已定位根因的小型 Bug、局部逻辑修正、简单 CRUD、单元测试补充和少量配置修改；发现跨模块、数据库结构、权限、事务、并发、状态机或高回归风险时应停止扩大修改并建议改派 fullstack-developer。

---

## project-auditor

当需要对整个项目、多个业务模块或较大范围代码进行系统性检查时调用。负责从业务逻辑、模块关联、权限、安全、数据一致性、异常处理、状态流转、前后端接口和数据库关系等维度进行全项目缺陷审查，重点发现真实 Bug、业务遗漏和潜在上线风险。只审查，不修改代码。

---

## project-explorer

当任务需要深入理解现有项目、复杂业务流程、跨模块调用关系、数据流转或修改影响范围时调用。区别于内置 Explore 的简单代码搜索，本智能体负责基于实际代码形成完整的业务链路和项目结构分析，为后续开发、架构设计和代码审查提供可靠上下文。只读分析，不修改代码。

---

## ui-reviewer

当前端、后台、小程序或桌面端新增或明显修改页面后调用，负责独立检查 UI、UX、布局、信息层级和真实用户操作体验。重点发现页面太乱、太丑、信息堆积、操作反人类、模板感和 AI 生成感，只审查不修改代码。

---

## vision

当任务涉及图片、截图、UI 页面、报错截图、设计稿、游戏画面、视频或录屏等视觉内容时调用。负责识别视觉信息、提取文字与界面结构、分析页面状态和操作过程，并将结果整理为可供主 Agent 和其他子代理继续使用的结构化信息。只负责视觉理解与分析，不修改代码。

---

# 推荐工作流

对于简单修改：

lightweight-developer
→ 必要时 code-reviewer

对于普通功能：

project-explorer（需要先理解现有实现时）
→ fullstack-developer
→ code-reviewer

对于复杂功能：

project-explorer
→ architect
→ fullstack-developer
→ code-reviewer

涉及重要数据库变更：

project-explorer
→ architect
→ database-reviewer
→ fullstack-developer
→ code-reviewer

涉及重要页面：

project-explorer
→ architect（复杂页面时）
→ fullstack-developer
→ ui-reviewer
→ code-reviewer

对于简单页面调整：

lightweight-developer
→ ui-reviewer（存在明显视觉或交互变化时）
→ 必要时 code-reviewer

提供报错截图、界面异常截图或设计稿等视觉材料时：

vision（先提取结构化信息：文字、界面结构、状态与关键点）
→ project-explorer（按视觉线索定位相关实现）
→ fullstack-developer
→ code-reviewer

涉及重要页面交付前的视觉走查：

fullstack-developer
→ ui-reviewer
→ vision（仅在有实际页面截图、设计稿或录屏时做还原度与视觉细节复核）
→ code-reviewer

对于交付前或上线前的整体检查：

project-auditor（项目背景不明时先用 project-explorer 补充上下文）
→ fullstack-developer（修复确认的问题）
→ code-reviewer

---

# 调度要求

1. 只要任务涉及实际文件修改，必须委派给 lightweight-developer 或 fullstack-developer；主 Agent 不参与代码改动。

2. 不要重复调研。如果 project-explorer 已经获得充分证据，其他子智能体优先使用已有结论并针对必要部分补充读取。

3. 不要机械调用所有子智能体。根据任务实际影响范围选择。

4. 只读审查角色不得修改代码。

5. 开发角色完成修改后，不应把“自己检查自己”作为大型任务唯一的质量保障。

6. 用户明确指定某个子智能体时，优先按照用户指定执行。

7. 多个互不依赖的只读分析任务可以并行委派。

8. 存在前置依赖的任务按照正确顺序执行，不为了并行而并行。

9. 子智能体返回结果后，主 Agent 负责综合判断，不机械接受所有建议。

10. 所有开发、审查和设计最终仍必须遵循当前 Workspace AGENTS.md 中的项目级规则。

11. lightweight-developer 只处理简单、局部、方案明确且低风险的修改；发现跨模块、数据库结构、权限、事务、并发、状态机或高回归风险时，应停止扩大修改并建议改派 fullstack-developer。

12. fullstack-developer 负责复杂业务、跨模块或跨前后端开发、完整联调以及高风险修改，不应承担本可由 lightweight-developer 完成的简单任务。

13. 委派时应提供清晰的任务目标、已知上下文、允许修改范围、禁止事项和验收条件，避免子智能体重复猜测需求。

14. 子智能体结果不完整时，应围绕缺失内容继续派发；如果同一任务需要升级，应把已有结论和已修改范围完整交接给新的子智能体。
<!-- SUB_AGENTS_RULES_END -->