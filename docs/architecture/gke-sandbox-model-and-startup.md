# GKE Sandbox 模型与启动（2026-09-28）

- Sandbox 管单个有状态 Pod；GKE 要求 gVisor，PVC 持久化文件。[概念][model]、[部署][deploy]、[存储][storage]。
- direct creation 不用池；SandboxTemplate 定义配置，SandboxWarmPool 保有就绪实例，SandboxClaim 领取。v1beta1 Claim 指向 WarmPool，旧版指向 Template。[部署/迁移][deploy]
- 官方 warm <1s 指预热实例分配，不是冷启动或完整 E2E 承诺。[Warm Pools][model]
- 直接创建的 stateful 环境需 PVC attach/mount；官方称磁盘附加有多秒延迟，故 E2E 可能更长（推论）。池中绑定存储另需自定义脚本与特权 DaemonSet。[存储][storage]
- PVC 恢复文件不等于恢复进程/Agent 会话（推论）；内存状态恢复另涉及 Pod snapshots。[存储][storage]、[快照][snapshots]。
- [smoke][smoke] 与[配置][fixture]：直接 Sandbox+gVisor+PVC；Agent 运行时仍在宿主，未放进沙盒，无 warm pool；仅验证读取，未测 warm pool/PVC 恢复。

## 单次实测

来源：主 Agent 转交的上一轮工具现场输出；本次未重测。

| 阶段 | 现场证据与计时边界（UTC） | 耗时 |
| --- | --- | --- |
| 集群创建 | `gcloud CREATE_CLUSTER` operation `operation-1790587080710-16f97cff-4ff9-41c8-a346-5fc228e20a4d`，只读 describe 返回 `DONE`；startTime `2026-09-28T09:18:00.710692424Z` → endTime `2026-09-28T09:25:56.163722182Z` | 7分55.453秒 |
| 控制器运行态 → Ready | 先前 `kubectl get Sandbox.status`：NotSuspended transition `2026-09-28T09:38:42Z` → Ready=True transition `2026-09-28T09:41:19Z` | 约157秒（约2分40秒） |

第二行包含首次 gVisor 节点扩容、镜像拉取及新建 10Gi PVC；缺 `metadata.creationTimestamp` 存档，不能称精确 API 创建到 Ready。单次样本非 SLA，不含调试和用户中断，也不是 warm pool/PVC 恢复实测。集群已销毁，不重建重复测试。

[model]: https://docs.cloud.google.com/kubernetes-engine/docs/concepts/machine-learning/agent-sandbox
[deploy]: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/how-install-agent-sandbox
[storage]: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/agent-sandbox-storage
[snapshots]: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/agent-sandbox-pod-snapshots
[smoke]: ./gke-sandbox-smoke-2026-09-28.md
[fixture]: ../../scripts/smoke/gke_sandbox.yaml
