# Feynman Agent Platform

本上下文定义平台中“现实证据、Agent 责任、变更治理与自维护”的核心语言，保证产品和代码讨论指向同一组概念。

## 租户、项目与责任

**Tenant**：Feynman 的组织级隔离与资源归属范围，管理成员、Projects 和租户共享资源。
_Avoid_: User, Project

**User**：登录 Feynman 并在一个或多个 Tenant 中执行操作的自然人身份；可以拥有个人 External Connection。
_Avoid_: Tenant, account（account 可能指外部渠道账号）

**Project**：属于一个 Tenant 的业务/产品项目，也是长期身份、事实、代码、制品、权限和发布历史的责任单元。一个 Project 只有一个 Main Agent。
_Avoid_: workspace（运行环境，不是 Project）；tenant（Tenant 是更上层的组织隔离）

**Main Agent**：一个 Project 的长期负责人和业务对话主体；模型可以替换，但其项目责任连续。
_Avoid_: worker, chatbot

**Agent Session**：Main Agent 在 Project 中长期拥有的运行时身份与持久 Workspace 绑定；可以承载多个 Agent Runs，具体运行时进程被恢复或替换时身份仍连续。
_Avoid_: Agent Run, Codex Session（Codex Session 是一种运行时实现）

**Sub-Agent**：Main Agent 临时调度的计算资源，不拥有 Project 的长期身份或控制权。
_Avoid_: co-owner, second agent

**Strong Reviewer**：平台信任域内独立重建判断、审查证据和发布风险的主体；它不修改 Worker 代码。
_Avoid_: second developer, approver（批准结果是它的输出，不是它的唯一职责）

**Conversation**：用户与 Project Main Agent 可见的持续沟通线程，是 Agent Runs、Observations 和外部 Actions 的人类可读投影，不是系统唯一的事实来源。
_Avoid_: chat log, Message（Message 是事件的一种）

**Agent Run**：Main Agent 被用户、外部事件或例行任务唤醒后完成的一次工作周期，可以调用多个 Tool，结果必须可恢复和审计。
_Avoid_: Codex session, prompt turn（Codex Session 是实现方式，不是责任实体）

**Session Heartbeat**：运行时向 Feynman 报告 Agent Session 存活状态的信号；它不等同于 Conversation Message，也不自动代表一次工作已发生。周期性唤醒任务是另一类触发器，触发后才创建 Agent Run。
_Avoid_: chat ping, Agent Run（一次 liveness heartbeat 不必创建 Run）

**Codex Session**：承载一次或多次 Agent Run 的具体模型/运行时会话；它可以被重启或替换，不能承载 Project 的唯一长期身份、记忆或权限。
_Avoid_: Main Agent, permanent process

**Tool Definition**：由 Feynman 维护、跨 Tenant 共享的外部能力及其受限操作定义；不包含某个用户或租户的外部账号授权。
_Avoid_: connection, credential

**External Connection**：Feynman 与某个外部渠道账号之间的已授权连接，由 User 或 Tenant 持有；其中的凭证不属于 Agent 可见状态。
_Avoid_: Tool Definition, Project Tool Grant

**Tool Grant**：将 External Connection 的指定操作授权给 Tenant 内某个 Project 和 Agent 角色使用的可撤销关系。
_Avoid_: credential, Capability

**Tool**：由 Tool Definition 和有效 Tool Grant 共同形成、以受限操作暴露给 Agent 的外部能力；它可以读取现实，也可以产生需要治理的外部 Action。
_Avoid_: unrestricted command, admin access

**Tool Adapter**：在 Tool seam 上连接具体 CLI、HTTP、MCP 或其他执行方式的实现；它返回结构化 Tool Result，由平台决定是否转化为 Observation 或 External Action。
_Avoid_: DWS integration（DWS 只是一个具体 Tool）

**Tool Invocation**：Agent Runtime 对一个已配置 Tool 发起的一次调用请求；平台必须在执行前校验调用上下文、Project 归属和有效 Tool Grant。
_Avoid_: unrestricted command, Tool Grant（授权关系不是一次调用）

**Tool Result**：一次 Tool Invocation 返回的结构化结果，带来源与观测时间；它是待解释的证据材料，不自动等同于 Claim 或已验证事实。
_Avoid_: truth, Observation（只有进入证据记录后才成为 Observation）

**External Action**：Tool 对 Project 外部现实产生的副作用，例如发送消息、修改记录或发布制品；它必须绑定 Capability，并按风险决定是否需要人工确认。
_Avoid_: observation, side effect hidden in tool output

## 现实与理解

**Observation**：来自某个事实源、带时间和可复现 provenance 的原始观测或断言。用户消息也是 Observation 的一种。
_Avoid_: truth, message（Message 只是事件的一种）

**Claim**：由一个或多个 Observation 支持的关于业务现实的结论，可能与其他 Claim 冲突。
_Avoid_: fact（Fact 只在有证据支持时作为 Problem Model 的字段出现）

**World State**：Agent 基于当前 Evidence 对 Project 现实的最好解释，不会抹掉历史 Observation。
_Avoid_: database snapshot, final truth

**Problem Model**：从 Evidence 到实现之间的业务理解，包含目标、实际流程、事实、不确定性、约束和可验证成功标准。
_Avoid_: prompt, PRD（PRD 可以是来源，但不是完整 Problem Model）

**Unknown**：仍未确认且可能改变实现方向的问题。只有高影响 Unknown 会阻止进入实现。
_Avoid_: every question, TODO

## 交付与自维护

**Change Package**：交给 Strong Reviewer 的完整变更证据包，包含 Problem Model、原始 Evidence、Diff、测试、制品 digest、权限变化和回滚方案。
_Avoid_: PR, deployment request

**Capability**：针对具体 Project、动作、资源、变更和短时间窗口发放的一次性最小权限授权，尤其用于高风险 External Action；不同于可撤销的 Tool Grant。
_Avoid_: credential, admin access

**Artifact**：由干净构建环境产生、以不可变 digest 标识的可部署制品。
_Avoid_: latest image, build output

**Self Repo**：Main Agent 可以在审查后维护的项目级长期经验和方法集合，不包含平台安全边界。
_Avoid_: platform core, unrestricted self-modification

**Evaluation**：来自测试、指标、用户反馈、事故或运行观测的结果，用来判断一次变更是否改善了现实。
_Avoid_: self-score
