# 原生 Codex Executor：会话调度与执行环境分离 Smoke

日期：2026-09-28。结论：**本机 Codex app-server 与 Spark 上 OpenSandbox 容器内
Codex exec-server 已真实分离；用户指定的 1–4 项验收均通过。**

这次验证走 Codex 原生环境协议，不是此前的文件 MCP。Feynman Rust 后端/CLI
的产品运行路径没有切换；新增 Python 文件仅用于 operator 级部署与集成验收。

## 实际拓扑

```text
本机 macOS
  Codex app-server 0.157.1
  主 Agent / 两个 Sub-Agent / 模型认证 / 持久会话历史
          |
          | 原生 Executor 协议，SSH loopback tunnel
          v
Spark ARM64 / OpenSandbox 0.2.3 / Docker runc
  Codex exec-server 0.157.1
  原生 Shell / apply_patch / Python unittest
          |
          +-- /workspace -> 同一个 Docker named volume
```

`initialize` 开启 `experimentalApi`；`environment/add` 注册 `execServerUrl`；
`environment/info` 返回 `/bin/bash`、`file:///workspace`；`thread/start` 和
`turn/start` 的 `environments` 显式绑定环境 ID 与 `/workspace`。
环境 ID 保持稳定，容器 ID、地址、执行器连接可以更换。

