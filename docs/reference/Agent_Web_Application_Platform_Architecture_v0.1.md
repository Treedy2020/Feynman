**多租户 Agent Web 应用平台**

**架构设计与产品原则 · v0.1**

以 Project 单主 Agent 为责任主体，以 Evidence 为事实基础，以 Reviewer 为发布治理，面向无状态 Web 应用持续交付

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>核心定义</strong></p>
<p>一个 Project = 一个长期负责的 Main Agent + 一套项目事实/记忆 + 一个受控的开发执行环境 + 一条由 Strong Reviewer 监管的 GCloud 发布链路。</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

工作稿 · 2026-09-16

# 0. 执行摘要

本设计面向多租户、非开发者用户。平台以 Project 为基本隔离与责任单元，每个 Project 只有一个长期 Main Agent。Main Agent 不是沙盒中的进程，而是可信平台中的长期数字主体；开发执行环境只是其可远程操控的“电脑”。

V1 聚焦无状态 Web 应用：代码、构建、制品、Staging 与 Production 主要映射到 Google Cloud。Agent 在短生命周期 DevSession 中进行快速自调试，正式构建交给 Cloud Build，镜像进入 Artifact Registry，Staging/Production 使用 Cloud Run；Strong Reviewer 以独立证据审查变更，并通过受限审批与临时能力控制生产发布。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>不是 Prompt → Generate → Deploy</strong></p>
<p>平台追求的是 Observe → Understand → Align → Build → Verify → Observe。需求不是最高真理，而是多种事实来源之一；软件开发首先是理解真实业务，再把这种理解转化成可验证的软件行为。</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 0.1 核心产品判断

- 企业软件不是越多越好。越贴近业务流程、边界越清晰、责任越确定的软件，越有长期价值。

- 高质量开发的核心不是代码生成速度，而是对客观世界规律的准确洞察与持续反馈。

- AI 不应是用户意志的传声筒；它应是有独立观测能力、质疑能力、拒绝能力和解释责任的事实解读者。

- 对非开发者用户，沟通应尽量使用业务语言；技术复杂性由 Agent 内部吸收。

- 实现之前优先消除高影响不确定性：能自己观察的先观察，只有答案会改变方案的问题才打断用户询问。

## 0.2 V1 的技术边界

| **范围** | **V1 决策** |
|----|----|
| 应用形态 | 优先无状态 HTTP Web 应用 |
| 运行平台 | Google Cloud |
| 构建 | Cloud Build |
| 镜像制品 | Artifact Registry，按 digest 审核与部署 |
| 应用运行 | Cloud Run Staging / Production |
| Agent 开发环境 | 短生命周期 DevSession / 隔离 Sandbox |
| 长期状态 | Project 数据、Git、Memory、Evidence、Artifacts；不依赖长期 Linux 主机 |
| 发布治理 | Strong Reviewer + Policy + Capability Broker + Audit |

# 1. 产品与工程原则

## 1.1 软件是业务现实的可执行模型

功能数量不是价值指标。软件应尽量准确地表达真实业务对象、流程、边界、责任与异常处理方式。对于一个新增功能请求，Agent 首先判断它是否解决了真实问题、是否重复已有事实源、是否引入不必要流程，而不是立即进入实现。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>原则</strong></p>
<p>“能做”不等于“应该做”。如果现有事实源已经更可靠地表达业务状态，Agent 应有权建议不新增功能。</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 1.2 Requirement 只是 Evidence 的一种

用户消息、PRD、数据库、日志、网页、邮件、CRM、Git、监控指标都属于事实来源。用户对业务语义通常有高价值，但用户消息本身仍然是一种带来源和时间的 Assertion，而不是不可质疑的系统真相。

## 1.3 实现方式自治，权限边界不自治

Main Agent 可以自主决定如何研究、写代码、调整内部 Skill、设计测试和优化实现，但不能自行扩展权限边界。生产部署、重要 API 能力、敏感数据访问和新增 OAuth Scope 必须通过 Capability Broker、Reviewer 或人类审批获得临时授权。

## 1.4 Agent 是独立合作者

