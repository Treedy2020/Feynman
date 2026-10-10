/* Native SVG, selectable by mouse, touch or keyboard. All content is local. */
(() => {
  "use strict";
  const svg = document.getElementById("architecture-svg");
  if (!svg) return;
  const details = {
    harness: { title: "用户 Harness", status: "已实现", location: "用户本机 · 管理入口", summary: "用户在自己的原生 Codex 或其他 Agent 中安装 Feynman Skill，通过 CLI 管理所有项目。这个会话不是项目 Main Agent。", duties: ["查询跨项目状态、任务与执行记录。", "明确 Project、目标、边界及验收条件。", "将批准和授权留给用户明确决定。"], state: "用户 harness 保有自己的 Home；Feynman 不改写它的配置或会话。", command: "feynman --json status", doc: "architecture/local-first-main-agent-v1.html", docLabel: "CLI 与运行指南" },
    server: { title: "Feynman Server", status: "已实现", location: "用户本机 · 共享 Rust 控制面", summary: "一个 Feynman 管理多个 Project：创建项目、调度各自 Main Agent、准备独立 Workspace，并替项目发起外部认证和权限申请。不是只有一个项目的 Agent 宿主。", duties: ["每 Project 0..1 个未归档 Agent；按固定身份调度。", "单项目串行排队；不同项目可并行工作。", "统一维护项目状态、授权进度和用户沟通；不能代用户批准自身提权。"], state: "~/.feynman/\n默认监听 127.0.0.1:4317\n固定本地开发身份；不向网络公开。", command: "feynman --json projects list\nfeynman projects create --name \"新项目\"", doc: "architecture/backend-cli-v1.html", docLabel: "后端与 HTTP / CLI 契约" },
    agent: { title: "Project Agent", status: "已实现", location: "用户本机 · Codex app-server", summary: "一个项目的长期责任主体。由原生 Agent 决定如何研究、写代码、验证和沟通；进程可以重启，Thread 保持连续。", duties: ["稳定 Agent ID；CRUD / 启停 / 能力交集由用户控制。", "原生 Shell / 文件工具明确绑定 Execution World。", "权限不足时等待审批，不自动扩张能力。"], state: "Project → 0..1 Agent → 独立 Session / Conversation / Thread。\n缺少原生会话日志时恢复失败，不悄悄新建 Thread。", command: "feynman send PROJECT_ID --message \"说明你的计划\" --wait", doc: "architecture/agent-schedules.html#agents", docLabel: "长期会话与项目所有权" },
    mcp: { title: "Feynman MCP / Grants", status: "已实现", location: "用户本机 · 项目工具边界", summary: "项目 Agent 通过标准 MCP 使用外部工具。Server 提供 Principal 和 Project 上下文，调用时再次核对 Tool Grant。", duties: ["工具是固定类型操作，不是任意 CLI 或 Shell 代理。", "登录成功不等于授予业务工具权限。", "Device 登录由 Server 异步维护，Agent Turn 结束不销毁授权任务。"], state: "项目 Grant 与公共授权元数据存入 SQLite。\n原始 Token 不返回给 Agent，也不写入项目 Workspace。", command: "feynman external-tools list PROJECT_ID", doc: "reference/external-authentication-flow.html", docLabel: "公共认证与项目授权流程" },
    sqlite: { title: "SQLite / Inbox Scheduler", status: "已实现", location: "~/.feynman/rust-platform.sqlite3", summary: "保存 Feynman 的项目消息、事件、Run、任务、定时计划、授权和 Workspace 绑定。这里不是一份新造的 JSONL 上下文。", duties: ["Occurrence / Inbox / 开窗与水位采用事务；来源注册独立历史。", "Schedule 默认暂停；错过的周期合并。", "重启不自动重放可能已经产生外部副作用的工作。"], state: "计划运行依赖 Server 存活且本机未休眠。\nHeartbeat 是独立存活概念，尚无新增对外接口。", command: "feynman sources list PROJECT_ID\nfeynman schedules list PROJECT_ID", doc: "architecture/agent-schedules.html#delivery", docLabel: "调度与恢复约定" },
    home: { title: "Feynman Codex Home", status: "已实现", location: "~/.feynman/codex/ · 本地持久化", summary: "Feynman 管理自己的原生配置、会话 JSONL、状态库和认证缓存。会话持久化不依赖远端容器是否仍然存在。", duties: ["子进程的 CODEX_HOME 与 CODEX_SQLITE_HOME 显式绑定此目录。", "默认不导入个人配置、认证或会话；可以独立登录。", "显式认证导入仅复制 auth.json，不链接或覆盖不同缓存。"], state: "新目录 0700；新配置与认证副本 0600。\n同一用户的多个项目共用此 Home，并非强租户隔离。", command: "feynman home\nfeynman init", doc: "architecture/local-user-home.html", docLabel: "用户目录、认证与备份" },
    dws: { title: "DWS 外部事实源", status: "Rust V1 已实现", location: "Server CLI → DingTalk 服务", summary: "device 认证、受限历史读取与托管消息长连接。搜索使用明确绑定的 Connection，不回退主机默认 profile。", duties: ["用户在浏览器完成 consent；成功请求经新鲜验证登记账号。", "每次核对项目 Grant、Binding、到期与账号身份。", "Agent 不接收原始认证凭证。"], state: "private/dws/ 保存独立缓存。\n来源 ID / 游标 / Capture 在本机持久化；本轮离线验证，未启动真实订阅。", command: "feynman access connections register-dws PROJECT_ID AUTHORIZATION_ID\nfeynman access bind PROJECT_ID --provider dws --connection CONNECTION_ID", doc: "modules/external-access/dws.html", docLabel: "DWS 子模块" },
    sandbox: { title: "OpenSandbox Provider", status: "已接入并验证", location: "Docker 主机 · 生命周期 API", summary: "为每个 Project 创建独立容器与持久卷。运营者提供固定镜像和私有连接配置，Agent 不能自行选择任意主机路径。", duties: ["创建 / 查询 / 暂停 / 恢复 / 续期并核对资源归属。", "已授权工作可恢复缺失容器；模糊创建先核对现有资源。", "远端不可用、身份不符或非私有端点时失败，不回退本机执行。"], state: "当前是个人可信 Docker/runc 环境，不是 gVisor 或 VM。\nSmoke 资源已清理，常驻远端服务仍需运营配置。", command: "feynman workspace create PROJECT_ID --provider opensandbox\nfeynman workspace refresh PROJECT_ID", doc: "architecture/local-first-main-agent-v1.html#opensandbox-operator-setup", docLabel: "Provider 配置与操作" },
    executor: { title: "Native Executor", status: "已接入并验证", location: "Project 容器 · 原生工具执行", summary: "本地 Codex 调度工具，远端 exec-server 实际执行 Shell 和文件操作。使用原生执行协议，不重新实现 subagent 派发。", duties: ["environment/info 核验远端 /workspace。", "Thread start / resume 和每个 Turn 都明确绑定环境。", "原生 subagent 继承已在单独 smoke 验证。"], state: "远端使用 :danger-full-access，仅限外层可信容器内，需运营者明确同意。\n共享 bridge 与未认证执行端点尚需生产加固。", command: "feynman send PROJECT_ID --message \"检查 Workspace 文件\" --wait", doc: "architecture/native-executor-smoke-2026-09-28.html", docLabel: "原生 Executor 验证记录（历史）" },
    volume: { title: "Persistent Workspace", status: "已接入并验证", location: "Project named volume → /workspace", summary: "保存代码、项目文件及卷内依赖。容器替换后可重新挂载同一个卷；执行环境可以更换，项目文件不随 rootfs 消失。", duties: ["每个 Project 使用独立卷与容器。", "卷外依赖、内存、进程与 PTY 不由卷恢复。", "Workspace 不是应用的必选部署位置。"], state: "本地 Home 备份不包含远端卷；两者需分别备份。\n暂停 Docker 不是免费 scale-to-zero。", command: "feynman workspace stop PROJECT_ID\nfeynman workspace resume PROJECT_ID\nfeynman workspace restore PROJECT_ID", doc: "architecture/local-first-main-agent-v1.html#security-boundary", docLabel: "恢复和安全边界" },
    drlee: { title: "Drlee / drlee", status: "Rust V1 已接入并实测", location: "Server 原生 CLI → 指定 Drlee 实例", summary: "所有者显式导入一个原生账号或发起浏览器登录。Feynman 的私有副本独立刷新，不改变个人默认缓存。", duties: ["固定身份、渠道、知识空间读取；无任意 argv。", "读取消息需所有者绑定单一渠道 UUID。", "Agent 可申请能力，不可登记密钥或批准。"], state: "真实 Agent 身份与渠道读取通过。\n消息 Source / listener 尚未接入；不发消息或写业务数据。", command: "feynman access connections import-drlee --hostname drlee.example.com", doc: "modules/external-access/drlee.html", docLabel: "Drlee 原生认证与能力" },
    slate: { title: "Slate / dip-slate", status: "Rust V1 已接入并实测", location: "Server 真实 Slate CLI → 指定实例", summary: "保留 Slate 浏览器 PKCE；也可由所有者显式导入选定 Keychain profile。登录成功后才登记 Connection。", duties: ["固定身份、个人任务和可见项目读取。", "运营者配置可信原生 Python 包；不把 CLI 复制进镜像。", "原生认证不是 DWS device 协议，不虚构后台状态机。"], state: "真实浏览器授权及 Agent 读取通过。\n任务数为 0 不代表未认证；无 task / project 写能力。", command: "feynman access connections login-slate --hostname slate.example.com", doc: "modules/external-access/slate.html", docLabel: "Slate PKCE 与只读工具" },
    reviewer: { title: "Reviewer", status: "可选预留", future: true, location: "未来项目角色 · 非当前主流程", summary: "一个独立的评审 Agent 与 Main Agent 交流 Review Round。当前项目只有 Main Agent，不要求 Reviewer 才能工作。", duties: ["长期 Session 与执行权限边界需独立定义。", "与 Generator / Main Agent 通过项目事实和审查证据沟通。", "发布 Review、部署工具和自治维护闭环尚未实现。"], state: "不是当前已运行的第二个 Agent，也不是 MVP 的强制 Gate。", doc: "reference/Agent_Web_Application_Platform_Architecture_v0.1.html", docLabel: "原始治理设计（历史）" },
    gke: { title: "GKE + gVisor", status: "未来 Provider", future: true, location: "替换执行环境，不替换会话控制面", summary: "保留 Kubernetes Agent Sandbox / PVC / gVisor 的未来适配点。当前 MVP 使用 OpenSandbox + Docker，不依赖运行中的 GKE 集群。", duties: ["具体 Runtime 与存储保证由 Provider 提供。", "PVC 恢复文件不等于恢复 Agent 历史或内存。", "历史成本与启动实测不应当作当前价格或 SLA。"], state: "历史 GKE 测试集群已销毁。没有本轮新建资源或重新启用计费。", doc: "architecture/gke-sandbox-model-and-startup.html", docLabel: "GKE 历史模型与计时边界" }
  };
  details.projects = { title: "Project Registry · A / B / … / N", status: "已实现", location: "Feynman Server · 多项目管理", summary: "Feynman 可以创建并管理任意多个项目。图中的 A / B 是两个并列示例，不是实时项目，也不是项目数量上限。", duties: ["每个 Project 保有自己的 Main Agent / Thread、队列和 Schedule。", "每个 Project 绑定独立 Workspace 与 Tool Grant。", "跨项目查询集中在本地管家；消息、权限和执行文件不因共用 Server 而自动共享。"], state: "SQLite 按 Tenant / Project 归属持久化。\n当前项目共享用户 Codex Home 与固定开发身份，不构成强安全租户隔离。", command: "feynman --json projects list\nfeynman projects create --name \"Project A\"\nfeynman projects create --name \"Project B\"", doc: "architecture/local-first-main-agent-v1.html#ownership", docLabel: "项目所有权与运行方式" };
  details.access = { title: "External Access Broker", status: "Rust 初版 · 五个 Provider", location: "Feynman Server · 独立领域模块", summary: "统一项目申请、账号登记、限期绑定和固定 CLI 调用。DWS、GitLab、Git、Slate、Drlee 共用权限内核；认证差异留在原生 Adapter。", duties: ["Connection / Project Binding / Tool Grant 分别管理。", "Agent 只可 providers / status / request，不能登记 Token 或 approve。", "只读 Server Proxy 与固定 Sandbox clone 分开审计。"], state: "八类组合 seam、单 Agent 来源 / Inbox、env 装载、使用租约和 MemoryStore 已接入。\n新增 Provider 真正短期签发和强租户隔离不在当前版本。", command: "feynman access providers\nfeynman access status PROJECT_ID", doc: "modules/external-access/index.html", docLabel: "权限模块与真实验收" };
  details.connections = { title: "Connections / Credential Store", status: "Rust V1 已实现 · 非通用 Vault", location: "用户私有目录 · 不在 Workspace", summary: "保存明确的外部账号及私有凭证引用；同一所有者可以显式为多个项目绑定账号，不自动继承其他项目的权限。", duties: ["仅经验证登记；原生账号缓存仅由所有者显式导入。", "管理元数据只公开身份，不返回 Token 或私有路径。", "业务调用按 Connection 串行；实际身份改变拒绝。"], state: "private/external-access/ 私有 SQLite、0600 Token 或单账号原生缓存。\nDWS 缓存在 private/dws/；不是加密 Vault 或同一 OS 用户间的强隔离。", command: "feynman access connections list\nfeynman access connections disable CONNECTION_ID", doc: "modules/external-access/index.html#limits", docLabel: "存储与安全边界" };
  details.gitlab = { title: "GitLab / glab", status: "Rust V1 已接入并实测", location: "Server CLI → 指定 GitLab 实例", summary: "所有者显式原生导入或 Token stdin；固定身份验证、仓库元数据与开放 MR 读取。", duties: ["repo.read 与 mr.read 分别授予。", "固定 GET，业务参数只能是空对象。", "不开放评论、MR 创建、合并或 CI 触发。"], state: "已显式复用公司原生登录，真实 Agent 读取 Slate 仓库成功。\nV1 不托管 glab device 流程。", command: "feynman access connections import-gitlab --hostname gitlab.example.com", doc: "modules/external-access/gitlab.html", docLabel: "GitLab 认证与读能力" };
  details.git = { title: "Git / HTTPS Transport", status: "Rust V1 已接入并实测", location: "Server remote 读取 / Sandbox 固定 clone", summary: "Git 无通用 login。显式委托 GitLab Connection，读取绑定 remote 或在项目 Sandbox 拉到 /workspace/repo。", duties: ["clone 仅接受 UUID 幂等键；不接受任意 URL / argv / 目标路径。", "已有工作树不覆盖；未知结果保留 key 并先检查。", "没有认证 fetch、push 或 SSH；克隆后可本地开发。"], state: "Token 仅进入固定 Git 进程环境，不写配置或收据。\n容器 root 可观察该进程；不是恶意 Agent 秘密隔离。", command: "feynman external-tools grant PROJECT_ID git.workspace.clone", doc: "modules/external-access/git.html", docLabel: "Git 子模块与传输边界" };
  details["access-requests"] = { title: "Access Requests", status: "Rust V1 已实现", location: "项目 MCP / 私有元数据", summary: "Agent 提出所需 capability 与 reason，所有者处理认证、绑定和 Grant。", duties: ["相同项目 / Provider / capability 有效请求去重。", "请求持久化 24 小时，不等于已登录或已批准。", "MCP 无登记密钥、绑定和 approve 操作。"], state: "配置状态不宣称 live authentication。\n更多 cancel / 批准状态机为后续演进。", doc: "modules/external-access/index.html#flow", docLabel: "权限流程" };
  details["access-policy"] = { title: "Bindings / Tool Grants", status: "Rust V1 已实现", location: "项目隔离与执行前校验", summary: "业务 Grant 与账号 / 仓库 / 期限绑定同时满足，才允许固定 CLI 读取。", duties: ["每 Provider 每项目一个绑定。", "默认 24h，有效期 60 秒至 7 天。", "停用、解绑、过期和撤销阻断新调用。"], state: "细粒度分支 / 资源 ACL 未实现。\n已启动的只读操作不承诺即时终止。", doc: "modules/external-access/index.html#boundary", docLabel: "项目权限对象" };
  details["cli-executor"] = { title: "Trusted CLI Executor", status: "Rust V1 已实现", location: "固定 Server CLI / 专用 Sandbox Git 进程", summary: "权限模块保留 dws / glab / git / Slate / Drlee 原生实现；不是任意远程 Shell 接口。", duties: ["不接受模型 executable、任意 argv / env。", "清洁环境、结果上限和有界超时；不返回原始 stderr。", "clone 经运行环境归属校验，不回退主机执行。"], state: "观察调用留在 Server；仅 Git clone 短暂使用容器进程凭证。\nCLI 与 Python 包由运营者可信配置，不由 Agent 安装到 Broker。", doc: "modules/external-access/index.html#layout", docLabel: "代码组织与边界" };
  for (const suffix of ["a", "b"]) {
    const name = suffix.toUpperCase();
    details["project-" + suffix] = { ...details.agent, title: `Project ${name} · Main Agent`, example: true, location: `项目 ${name} · 本地长期会话`, summary: `项目 ${name} 拥有独立 Main Agent、Thread、任务队列与 Schedule。图示用于表达多项目模型，不表示当前正在运行一个名为 ${name} 的项目。`, duties: ["同项目的消息、任务、定时计划进入同一 Conversation。", `原生工具只绑定 Workspace ${name}；不因 Project B / A 获权而获得额外权限。`, "通过项目 MCP 请求外部能力，由 Feynman 维护批准与认证状态。"], command: `feynman send PROJECT_${name} --message \"说明你的计划\" --wait` };
    details["grant-" + suffix] = { title: `Project ${name} · Grants`, status: "Tool Grant + Binding 已实现", example: true, location: `项目 ${name} · 权限不可自动继承`, summary: "项目 Tool Grant 与账号 / 仓库 / 到期 Binding 一起生效；登录成功不自动扩大能力。", duties: ["Agent 提供需求，所有者绑定并授予。", "A、B 即使使用同一账号，也需各自获权。", "调用前验证身份并核对最新授权。"], state: "基本绑定和 TTL 已落地。\n细粒度资源 / 分支约束仍待实现。", command: `feynman external-tools list PROJECT_${name}`, doc: "modules/external-access/index.html#boundary", docLabel: "项目绑定与 Grant" };
    details["workspace-" + suffix] = { ...details.volume, title: `Workspace ${name} · Executor`, example: true, location: `远端 · Project ${name} 独立容器与卷`, summary: `项目 ${name} 的执行环境。Main Agent 会话在本地调度，Shell / 文件操作经原生 Executor 在这个 Workspace 执行。A / B 使用不同容器和持久卷。`, command: `feynman workspace create PROJECT_${name} --provider opensandbox` };
  }
  const executionPositions = [
    ["harness", 28, 82, 230, 84, "Skill + CLI · 用户入口"], ["server", 337, 82, 230, 84, "Rust · 项目 / 消息 / 调度"], ["agent", 646, 82, 230, 84, "长期 Thread · 本地 app-server"],
    ["mcp", 28, 230, 230, 84, "项目级 MCP · Tool Grant"], ["sqlite", 337, 230, 230, 84, "消息 / Run / Task / Schedule"], ["home", 646, 230, 230, 84, "CODEX_HOME · 会话 / 认证"],
    ["dws", 28, 448, 230, 84, "主机 CLI · 事实获取"], ["sandbox", 337, 448, 230, 84, "生命周期 / 资源归属"], ["executor", 646, 448, 230, 84, "Shell / 文件 / apply_patch"], ["volume", 646, 585, 230, 72, "/workspace · 独立 named volume"],
    ["drlee", 28, 585, 230, 72, "插拔工具与凭证策略"], ["reviewer", 337, 738, 230, 84, "独立角色 · 非强制 Gate"], ["gke", 646, 738, 230, 84, "Agent Sandbox / gVisor / PVC"]
  ];
  const executionEdges = [
    ["harness", "server", "M258 124H337"], ["server", "agent", "M567 124H646"],
    ["server", "sqlite", "M452 166V230"], ["agent", "home", "M761 166V230"],
    ["agent", "mcp", "M646 124H626V202H143V230", true], ["mcp", "dws", "M143 314V448"], ["mcp", "drlee", "M28 272H18V621H28"],
    ["server", "sandbox", "M337 124H310V490H337", true], ["agent", "executor", "M876 124H898V490H876"],
    ["sandbox", "executor", "M567 490H646"], ["executor", "volume", "M761 532V585"]
  ];
  const projectPositions = [
    ["harness", 36, 72, 230, 72, "Skill + CLI · 所有项目的入口"], ["server", 368, 72, 230, 72, "统一调度 / 创建 / 权限申请"], ["projects", 700, 72, 270, 72, "Project A / B / … / N"],
    ["sqlite", 36, 174, 230, 72, "项目记录 / 队列 / Schedule"], ["home", 368, 174, 230, 72, "共享 Home · 独立项目 Thread"], ["access", 700, 174, 270, 72, "申请 / 认证 / 用户批准 / 检查"],
    ["project-a", 36, 337, 210, 84, "Thread A · Task / Schedule A"], ["grant-a", 274, 337, 210, 84, "Tool Grant + Binding / TTL"],
    ["project-b", 540, 337, 210, 84, "Thread B · Task / Schedule B"], ["grant-b", 778, 337, 210, 84, "Tool Grant + Binding / TTL"],
    ["workspace-a", 36, 548, 448, 72, "Sandbox A · /workspace → volume A"], ["workspace-b", 540, 548, 448, 72, "Sandbox B · /workspace → volume B"],
    ["connections", 36, 712, 270, 80, "私有账号缓存 · 不进入镜像"], ["dws", 368, 712, 270, 80, "Device → Connection → 事实搜索"], ["gitlab", 700, 712, 288, 80, "原生导入 / stdin · 仓库 / MR"],
    ["slate", 36, 816, 270, 80, "PKCE / Keychain · 固定只读"], ["git", 368, 816, 270, 80, "绑定 remote · Sandbox clone"], ["drlee", 700, 816, 288, 80, "原生浏览器 / 单账号 · 只读"],
    ["reviewer", 36, 980, 448, 72, "未来每项目可选 · 独立审查角色"], ["gke", 540, 980, 448, 72, "替换 Workspace Provider · 不改项目会话"]
  ];
  const projectEdges = [
    ["harness", "server", "M266 108H368"], ["server", "projects", "M598 108H700"],
    ["server", "sqlite", "M368 108H310V210H266"], ["server", "home", "M483 144V174"], ["server", "access", "M598 108H660V210H700", true],
    ["projects", "project-a", "M700 108H682V278H141V337"], ["projects", "project-b", "M835 144V278H645V337"],
    ["project-a", "grant-a", "M246 379H274", true], ["project-b", "grant-b", "M750 379H778", true],
    ["grant-a", "access", "M379 337V266H674V210H700", true], ["grant-b", "access", "M883 337V246", true],
    ["project-a", "workspace-a", "M141 421V548"], ["project-b", "workspace-b", "M645 421V548"],
    ["access", "connections", "M970 210H1006V650H171V712", true], ["connections", "dws", "M306 752H368", true],
    ["connections", "gitlab", "M171 792V804H844V792", true], ["connections", "git", "M306 752H328V856H368", true],
    ["connections", "slate", "M171 792V816", true], ["connections", "drlee", "M36 752H26V906H844V896", true]
  ];
  const accessPositions = [
    ["mcp", 28, 82, 230, 84, "providers / status / request"], ["access", 337, 82, 230, 84, "独立 Rust · 权限内核"], ["harness", 646, 82, 230, 84, "所有者认证 / 绑定 / Grant"],
    ["access-requests", 28, 230, 230, 84, "24h 请求 · 去重 / 持久化"], ["connections", 337, 230, 230, 84, "账号身份 / 私有凭证"], ["access-policy", 646, 230, 230, 84, "Project / repo / TTL / Grant"],
    ["dws", 28, 448, 230, 84, "device · 企业事实搜索"], ["gitlab", 337, 448, 230, 84, "原生导入 / stdin · 仓库 / MR"], ["git", 646, 448, 230, 84, "委托 HTTPS · remote / clone"],
    ["slate", 28, 576, 230, 84, "浏览器 PKCE · 固定只读"], ["drlee", 646, 576, 230, 84, "单账号副本 · 独立刷新"],
    ["cli-executor", 337, 752, 230, 84, "固定 argv / 清洁环境 / 上限"]
  ];
  const accessEdges = [
    ["mcp", "access", "M258 124H337"], ["harness", "access", "M646 124H567", true], ["mcp", "access-requests", "M143 166V230", true],
    ["access", "connections", "M452 166V230", true], ["harness", "access-policy", "M761 166V230", true], ["access-policy", "connections", "M646 272H567", true],
    ["connections", "dws", "M452 314V364H143V448"], ["connections", "gitlab", "M452 314V448"], ["connections", "git", "M452 364H761V448"],
    ["connections", "slate", "M337 272H18V618H28"], ["connections", "drlee", "M567 272H902V618H876"],
    ["dws", "cli-executor", "M258 490H300V714H452V752"], ["gitlab", "cli-executor", "M452 532V752"], ["git", "cli-executor", "M646 490H604V714H452V752"],
    ["slate", "cli-executor", "M143 660V714H452V752"], ["drlee", "cli-executor", "M761 660V714H452V752"]
  ];
  const layouts = { projects: projectPositions, execution: executionPositions, access: accessPositions };
  const ns = "http://www.w3.org/2000/svg";
  function el(tag, attrs = {}, text) {
    const node = document.createElementNS(ns, tag);
    for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
    if (text !== undefined) node.textContent = text;
    return node;
  }
  const viewport = document.getElementById("diagram-viewport");
  const inspector = document.getElementById("component-inspector");
  const picker = document.getElementById("component-picker");
  const futureToggle = document.getElementById("show-future");
  let future = document.body.dataset.future === "true";
  let view = Object.hasOwn(layouts, document.body.dataset.diagramView) ? document.body.dataset.diagramView : "projects";
  let selected = view === "access" ? "access" : "server";
  let zoom = 1;
  const htmlEscape = (text) => String(text).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const docURL = (path) => window.FeynmanDocs.url(path);
  function resize() {
    svg.style.width = Math.round(Math.max(viewport.clientWidth, 740) * zoom) + "px";
    svg.style.minWidth = "0";
    document.getElementById("zoom-reset").textContent = Math.round(zoom * 100) + "%";
  }
  function inspect(id, updateURL = true) {
    const item = details[id];
    if (!item) return;
    selected = id;
    for (const node of svg.querySelectorAll("[data-component]")) {
      const active = node.dataset.component === id;
      node.classList.toggle("is-selected", active);
      node.setAttribute("aria-pressed", String(active));
    }
    for (const edge of svg.querySelectorAll("[data-edge]")) {
      const related = edge.dataset.edge.split(" ").includes(id);
      edge.classList.toggle("is-related", related);
      edge.setAttribute("marker-end", related ? "url(#arrow-active)" : "url(#arrow)");
    }
    for (const button of picker.querySelectorAll("button")) button.setAttribute("aria-pressed", String(button.dataset.component === id));
    inspector.innerHTML = `<div class="inspector-heading">COMPONENT DETAIL / 组件细节</div>
      <h2 id="component-title">${htmlEscape(item.title)}</h2>
      <span class="status ${item.future || item.proposed ? "future" : ""}">${htmlEscape(item.status)}</span>
      <p class="component-location">${htmlEscape(item.location)}</p><p>${htmlEscape(item.summary)}</p>
      <div class="inspector-grid"><section><h3>职责与行为</h3><ul>${item.duties.map(x => `<li>${htmlEscape(x)}</li>`).join("")}</ul></section>
      <section><h3>状态与边界</h3><p>${htmlEscape(item.state).replace(/\n/g, "<br>")}</p></section></div>
      ${item.command ? `<h3>CLI 入口</h3><pre><code>${htmlEscape(item.command)}</code></pre>` : ""}
      <h3>继续阅读</h3><a href="${htmlEscape(docURL(item.doc))}">${htmlEscape(item.docLabel)} ↗</a>`;
    if (updateURL) {
      try { history.replaceState(null, "", "#view=" + view + "&node=" + id); } catch (_) { /* file mode */ }
    }
  }
  function render() {
    svg.replaceChildren();
    const overview = view === "projects";
    const accessView = view === "access";
    const positions = layouts[view];
    const edges = overview ? projectEdges : accessView ? accessEdges : executionEdges;
    svg.dataset.diagramView = view;
    svg.setAttribute("viewBox", overview ? `0 0 1024 ${future ? 1080 : 920}` : accessView ? "0 0 920 860" : `0 0 920 ${future ? 848 : 684}`);
    svg.append(el("title", { id: "svg-title" }, overview ? "一个 Feynman 管理多个 Project 及外部权限" : accessView ? "External Access 内核与 DWS / GitLab / Git / Slate / Drlee" : "单项目本地会话与远端执行架构"), el("desc", { id: "svg-desc" }, "点击或键盘选择组件查看详情。A / B 是并列项目示例，不是实时状态。虚线框表示目标设计或未来预留。"));
    const defs = el("defs");
    for (const id of ["arrow", "arrow-active"]) {
      const marker = el("marker", { id, viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 6, markerHeight: 6, orient: "auto-start-reverse" });
      marker.append(el("path", { d: "M0 0L10 5L0 10Z", fill: id === "arrow" ? "var(--muted)" : "var(--accent)" }));
      defs.append(marker);
    }
    svg.append(defs);
    const planes = overview ? [[16, 24, 984, 236, "共享管理 / FEYNMAN 本地控制面"], [16, 296, 484, 178, "PROJECT A / 本地独立上下文"], [520, 296, 480, 178, "PROJECT B / 本地独立上下文 · 可继续创建 N 个项目"], [16, 510, 484, 122, "EXECUTION WORLD A / 独立容器与工作卷"], [520, 510, 480, 122, "EXECUTION WORLD B / 独立容器与工作卷"], [16, 672, 984, 240, "EXTERNAL ACCESS / 私有账号缓存 · 五个 Provider · 固定调用"], ...(future ? [[16, 938, 984, 128, "可选演进 / 尚未实现"]] : [])] : accessView ? [[16, 26, 876, 308, "共享权限内核 / ACCOUNT ≠ PROJECT BINDING ≠ TOOL GRANT"], [16, 394, 876, 286, "PROVIDER / 原生认证与固定操作"], [294, 708, 304, 138, "执行边界 / SERVER PROXY + SANDBOX CLONE"]] : [[16, 26, 876, 308, "用户本机 / CONTROL & AGENT PLANE"], [16, 394, 260, 278, "外部事实源"], [294, 394, 598, 278, "Docker 主机 / EXECUTION WORLD"], ...(future ? [[294, 700, 598, 134, "可选演进 / 尚未实现"]] : [])];
    for (const [x, y, width, height, label] of planes) {
      svg.append(el("rect", { x, y, width, height, rx: 7, class: "svg-plane" }), el("text", { x: x + 14, y: y + 26, class: "svg-plane-label" }, label));
    }
    for (const [from, to, path, dashed] of edges) svg.append(el("path", { d: path, class: "svg-edge" + (dashed ? " dashed" : ""), "data-edge": from + " " + to, "marker-end": "url(#arrow)" }));
    const labels = overview ? [[291, 98, "CLI"], [619, 98, "创建"], [151, 496, "原生 Executor"], [655, 496, "原生 Executor"], [284, 448, "队列 A / Schedule A"], [788, 448, "队列 B / Schedule B"], [387, 300, "申请 / 项目 MCP"], [850, 300, "申请 / 项目 MCP"]] : accessView ? [[275, 111, "请求"], [581, 111, "批准"], [482, 351, "绑定 / 校验身份"]] : [[271, 112, "CLI"], [580, 112, "调度"], [150, 370, "device / search"], [371, 196, "MCP / Grant"], [588, 480, "容器"], [770, 567, "挂载"]];
    for (const [x, y, text] of labels) svg.append(el("text", { x, y, class: "svg-label" }, text));
    if (!overview && !accessView) {
      svg.append(el("text", { x: 300, y: 355, class: "svg-label", transform: "rotate(-90 300 355)" }, "生命周期 API"), el("text", { x: 911, y: 365, class: "svg-label", transform: "rotate(-90 911 365)" }, "原生工具执行"));
      if (!future) svg.append(el("text", { x: 351, y: 593, class: "svg-label" }, "会话与模型认证留在本地"), el("text", { x: 351, y: 614, class: "svg-label" }, "独立卷保留文件，不恢复进程"));
    }
    picker.replaceChildren();
    let number = 0;
    for (const [id, x, y, width, height, meta] of positions) {
      const item = details[id];
      if (item.future && !future) continue;
      number++;
      const node = el("g", { class: "svg-node" + (item.future ? " is-future" : "") + (item.proposed ? " is-proposed" : ""), role: "button", tabindex: 0, "data-component": id, "aria-label": `查看 ${item.title} 细节，${item.status}` });
      const shortTitles = { sqlite: "SQLite / Scheduler", home: "Codex Home", volume: "Persistent Workspace", mcp: "Feynman MCP", sandbox: "OpenSandbox", projects: "Project Registry", access: "External Access Broker", connections: "Connections / Credentials", "project-a": "Main Agent A", "project-b": "Main Agent B", "grant-a": "Project Grants A", "grant-b": "Project Grants B" };
      node.append(el("rect", { x, y, width, height, rx: 6 }), el("text", { x: x + 14, y: y + 19, class: "svg-node-number" }, String(number).padStart(2, "0")), el("text", { x: x + width - 12, y: y + 19, "text-anchor": "end", class: "svg-node-status" }, item.future ? "预留" : item.proposed ? "设计" : item.example ? "示例" : "已实现"), el("text", { x: x + 14, y: y + 43, class: "svg-node-title" }, shortTitles[id] || item.title), el("text", { x: x + 14, y: y + 63, class: "svg-node-meta" }, meta));
      node.addEventListener("click", () => inspect(id));
      node.addEventListener("keydown", event => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); inspect(id); } });
      svg.append(node);
      const button = document.createElement("button");
      button.type = "button"; button.dataset.component = id; button.textContent = item.title;
      button.addEventListener("click", () => inspect(id)); picker.append(button);
    }
    futureToggle.setAttribute("aria-pressed", String(future));
    futureToggle.textContent = future ? "隐藏预留能力" : "显示预留能力";
    for (const button of document.querySelectorAll("[data-diagram-view]")) {
      if (button.tagName === "BUTTON") button.setAttribute("aria-pressed", String(button.dataset.diagramView === view));
    }
    if ((!future && details[selected].future) || !positions.some(([id]) => id === selected)) selected = accessView ? "access" : "server";
    inspect(selected, false); resize();
  }
  function fromHash() {
    const params = new URLSearchParams(location.hash.slice(1));
    const requestedView = params.get("view");
    if (Object.hasOwn(layouts, requestedView)) view = requestedView;
    const value = params.get("node");
    if (value && Object.hasOwn(details, value)) {
      selected = value;
      if (!layouts[view].some(([id]) => id === value)) {
        view = Object.keys(layouts).find(key => layouts[key].some(([id]) => id === value));
      }
      if (details[value].future) future = true;
    }
    render();
  }
  document.getElementById("zoom-in").addEventListener("click", () => { zoom = Math.min(1.75, zoom + .15); resize(); });
  document.getElementById("zoom-out").addEventListener("click", () => { zoom = Math.max(.75, zoom - .15); resize(); });
  document.getElementById("zoom-reset").addEventListener("click", () => { zoom = 1; viewport.scrollLeft = 0; viewport.scrollTop = 0; resize(); });
  futureToggle.addEventListener("click", () => { future = !future; render(); inspect(selected); });
  for (const button of document.querySelectorAll("button[data-diagram-view]")) button.addEventListener("click", () => { view = button.dataset.diagramView; zoom = 1; viewport.scrollLeft = 0; viewport.scrollTop = 0; render(); inspect(selected); });
  new ResizeObserver(resize).observe(viewport);
  window.addEventListener("hashchange", fromHash);
  render(); fromHash();
})();