协议以**本机 0.157.1 生成的 Schema 和真实响应**为准。
[官方 App Server 文档](https://learn.chatgpt.com/docs/app-server)提供环境检查、
会话恢复与事件流的背景；其中的 remote TUI / Code Mode host 不应被等同于
本次原生远端 Executor 接入。

## 验收与证据

| 验收项 | 实际验证 | 结果 |
| --- | --- | --- |
| 1. 主 Agent 远端执行 | pwd、uname、hostname、读取未出现在 prompt 的随机 marker；原生 apply_patch 修复加法；原生 unittest 成功 | 通过 |
| 2. 原生 Sub-Agent 派发 | 两个子 Agent 继承同一 environment 与 /workspace；各自读取 marker、写独立结果；读取两个子会话的原生命令历史 | 通过 |
| 3. 宿主文件隔离 | 对本机、Spark 宿主的 dummy canary 分别尝试读与 r+ 写；四项均失败；独立检查原文件未变 | 通过 |
| 4. 恢复 | 隧道断开不回退本机；重连、Executor 重启、容器替换、app-server 重启后继续同一 thread，重新读文件并测试 | 通过 |

恢复前先给主 Agent 一个**仅存在对话中、禁止写入文件的随机口令**。
每次恢复均要求重新执行命令、读取 Workspace marker、运行 unittest，并返回
记住的口令。Workspace 文件快照保持一致；容器替换后 hostname 改变。
因此“文件仍存在”与“会话上下文仍连续”有分别的真实证据。

首次完整 run 的独立核对结果：

- 主会话 thread ID：`01a0e7fb-dbc8-7bf0-91dd-f4d22061caa5`。
- 两个子会话均存在独立 `commandExecution` 和相同环境绑定。
- **15 条完成的原生命令**，cwd 全部为 `/workspace`；**0 次 MCP 调用**。
- 原生文件修改路径为 `/workspace/calculator.py`。
- 隧道断开后状态为 `disconnected`；命令因远端连接超时失败，未改用本机。
- 四种恢复后均继续原 thread，返回相同 marker/会话口令，测试成功。
- 容器重建、复制 Executor、启动、建立隧道及原生环境绑定：**1.36 秒**。
- 全量验收加清理：**123.61 秒**，其中包括实际模型回合和断线等待。

上述是单次暖缓存 Smoke，不是冷启动指标或生产 SLO。
完整本地证据目录：
`/var/folders/tb/bxhyq9ld1gg6lpb97zk98m8c0000gn/T/feynman-native-smoke-zses1kps/`，
包括 `verification.json`、`native-events.jsonl`、`subagent-threads.json` 和
各阶段答案。远端 operator 日志目录：`/tmp/feynman-os-run-ViCyXCkk/`。
仓库不保存真实凭证或原始随机 marker/口令。

加固后再次完整复跑也通过：子 Agent 验收必须在其**自己的 thread 事件**中找到
远端 hostname，不能用继承的父会话历史充当执行证据；清理步骤即使部分失败也会
继续清理其他资源，并将整个验收标记为失败。本次独立核对：

- 主 thread：`01a0e800-ec28-7a10-ab48-5f12fab794fa`，四项验收和四种恢复均通过。
- **17 条完成的原生命令**，cwd 全部为 `/workspace`；**0 次 MCP 调用**。
- 原生 patch 修改 `/workspace/calculator.py` 和子 Agent 的 `/workspace/subagent-b.json`。
- 暖缓存容器重建与环境重新绑定 **1.23 秒**；全量验收加清理 **121.56 秒**。
- 清理后无测试容器或 Volume；再次独立确认宿主无临时服务 listener，原服务健康。

复跑证据目录：
`/var/folders/tb/bxhyq9ld1gg6lpb97zk98m8c0000gn/T/feynman-native-smoke-vc4o82ht/`；
远端日志目录：`/tmp/feynman-os-run-8T45Omx1/`。
`verification.json` 的 `last_runtime_*` 仅统计重启后的最后一个 app-server；
上述全量事件数独立统计整个 `native-events.jsonl`，包含重启前后的全部阶段。

TDD 在用户确认的原生协议边界按四个纵向切片执行：各验收先失败，再补对应
连接/检查步骤，最后分别通过。额外验证：Python 编译、既有 GKE 工具边界测试、
`cargo test --workspace`（14 项 Rust 测试）和 `git diff --check` 通过。
真实集成测试默认跳过，必须显式 opt-in；跳过不能被报告为一次真实 Smoke 通过。

## 实测边界与安全限制

1. **内层 bwrap 不可用。** 普通 Docker 的限制拒绝创建用户命名空间；带
   workspace permission profile 的线程在加载远端 AGENTS.md 时即失败。
   未改变 Spark 内核配置、未添加 SYS_ADMIN、未启用 privileged，也未重启 Docker。
   本次明确采用远端线程 `:danger-full-access`，依赖外层 unprivileged Docker/runc
   隔离。该模式确实移除内层 Codex 限制，不能当作 workspace-write 的替代安全证明。
   [官方 Permission Profile 定义](https://learn.chatgpt.com/docs/permissions)。
2. **隔离检查不是逃逸审计。** 无宿主 bind mount、Docker socket、GPU 或认证文件；
   容器 drop ALL capabilities、no-new-privileges、1 CPU / 1 GiB。
   仍是共享内核的 runc，不是 gVisor/微虚机或生产多租户隔离。
3. **Executor listener 是受信任测试接口。** 容器内 44773 不发布到宿主；本机
   经 SSH 转发访问其 bridge 地址。该原生 listener 无额外认证，不能暴露到公网或
   与不可信 workload 共用的可达 bridge 网络。生产需独立网络与认证边界。
4. **恢复由 operator 实施。** 容器更换后重新复制 Executor 并注册连接；app-server
   更换后重新注册环境，再 `thread/resume`。尚未实现 Feynman 的自动生命周期恢复。
5. **持久化不是进程 checkpoint。** `/workspace` 文件由 Volume 保留，中央会话由
   本机 Codex runtime home 保留；容器根文件系统、PTY、正在运行的进程和任意系统
   安装状态没有被承诺恢复。客户端 `fs/*`/`process/*` RPC 也没有被验证为远端 API。
6. **原生事件形状需兼容。** 本版本派发使用 `subAgentActivity(kind=started)`；不能
   只识别旧 `collabAgentToolCall(tool=spawnAgent)`。只取 `final_answer` 作为答案，
   避免把 commentary 混入结构化验收结果。

## 清理与复跑

已删除本次测试容器与 named volume，停止临时 OpenSandbox 服务、本机 app-server
和 SSH tunnels。独立检查无 fixture-labelled 容器/volume，Spark 的 18080/44773
无宿主 listener；原 `uptime-kuma` 仍健康。没有创建 GKE 或使用 GPU。
测试 Volume 中只有 dummy Workspace 数据，删除不可恢复；日志、包缓存和私有
会话证据仍保留，且无临时认证链接。

复跑方法见 [Native Executor Smoke 操作说明](../../scripts/smoke/README.md#native-codex-executor-separation-smoke)。

## 对 Feynman 下一步的影响

可以优先复用原生 Executor，不必自建 Shell/apply_patch/Sub-Agent 工具集合。
接入 Rust 产品前仍需实现：持久化项目/会话与 Volume、环境 ID 的关联；容器地址
变化后的原生连接重新注册；启动时同一 thread 的恢复；连接状态与 Session Heartbeat
的映射；断线/超时可见性；生产隔离与凭证边界。

外部观测渠道的凭证仍由中心 Feynman broker 管理。长期独立 Reviewer 需另一个
Session/Workspace，不能因为普通 Sub-Agent 共享环境已通过，就将 Reviewer 也接到
Generator 的可写现场。