| **能力** | **期望行为**                                             |
|----------|----------------------------------------------------------|
| 观察     | 主动从可用事实源取证，而不是只复述用户消息               |
| 质疑     | 发现来源冲突时明确指出差异并继续调查                     |
| 拒绝     | 事实不足、产品方向错误或权限越界时可暂缓/拒绝执行        |
| 解释     | 面向用户用业务语言说明原因，技术细节默认隐藏             |
| 负责     | 对项目连续结果负责，知道过去做过什么、为什么做、结果如何 |

# 2. 领域模型与责任边界

## 2.1 Project 是基本隔离单元

用户/组织可以拥有多个 Project。Project 承载独立的目标、状态、权限、Evidence、Memory、Git、制品与部署历史。Project 比 Agent Runtime 更稳定：模型版本可以更换，但 Project 身份和责任连续性不能丢失。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>Organization / User</p>
<p>├─ Project A</p>
<p>│ ├─ Main Agent A</p>
<p>│ ├─ Memory / Self Repo</p>
<p>│ ├─ Evidence / World State</p>
<p>│ ├─ Git / Artifacts</p>
<p>│ ├─ Capability Scope</p>
<p>│ └─ Deployment History</p>
<p>├─ Project B</p>
<p>│ └─ Main Agent B</p>
<p>└─ Project C</p>
<p>└─ Main Agent C</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 2.2 一个 Project 只有一个长期 Main Agent

Main Agent 是 Project 的长期责任人和对话主体。它拥有项目级长期身份与上下文，可以临时调度 Sub-Agent，但 Sub-Agent 只作为短期计算资源，不拥有项目长期身份，不与 Main Agent 争夺项目控制权。

## 2.3 Reviewer 属于平台信任域

Strong Reviewer 不属于某个 Project。它以独立上下文和更强推理资源审查 Change Package，可主动读取 Evidence，但不修改 Worker 代码。Reviewer 负责 challenge 假设、判断证据是否充分以及决定允许进入 Test/Canary/Production 的等级。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>职责分离</strong></p>
<p>Worker/Main Agent 能写但不能独立放行生产；Reviewer 能审但不能改代码；Deployment Controller 才真正执行已批准的发布。</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

# 3. 总体架构

平台按职责划分为 Interaction、Agent、Evidence/Understanding、Execution、Control 五个平面。Evidence Plane 回答“现实是什么”，Execution Plane 回答“如何改变现实”，Agent 位于两者之上进行独立判断。

<img src="media/image1.png" style="width:6.61417in;height:3.72245in" />

图 1 · 多租户 Agent Web 应用平台总体架构

| **平面** | **职责** |
|----|----|
| Interaction Plane | 用户与 Project Main Agent 的业务语言交流；不暴露底层技术复杂度 |
| Agent Plane | Main Agent、Reviewer、规划、长期身份、Memory、Self Repo |
| Evidence / Understanding Plane | 来源接入、Observation、证据关联、World State、Problem Model |
| Execution Plane | DevSession、测试、浏览器、构建、Staging/Production 执行 |
| Control Plane | IAM、Capability、审批、临时凭证、审计、制品签署与部署治理 |

# 4. Evidence / Observation / Understanding

## 4.1 Source 是平台级抽象

所有外部输入统一映射为带 provenance 的 Observation。Source Adapter 可对应用户消息、Web、Gmail、Drive、Git、数据库、日志、Metrics、文件或其他 SaaS。Agent 不直接把 Tool 返回文本当结论，而是通过 Evidence Engine 关联、冲突检测与时效性判断建立 World State。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>Observation</p>
<p>source = user / db / web / log / git / ...</p>
<p>observed_at = timestamp</p>
<p>actor = who/what produced it</p>
<p>content = raw value / assertion / event</p>
<p>provenance = reproducible reference</p>
<p>freshness = time relevance</p>
<p>authority = source authority for this topic</p>
<p>directness = direct observation vs retelling</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 4.2 Observation → Claim → World State

Observation 表示世界对 Agent 暴露了什么；Claim 表示这些 Observation 支持的结论；World State 表示 Agent 当前对项目现实的最好解释。历史事实不简单覆盖，关键业务事实应保留时间有效区间。

## 4.3 Problem Model：Evidence 与实现之间的关键层

在进入 Coding 前，Agent 应建立 Problem Model。它描述目标、业务对象、现有流程、痛点、事实、约束、异常路径、成功标准与未确认问题。它比传统 PRD 更接近“当前对现实的可执行理解”。

