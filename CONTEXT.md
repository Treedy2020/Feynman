# Feynman Agent Platform

本上下文定义平台中“现实证据、Agent 责任、变更治理与自维护”的核心语言，保证产品和代码讨论指向同一组概念。

## 租户、项目与责任

**Tenant**：Feynman 的组织级隔离与资源归属范围，管理成员、Projects 和租户共享资源。
_Avoid_: User, Project

**User**：登录 Feynman 并在一个或多个 Tenant 中执行操作的自然人身份；可以拥有个人 External Connection。
_Avoid_: Tenant, account（account 可能指外部渠道账号）

**Project**：属于一个 Tenant 的业务/产品项目，也是长期身份、事实、代码、制品、权限和发布历史的责任单元。它可以拥有零个或多个长期 Project Agents。
_Avoid_: workspace（运行环境，不是 Project）；tenant（Tenant 是更上层的组织隔离）

**Project Agent**：属于某个 Project、拥有独立长期身份与会话的 Agent；多个 Project Agents 可以使用同一 Project Workspace，但不共享会话身份。
_Avoid_: Sub-Agent, Workspace

**Default Agent**：Project 可选的缺省对话路由目标；缺省路由不存在时，消息发送方必须明确选择 Project Agent。
_Avoid_: Main Agent（角色不是路由）, Project owner

**Main Agent**：Project Agent 可以承担的协调角色，不是每个 Project 必须拥有的唯一 Agent。
_Avoid_: Default Agent, Project owner

**Agent Session**：Project Agent 的长期运行时身份与 Project Workspace 绑定；可以承载多个 Agent Runs，具体运行时进程被恢复或替换时身份仍连续。
_Avoid_: Agent Run, Codex Session（Codex Session 是一种运行时实现）

**Sub-Agent**：Project Agent 临时调度的计算资源，不拥有 Project Agent 的长期身份或控制权。
_Avoid_: co-owner, second agent

**Strong Reviewer**：可选的独立审查主体，重建判断并复核证据与发布风险；不直接修改被审查 Agent 的工作区，也不是项目或任务的必需前置条件。
_Avoid_: second developer, approver（批准结果是它的输出，不是它的唯一职责）

**Project Workspace**：Project 长期拥有的文件系统与执行环境绑定，容器可被替换，项目持久文件继续保留；它不包含任何 Project Agent 的唯一会话身份。
_Avoid_: Project, permanent container

**Project Task**：属于一个 Project、委派给指定 Project Agent 的持久工作请求；执行记录可以关联一次 Agent Run，不等于已经验收的交付。
_Avoid_: prompt turn, schedule

**Project Schedule**：由用户授权、按时间或外部事实源变化触发的持久消息规则；绑定一个明确的 Project Agent，外部事件按整个 Schedule 收集成窗口后投递。
_Avoid_: heartbeat, perpetual agent

**Schedule Occurrence**：Project Schedule 的一次实际触发，标识同一收集窗口或同一计划时刻的一组待投递事实。
_Avoid_: Agent Run, Delivery

**Schedule Delivery**：某次 Schedule Occurrence 发给指定 Project Agent 的消息投递，具有独立于 Agent Run 的接收与对账状态。
_Avoid_: Schedule Occurrence, Agent Run

**Agent Inbox Message**：持久化给指定 Project Agent 的输入，可以来自用户、Schedule 或被授权的外部通知；进入收件箱不等于 Agent 已接收或完成处理。
_Avoid_: Schedule Occurrence, Agent Run

**Local Harness**：用户本地使用的 Agent 客户端，通过 Feynman Skill 与 CLI 管理多个 Project；它不是这些 Project 的 Project Agent。
_Avoid_: Project Agent, control-plane daemon

**Conversation**：用户与某个 Project Agent 可见的持续沟通线程，是 Agent Runs、Observations 和外部 Actions 的人类可读投影，不是系统唯一的事实来源。
_Avoid_: chat log, Message（Message 是事件的一种）

**Agent Run**：Project Agent 被用户、外部事件或例行任务唤醒后完成的一次工作周期，可以接收多个消息并调用多个 Tool，结果必须可恢复和审计。
_Avoid_: Codex session, prompt turn（Codex Session 是实现方式，不是责任实体）

**Session Heartbeat**：运行时向 Feynman 报告 Agent Session 存活状态的信号；它不等同于 Conversation Message，也不自动代表一次工作已发生。周期性唤醒由 Project Schedule 管理，可启动新的 Agent Run 或追加输入到正在运行的 Run。
_Avoid_: chat ping, Agent Run（一次 liveness heartbeat 不必创建 Run）

**Codex Session**：承载一次或多次 Agent Run 的具体模型/运行时会话；它可以被重启或替换，不能承载 Project Agent 的唯一长期身份、记忆或权限。
_Avoid_: Project Agent, permanent process

**Tool Definition**：由 Feynman 维护、跨 Tenant 共享的外部能力及其受限操作定义；不包含某个用户或租户的外部账号授权。
_Avoid_: connection, credential

**External Connection**：Feynman 与某个外部渠道账号之间的已授权连接，由 User 或 Tenant 持有；其中的凭证不属于 Agent 可见状态。
_Avoid_: Tool Definition, Project Tool Grant

**External Access**：Server 内独立的外部权限模块，统一处理账号、项目申请、绑定与实际调用校验。Rust V1 首批 Provider 为 DWS、GitLab、Git；只读 CLI 在本地 Server 执行，不把凭证下发到 Sandbox。
_Avoid_: arbitrary CLI proxy, encrypted vault（V1 是私有文件存储）

**Project CLI Binding**：Rust V1 中将 Project / Provider 显式关联 Connection、仓库（GitLab / Git）和有效期的对象；与 Tool Grant 同时满足才允许业务调用。登记或绑定账号不自动授予业务权。
_Avoid_: login, implicit inherited permission

**Tool Grant**：将 External Connection 的指定操作授权给 Tenant 内某个 Project 和 Agent 角色使用的可撤销关系。
_Avoid_: credential, Capability

V1 实际 ToolGrant 仍为 Tenant / Project / Tool 许可，账号与仓库限制由独立 ProjectCliBinding 补充；完整角色与资源策略尚未实现。可信个人 loopback 的协议边界不构成同一 OS 用户进程间的强隔离。

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

**Observation Source**：Project 中一次明确授权范围的外部事实源注册；即使目标相同，不同注册仍拥有独立身份和历史。
_Avoid_: External Connection（账号不是来源注册）, Schedule

**Observation Channel**：一个 Observation Source 持续形成的事实流及其只读投影；不是额外的账号、会话或授权主体。
_Avoid_: Agent Inbox, Conversation

**Observation Feed**：Project 内当前获权渠道的统一外部消息收件视图；汇总原始 Observation，不表示已经向 Agent 投递。
_Avoid_: Agent Inbox Message, Agent Run

**Channel Watch**：人或本地客户端对某个渠道或统一 Feed 的实时查看；关闭查看不停止后台采集，也不确认 Agent 已接收。
_Avoid_: Source listener（采集器）, Schedule 收集窗口

**Observation Read Position**：某个查看者已浏览到的持久事实位置；与来源采集进度、Schedule 消费进度和 Agent 投递确认分别独立。
_Avoid_: Agent acknowledgement, Source cursor

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

**Self Repo**：Project Agent 可以在审查后维护的项目级长期经验和方法集合，不包含平台安全边界。
_Avoid_: platform core, unrestricted self-modification

**Evaluation**：来自测试、指标、用户反馈、事故或运行观测的结果，用来判断一次变更是否改善了现实。
_Avoid_: self-score
