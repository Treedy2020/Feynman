# GKE Sandbox 内容读取 Smoke · 2026-09-28

## 结论和范围

**读取 Smoke 通过。** 本地新启动的 Codex Agent 实际调用远程工具，读取了
GKE Agent Sandbox 的文件，并返回与独立验证器一致的随机标记和文件内容。
随机标记在 Pod 内生成，不出现在提示词中，验证器在 Agent 运行期间仅在内存中保存预期值。

```text
本地 Codex Agent
       |
       v
宿主侧受限 STDIO MCP（固定 Pod、固定命令、文件白名单）
       |
       v
GKE Agent Sandbox / gVisor / PVC /workspace
```

这不是完整的 Feynman CLI → Server → GKE 测试：Agent 仍运行在本机，尚未
将生产 WorkspaceProvider 接入 GKE，也未测试长期会话、暂停恢复、Pod 重建、
Generator/Reviewer 或应用部署。诊断脚本使用 Python 标准库；生产后端仍为 Rust。

## 实际环境

| 项目 | 实际值 |
| --- | --- |
| GCP 项目 | `nomadic-memento-464304-u1` |
| 区域 | `asia-southeast1`（新加坡） |
| 临时集群 | `feynman-smoke-260928`，Autopilot |
| GKE 版本 | `1.36.4-gke.1247000` |
| 托管插件 | `addonsConfig.agentSandboxConfig.enabled = true` |
| Sandbox | `agents.x-k8s.io/v1beta1`，`reader-smoke`，`Ready=True` |
| 运行时 | Pod `runtimeClassName=gvisor`；就绪节点带 `sandbox.gke.io/runtime=gvisor` 标签 |
| Workspace | `/workspace`，外部 `standard-rwo` PVC，10 GiB，`Bound` |
| Agent CLI | `codex-cli 0.157.1`，新建 ephemeral 会话 |

使用原生托管 Agent Sandbox，而非普通 Pod 冒充沙盒；启用方式参考
[Google 官方安装说明](https://docs.cloud.google.com/kubernetes-engine/docs/how-to/how-install-agent-sandbox)。
外部 PVC 的用法与官方的
[stateful workspace 模式](https://docs.cloud.google.com/kubernetes-engine/docs/how-to/agent-sandbox-storage)
一致，但本轮只验证挂载与读取，不将其视为恢复测试。

## 验收证据

- Codex 原生事件记录了三个已完成的 `sandbox_smoke` MCP 调用：
  `sandbox_read_file(probe.json)`、`sandbox_list_files()`、`sandbox_read_file(note.txt)`。
- MCP 执行面的独立审计也记录了对应三次成功远程读取。
- Agent 返回的随机标记、消息、Pod hostname 和 `note.txt` 完整内容与验证器一致。
- 返回目录为 `lost+found`、`note.txt`、`probe.json`，与实际 PVC 根目录一致。
- `host_command_count = 0`；测试关闭本地命令工具、浏览器、插件和其他 MCP 连接。
- 随机标记 SHA-256：`f8a8c2d633a56850f62b8eecd3c0e22980e147aec3c26db31d0330a84deda0aa`。
- 标准输出汇总 `passed=true`，完整实时 Smoke 进程退出码为 `0`。

## 权限与安全检查

- Pod 中没有挂载 Kubernetes ServiceAccount token，容器内断言已验证。
- 云凭证保留在宿主，不进入镜像或 Sandbox。
- MCP 使用单独的短期 Kubernetes token，仅获指定 Pod 的 `get` 和 `pods/exec` 权限。
- 使用该 token 创建 Secret 的授权检查返回 `no`；读取未分配 Pod 返回 `Forbidden`。
- 公开 STDIO 边界回归：请求 `../../etc/passwd` 被拒绝，测试通过。
- 已配置 Sandbox ingress/egress deny-all NetworkPolicy；本轮未进行网络逃逸或攻击测试。

注意：`pods/exec` 权限本身并不等于只读文件权限。只读约束由宿主侧 MCP 的
固定命令和白名单实施。这是最小诊断工具，不是已完成的生产多租户安全网关。
未来生产服务必须将工作区分配、凭证和执行边界放在受信执行面中。

## 本轮诊断修正

按 `diagnosing-bugs` 流程使用服务端 dry-run 和真实 CRD 区分初始化问题与 schema 问题：

1. 新集群初始化期间 CRD 和 GKE Warden 尚未就绪：等待，不绕过策略。
2. 当前集群 `v1beta1` 的 `spec` 不包含 `replicas`；实际支持 `operatingMode`。
   替换为 `operatingMode: Running` 后，服务端 dry-run 从失败变为通过。
3. 首次 Agent 验证错误地关闭了 Code Mode 工具执行宿主，导致未发生远程读取，
   Smoke 正确返回失败。保留该必需组件，同时单独关闭本地 shell 后重跑通过。
   官方文档也说明了默认的
   [Code Mode 宿主连接](https://learn.chatgpt.com/docs/app-server#connect-a-remote-code-mode-host)。

按 `tdd` 保留了真实 STDIO 边界的越界读取回归，实时随机标记比对提供端到端验收。
测试没有模拟 GKE 或伪造 Agent 结果。

## 复现与清理

脚本和操作限制见 [Smoke README](../../scripts/smoke/README.md)。

本次测试的临时 namespace、Sandbox、PVC、对应 Compute 磁盘、集群、专用 VPC、
子网和节点服务账号均已删除；专用服务账号的项目 IAM 绑定已移除。
`DELETE_CLUSTER` 操作最终为 `DONE`，项目集群列表、指定 VPC/子网列表均为空；
指定磁盘和服务账号列表也为空，指定成员的项目 IAM 绑定查询无结果。
临时 Pod/PVC 只包含测试数据；删除后不再保留其文件。仓库保留的是无凭证的测试脚本和记录。
临时 admin/reader kubeconfig 均已移除；仅在本机临时目录保留诊断日志和测试文件的 Agent 输出。
GKE API 保持开启，方便后续接入；清理资源不会取消测试期间已经发生的计费。

本机安装了 `gke-gcloud-auth-plugin`。Google 安装器同时更新了其 Python 环境，
并通过 Homebrew 安装 Python 3.13/更新依赖；这些本机工具未回滚。
使用的 kubectl 1.36.4 下载到临时目录并核对了官方 SHA-256，没有替换系统 kubectl。
测试 kubeconfig 使用独立临时路径，没有修改全局 `~/.kube/config`。

下一步才是把已验证的远程访问方式接入 Rust WorkspaceProvider/Agent 工具配置，
然后从真实 Feynman 项目会话完成同样的验收。