| **Problem Model 字段** | **含义**                             |
|------------------------|--------------------------------------|
| Goal                   | 真正希望改变的业务结果，而非界面功能 |
| Observed Workflow      | 当前业务实际如何运行                 |
| Facts                  | 已经有证据支持的重要事实             |
| Unknowns               | 仍会显著改变方案的不确定性           |
| Constraints            | 权限、合规、预算、数据边界等限制     |
| Success Criteria       | 实现完成后如何客观判断是否有效       |
| Rejected Alternatives  | 已经分析但不建议采用的方案及原因     |

# 5. 面向业务的 GrillMe / Discovery Skill

GrillMe 的目标不是最大化提问数量，而是以最低用户认知负担消除高影响不确定性。它应遵循“能观察就先观察、能推导就谨慎推导、只有会改变方案的问题才询问用户”的原则。

## 5.1 Discovery 流程

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>用户提出目标</p>
<p>↓</p>
<p>Agent 主动观察现有系统 / 数据 / 文档</p>
<p>↓</p>
<p>建立初步 Problem Model</p>
<p>↓</p>
<p>识别高影响 Unknowns</p>
<p>↓</p>
<p>只向用户询问关键业务问题</p>
<p>↓</p>
<p>更新 Problem Model</p>
<p>↓</p>
<p>Implementation Readiness Gate</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 5.2 Implementation Readiness Gate

- 目标与业务结果足够明确。

- 核心业务对象和关键状态语义明确。

- 关键事实来源明确，主要来源冲突已处理或显式记录。

- 主要流程边界、权限边界和异常路径已理解。

- 成功/验收标准可验证。

- 尚未解决的不确定性不会使当前实现方向失真，或用户明确接受该风险。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>避免“追求绝对清晰”</strong></p>
<p>现实业务永远存在不确定性。Agent 不是等到所有问题都被回答，而是在重大决策变量达到可实施阈值后进入实现，并持续保留剩余 Unknowns。</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 5.3 三类拒绝权

| **拒绝类型** | **触发条件** | **用户侧表达** |
|----|----|----|
| Epistemic | 关键事实不足或冲突未解决 | “这个语义会直接影响结果，我暂时不会实现，需要先确认。” |
| Product | 功能请求可能不是问题的正确解法 | “可以做，但它不会解决当前主要问题；我建议先处理 X。” |
| Safety / Authority | 超出 Project 或 Agent 权限边界 | “我可以准备变更方案并提交审核，但不会直接修改生产权限。” |

# 6. Main Agent 的自调试与开发闭环

Agent Runtime 自身不运行在 Sandbox 中。Sandbox 是远程执行环境，Agent 通过 Computer API 获得类似本地 bash / 文件 / 浏览器 / 日志的体验。开发阶段使用短生命周期 DevSession，项目持久性来自 Git、Evidence、Memory 与 Artifact，而不是长期运行的 Linux 主机。

## 6.1 三层反馈环

| **Loop** | **目标** | **典型能力** | **速度** |
|----|----|----|----|
| Hot Debug Loop | 快速定位和修复大多数开发问题 | edit / bash / dev server / browser / logs / tests | 100ms～数秒级 |
| Build & Staging Loop | 验证干净环境可复现、云端真实配置可运行 | Cloud Build / image / Cloud Run Staging / logs | 几十秒～分钟级 |
| Release & Observe Loop | 验证真实用户影响与业务效果 | Reviewer / Canary / Production / Metrics | 分钟～小时级 |

## 6.2 Debug 最小证明闭环

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>1. Reproduce</p>
<p>证明问题真实存在</p>
<p>2. Diagnose</p>
<p>基于 logs / browser / HTTP / tests 提出原因</p>
<p>3. Patch</p>
<p>修改代码</p>
<p>4. Re-run</p>
<p>原复现路径由 fail 变 pass</p>
<p>5. Regression Test</p>
<p>将问题固化为测试</p>
<p>6. Broader Test</p>
<p>检查是否破坏其他行为</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

Agent 不应因为“看起来已经修好”就进入发布。Before-fail → After-pass 是 Reviewer 后续可复查的重要 Evidence。

# 7. GCloud 构建、Staging 与生产交付

## 7.1 Golden Path

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>Main Agent</p>
<p>↓</p>
<p>Git commit</p>
<p>↓</p>
<p>Cloud Build</p>
<p>↓</p>
<p>Artifact Registry</p>
<p>↓</p>
<p>immutable image@sha256</p>
<p>↓</p>
<p>Cloud Run Staging</p>
<p>↓</p>
<p>Strong Reviewer</p>
<p>↓</p>
<p>approved release / attestation</p>
<p>↓</p>
<p>Cloud Deploy / controlled rollout</p>
<p>↓</p>
<p>Cloud Run Production</p>
<p>↓</p>
<p>Logs / Metrics / User feedback</p>
<p>↓</p>
<p>Evidence Plane</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 7.2 为什么正式构建必须脱离 DevSession

DevSession 允许快速反馈，但它不是可信制品来源。Cloud Build 从干净环境重新安装依赖、执行测试并构建镜像，是可复现性 Gate。Reviewer 应审核不可变 digest，而不是 tag（如 latest），确保“被审的制品”和“实际上线的制品”完全一致。

## 7.3 Staging 是云端自调试的一部分

本地 DevSession 无法完整复现 IAM、Secret、CORS、真实域名、Cloud Run 运行参数和网络边界，因此 Staging 仍属于 Main Agent 的自调试闭环。Main Agent 可以读 Staging 日志并重现错误，但权限/IAM 变更仍需 Capability Broker 与 Reviewer。

# 8. Strong Reviewer 与 Release Governance

## 8.1 Reviewer 审查 Change Package

| **内容**               | **说明**                           |
|------------------------|------------------------------------|
| Goal / Problem Model   | 这次改动试图解决什么现实问题       |
| Evidence               | 原始事实、复现结果与关键业务依据   |
| Diff / Artifact Digest | 具体改变了什么、准备上线哪一个制品 |
| Tests                  | 单测、集成、浏览器、历史 replay    |
| Metrics Delta          | 新旧行为是否客观改善               |
| Permission Delta       | 是否申请了新的能力/权限            |
| Deployment Plan        | 如何 Canary / Promote              |
| Rollback Plan          | 发生异常时如何恢复                 |

## 8.2 Reviewer 输出不是简单 approve/reject

| **状态**                | **含义**                           |
|-------------------------|------------------------------------|
| REJECT                  | 存在明确问题，返回 Main Agent 修正 |
| TEST_ONLY               | 可继续实验，不允许触达生产         |
| CANARY                  | 仅允许有限用户/流量验证            |
| PRODUCTION              | 证据充分，可进行正式推广           |
| HUMAN_APPROVAL_REQUIRED | 超出 Reviewer 自主权限天花板       |

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>独立性原则</strong></p>
<p>Reviewer 共享现实世界 Evidence，但不共享 Main Agent 的结论。它应从原始需求、Problem Model、Diff、测试和运行证据独立重建判断。</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

# 9. Capability Broker、JIT 权限与 Secret 安全

Agent Runtime 位于可信平台，但 Agent 的决策本身仍按 Zero-Trust Principal 处理。Main Agent 不持有长期生产凭证；需要高权限动作时，只能请求针对具体资源、动作、时间和制品的临时 Capability。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>Main Agent</p>
<p>│ request capability</p>
<p>▼</p>
<p>Capability Broker</p>
<p>├─ identity</p>
<p>├─ project scope</p>
<p>├─ policy</p>
<p>├─ reviewer/human approval</p>
<p>└─ audit</p>
<p>│</p>
<p>▼</p>
<p>short-lived credential</p>
<p>/ one-shot capability</p>
<p>│</p>
<p>▼</p>
<p>Deployment / External API</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 9.1 Capability 建议绑定的字段

| **字段**         | **示例**              |
|------------------|-----------------------|
| Principal        | agent_123             |
| Project / Tenant | project_A / company_X |
| Task / Change    | change_9821           |
| Action           | deploy                |
| Resource         | prod/invoice-service  |
| Artifact         | sha256:…              |
| TTL              | 10 min                |
| Max Uses         | 1                     |
| Approval         | reviewer_7            |
| Audit ID         | audit_9271            |

## 9.2 Secret 原则

- 长期 API Key 不进入 LLM Context、Sandbox、日志和 Memory。

- 能用短期 Token 或 Service Account Impersonation 时，不发放静态 Key。

- Agent 更适合获得“允许做某件事”的 Capability，而不是通用管理员凭证。

- Reviewer 负责签署批准，Deployment Controller 才真正执行生产动作。

# 10. 基于外部事实的自优化与长期责任

Main Agent 的优化目标不是“让自己感觉更好”，而是让项目的真实结果变好。任何 Self-Improvement 都应经过 Observation → Hypothesis → Candidate Change → Test/Replay → Canary → Real-world Observation 的闭环。

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><blockquote>
<p>Observe</p>
<p>↓</p>
<p>发现失败 / 用户修正 / 指标变化</p>
<p>↓</p>
<p>Hypothesis</p>
<p>↓</p>
<p>修改 Skill / Script / Workflow / Code</p>
<p>↓</p>
<p>Replay / Deterministic Tests</p>
<p>↓</p>
<p>Reviewer / Canary</p>
<p>↓</p>
<p>Real-world Observation</p>
<p>↓</p>
<p>保留、继续优化或回滚</p>
</blockquote></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

## 10.1 Agent Core 与 Agent-owned Self Repo 分离

| **平台不可自治修改**                     | **Agent 可自治维护**          |
|------------------------------------------|-------------------------------|
| Tool protocol / IAM / Capability / Audit | Memory / Lessons              |
| Reviewer policy / release boundary       | Skills / Scripts / Routines   |
| Sandbox isolation / platform security    | Project-specific tools        |
| Credential mint / secret boundary        | Self model / capability notes |

这种分离允许 Agent 真正积累长期能力，同时保证安全边界不会被它通过“自优化”绕过。

# 11. 核心数据对象

| **对象**           | **说明**                                   |
|--------------------|--------------------------------------------|
| Project            | 长期隔离、权限与责任的基本单元             |
| MainAgent          | Project 的长期负责人；模型可替换，身份连续 |
| Observation        | 带来源、时间和 provenance 的原始观测       |
| Claim / WorldState | 基于 Evidence 建立的当前事实解释           |
| ProblemModel       | 实现前的业务理解、Unknowns 与验收标准      |
| DevSession         | 短生命周期的隔离开发执行环境               |
| ChangePackage      | 提交 Reviewer 的完整变更证据               |
| Artifact           | 不可变构建制品及 digest                    |
| CapabilityGrant    | 短期、最小权限、可审计授权                 |
| Deployment         | Staging/Canary/Production 的发布记录       |
| Evaluation         | 测试、指标、用户反馈和现实结果             |
| Memory / SelfRepo  | Agent 长期经验、技能和内部方法             |

## 11.1 Message 不是系统核心表

Conversation Message 只是 Event 的一种。平台更核心的事实应以 Events、Observations、Changes、Artifacts、Capabilities、Deployments 与 Evaluations 表达，这样 Agent 才能长期知道“发生了什么”，而不仅是“聊过什么”。

# 12. MVP 建议与演进顺序

## 12.1 Phase 1：证明自治 Web 开发闭环

1.  Project / Main Agent 基础模型。

2.  Git + DevSession + bash/files/browser/logs Computer API。

3.  Problem Model + 简化 GrillMe Readiness Gate。

4.  Cloud Build → Artifact Registry → Cloud Run Staging。

5.  Reviewer Change Package 审查。

6.  Production Cloud Run 的受控发布。

7.  Logs/Metrics/User correction 回流 Observation Store。

## 12.2 Phase 2：治理与客观评估

8.  Capability Broker 与 JIT Credential。

9.  Canary / Rollback / Artifact Attestation。

10. 多 Source Evidence Adapter。

11. 历史任务 Replay 与 regression evidence。

12. Reviewer 独立取证与风险分级。

## 12.3 Phase 3：长期自优化

13. Self Repo / Skill versioning。

14. 自动识别重复任务并抽象工具/Skill。

15. 长期 World State / temporal facts。

16. 低风险改动的 Reviewer 自动放行策略。

17. 更完整的项目级主动维护与定期 retrospective。

# 13. 关键风险与开放问题

| **风险 / 问题** | **需要验证的设计点** |
|----|----|
| 过度澄清导致体验迟缓 | GrillMe 必须基于 Decision Impact，避免问答表单化 |
| Agent 自信但证据不足 | 强制 provenance、Unknowns 和 Readiness Gate |
| Reviewer 与 Worker 同源偏差 | 独立上下文、原始 Evidence、必要时模型/提示差异化 |
| 自调试环境与正式环境漂移 | Cloud Build 作为可复现性 Gate，Staging 作为真实云环境 Gate |
| 权限逐渐膨胀 | Project Capability Envelope + JIT Grant + TTL + Max Uses |
| 成本失控 | 无状态 Web 优先、DevSession 短生命周期、构建/Reviewer 分级调度 |
| 用户将 Agent 误认为绝对权威 | 显式表达 Evidence、冲突与不确定性；独立但不傲慢 |

<table>
<colgroup>
<col style="width: 100%" />
</colgroup>
<thead>
<tr>
<th><p><strong>V1 最重要的验证问题</strong></p>
<p>一个长期 Main Agent 能否在真实 Evidence 的约束下，先把业务问题理解清楚，再通过快速自调试、强 Reviewer 和 GCloud 交付闭环，长期独立维护一个无状态 Web 项目？</p></th>
</tr>
</thead>
<tbody>
</tbody>
</table>

# 附录 A · 一页式系统哲学

| 软件不是越多越好，而应尽可能准确地表达和支撑真实业务流程。 |
|------------------------------------------------------------|

| 开发不是把需求翻译成代码，而是持续理解现实，并用软件对现实进行可验证的改善。 |
|----|

| 用户消息是一种重要事实来源，但不是唯一事实来源。 |
|--------------------------------------------------|

| Agent 不是传声筒，而是会调查、会质疑、会解释，也会在证据不足时暂缓实现的独立合作者。 |
|----|

| 实现方式自治，权限边界不自治。 |
|--------------------------------|

| Reviewer 不是第二个开发者，而是独立的证据审查与发布治理者。 |
|-------------------------------------------------------------|

| 真正的自优化必须由外部事实闭环验证，而不是 Agent 自己给自己打分。 |
|-------------------------------------------------------------------|

# 附录 B · 推荐内部术语

| **术语** | **建议定义** |
|----|----|
| Project | 用户理解的业务/产品项目；一个 Project 一个长期 Main Agent |
| Main Agent | Project 的长期负责人与业务对话主体 |
| Evidence Plane | 现实世界的统一观测与证据接口 |
| Problem Model | 进入实现前对业务现实和目标的结构化理解 |
| GrillMe | 面向业务、基于高影响不确定性的澄清能力 |
| DevSession | Main Agent 的临时开发机 / 快速自调试环境 |
| Strong Reviewer | 独立审查 Evidence、Change 与 Release Risk 的平台级 Agent |
| Capability Broker | 对高风险现实动作发放最小化临时权限的控制面 |
| Change Package | 供 Reviewer 审查的目标、证据、代码、测试、权限和发布方案集合 |

# 附录 C · 五个关键 Gate

这五个 Gate 是 V1 最值得直接固化进状态机和审计日志的控制点。

| **Gate** | **进入条件** | **输出** | **主要责任方** |
|----|----|----|----|
| Understanding Gate | 关键业务语义、事实源、边界与验收标准达到可实施阈值 | READY_TO_IMPLEMENT / 继续 Discovery | Main Agent |
| Build Gate | DevSession 已完成 fail→pass 与基础 regression | 不可变 Artifact Digest | Cloud Build |
| Review Gate | Change Package、Evidence、测试与风险信息完整 | REJECT / TEST_ONLY / CANARY / PRODUCTION | Strong Reviewer |
| Capability Gate | 动作超出默认 Project Capability Envelope | 短期最小权限 Grant | Capability Broker / Policy |
| Observe Gate | Canary/Production 已产生足够现实反馈 | Promote / Hold / Rollback / 新一轮优化 | Reviewer + Main Agent |

**建议状态机主干：** DISCOVERY → READY_TO_IMPLEMENT → DEV → BUILD → STAGING → REVIEW → CANARY → PRODUCTION → OBSERVE

任何 Gate 都应允许回退，而不是只有向前流转：理解不足回到 Discovery；构建失败回到 Dev；Reviewer 拒绝回到实现；Canary 异常回到 Rollback/Dev。
