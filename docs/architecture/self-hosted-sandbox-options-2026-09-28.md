# 自托管 Agent Sandbox 选型 · 2026-09-28

范围：使用自有 Spark 主机替代 GKE 的测试执行环境。这里只读检查与研究，不部署，不修改现有宿主服务。

## 结论

优先验证 **OpenSandbox + Docker Engine + 独立持久卷**，由 Feynman 的 Rust 中心通过 Adapter 调用。
如果近期目标改为复用原生 Kubernetes Sandbox/PVC/RuntimeClass 工作流，则选择 **K3s + 上游 Agent Sandbox**。
这两种路径替代的是执行环境，不是 GKE 托管 API、SLA 或多租户安全保证。

## Kubernetes 原生备选

[K3s](https://docs.k3s.io/)是轻量 Kubernetes 发行版；
[官方 requirements](https://docs.k3s.io/installation/requirements)支持 ARM64/aarch64，
自带 containerd 与 local-path provisioner。
[K3s License](https://github.com/k3s-io/k3s/blob/master/LICENSE)为 Apache-2.0。

[上游 Agent Sandbox](https://github.com/kubernetes-sigs/agent-sandbox)提供 Sandbox CRD/controller，
管理有稳定身份的有状态 singleton 工作负载，其 [License](https://github.com/kubernetes-sigs/agent-sandbox/blob/main/LICENSE)
为 Apache-2.0。它是编排器，不自动替代隔离运行时；gVisor/Kata 由 RuntimeClass 等宿主配置提供。
可在自托管 Kubernetes 上搭建原生资源测试环境，但本轮未验证该 controller 的发布镜像在这台 ARM64 主机可运行。

需要自己承担安装升级、RBAC、网络策略、备份和宿主维护。单机 local-path 存储没有 GKE 云磁盘的跨节点迁移能力。
工作卷独立于运行 Pod，确认 PVC/PV reclaim policy，避免删除执行环境时误删项目数据。
当前 GKE 的实际 CRD 与所选上游发布版应分别进行 schema 检查，不能认为所有 manifest 可原样复用。

K3s 的 [NVIDIA runtime 配置说明](https://docs.k3s.io/advanced#nvidia-container-runtime-support)
要求为集群的运行时配置 GPU 支持，Kubernetes 还需 device plugin 等设备资源暴露。
已经安装 Docker 的 Toolkit 不等于 K3s 自带 containerd 已配置好。GB10 的 GPU 任务仍须实际 CUDA 验收。

## 主线补充：推荐 OpenSandbox + Docker，Feynman 保持 Rust

以下是结合实际机器与当前 Feynman 代码的建议，不是已部署方案。

### 机器只读检查

通过现有 SSH/Tailscale 连接检查了 `simonlong@spark-d304.tail242882.ts.net`：

- Ubuntu 24.04.4 LTS，Linux 6.17.0-1014-nvidia，aarch64；20 个逻辑 CPU。
- RAM 总计约 119 GiB，检查时 available 约 97 GiB；根盘约 3.7 TB，空闲约 3.4 TB。
- NVIDIA GB10，驱动 580.142；`nvidia-smi` 的独立显存容量字段为 N/A，不把系统 RAM 当成独立 VRAM。
- Docker Client/Server 29.2.1 可读；NVIDIA Container Toolkit CLI 1.19.0 已安装。
- 当前 Docker runtime 名称为 runc 和 io.containerd.runc.v2，未显示 nvidia；这不是 GPU 容器可用性的完整验证，CDI 等实际路径仍需单独 Smoke。
- `/dev/kvm` 存在，page size 4096；不等于 Kata/GPU 直通已验证。
- 已有其他用途的容器运行，本次未启动新容器、拉镜像、安装软件、改配置或重启 Docker。

### 为什么先推荐 OpenSandbox

[OpenSandbox 官方仓库](https://github.com/opensandbox-group/OpenSandbox)采用
[Apache-2.0](https://github.com/opensandbox-group/OpenSandbox/blob/main/LICENSE)。
其[生命周期模块](https://github.com/opensandbox-group/OpenSandbox/blob/main/docs/architecture/control-plane/server.md)
提供认证 HTTP Interface，背后可选择 Docker 或 Kubernetes。Feynman 的租户、项目、审批、
会话和外部源凭证仍由 Rust 中心管理，OpenSandbox 只作为受信的沙盒实现，不把其平台管理密钥交给 Agent。
无需为了采用它改回 TS；官方 OpenAPI 可由 Rust HTTP client 调用。

[命名卷例子](https://github.com/opensandbox-group/OpenSandbox/blob/main/docs/examples/docker-pvc-volume-mount.md)
把同一 `pvc.claimName` 在 Docker 映射为 named volume，在 Kubernetes 映射为 PVC。
Workspace 卷应在 Sandbox 之外独立管理，显式避免随 Sandbox 删除；
Docker 普通卷不提供 Kubernetes 的容量预留/配额保证。
[Docker 卷生命周期](https://docs.docker.com/engine/storage/volumes/)也独立于容器，
但宿主磁盘仍是单点，需单独备份。

GPU 不是仅在 schema 中出现：
[Docker GPU DeviceRequest 修复 #775](https://github.com/opensandbox-group/OpenSandbox/pull/775)
与 [Kubernetes GPU limits 修复 #782](https://github.com/opensandbox-group/OpenSandbox/pull/782)
均已合并。不过这不证明 GB10 在任意 image/tag/安全运行时下工作。
部署前要选固定发布版本与镜像 digest，验证 ARM64、execd、文件读写、资源上限、GPU
以及恢复行为；不把 main 分支文档的所有特性自动视作所选发布版能力。

许可层面，Linux [Docker Engine](https://docs.docker.com/engine/install/)的开源核心
[Moby](https://github.com/moby/moby/blob/master/LICENSE)及
[NVIDIA Container Toolkit](https://github.com/NVIDIA/nvidia-container-toolkit/blob/main/LICENSE)
为 Apache-2.0，可按许可商用；Docker Desktop 的订阅条款不等于 Linux Engine 的许可。
NVIDIA 驱动/CUDA、所选容器镜像、模型权重各有自己的许可，不能因此宣称整个 GPU 软件栈都为 Apache-2.0。
没有沙盒云管理费不等于零成本：电费、硬件折旧、备份及外部模型费用仍存在。

### 不是 GKE API 模拟器

推荐一个 Feynman 沙盒 Interface，后面接自托管与云端两个 Adapter；
不仿造 GKE 管理 API、IAM、Warden 或云磁盘 CSI。

- 当前自用：OpenSandbox/Docker。
- 需要 Kubernetes 原生资源验证：K3s + upstream Agent Sandbox。
- 云端：GKE 原生 Agent Sandbox（或另行验证 OpenSandbox Kubernetes provider 的 GKE CRD 兼容性）。
- 最少依赖的备用选择：直接 Docker Engine Adapter，经 SSH 访问宿主；需要我们自己维护更多生命周期逻辑。

### 当前 Feynman 的真实接入点

本地代码检查（不是上游能力推断）：

- `crates/feynman-core/src/ports.rs` 中 WorkspaceProvider 目前只返回 String 路径。
- `crates/feynman-server/src/workspace.rs` 创建宿主 tenant/project 目录。
- `crates/feynman-server/src/runtime/codex.rs` 以宿主工作目录启动本机 Agent 子进程，
  MCP 配置还引用本机 Feynman 可执行文件及 SQLite。
- 先前 GKE Smoke 是本地 Agent 经受限 MCP 读取远端文件，不是 Agent 在 Pod 内运行。

因此远端适配至少涉及 Workspace 身份/卷与 AgentRuntime 的执行位置，
以及本地 MCP 到中心 Broker 的连接。不能仅把 String 改成远端路径就算接入。
建议在现有两个 seam 演进：持久化 WorkspaceRef（provider/opaque ID/卷引用/容器内路径）
及可断线重连的运行时 transport，保持上层项目与审批逻辑不依赖 Docker/Kubernetes。

### 长期恢复与安全验收

建议 Generator/Reviewer 各自拥有独立工作卷和会话状态；Reviewer 检查指定 commit 的独立副本，
不与 Generator 并发改同一个可写目录。状态应包括项目/角色到 Workspace、Sandbox、Session 的映射。

[Docker pause](https://docs.docker.com/reference/cli/docker/container/pause/)冻结进程，并不是释放内存的持久化恢复。
[OpenSandbox 运行语义](https://github.com/opensandbox-group/OpenSandbox/blob/main/docs/architecture/control-plane/server.md)
也将 resume 限定为暂停状态，不能拿它重启已经终止的工作负载。
MVP 的 idle 停机方案应是：保存会话状态，停止/销毁容器但保留命名卷，需要时重建 Agent，
从持久化状态接续；不承诺恢复任意进程内存。
容器可写层的 apt/pip 改动不自动跟随卷保留，工具链要固化 image 或另外验证 rootfs snapshot。

Docker runc 共享宿主内核，不等同于 GKE gVisor 隔离；
[Docker 官方安全说明](https://docs.docker.com/engine/security/)要求只允许受信者控制 Docker daemon。
建议非 root、drop capabilities、no-new-privileges、CPU/RAM/PID 限制、最小卷挂载、
独立网络；禁止 Agent 接触 docker.sock、宿主 SSH/DWS/cloud 凭证或任意 host path。
Tailscale 仅提供管理网络，不代替沙盒隔离、租户授权与端口访问控制。
生命周期与执行端口只经私网受控访问，中心 Broker 继续保管外部源凭证。

GPU 默认留给单独受控的推理/计算模块，由中心分配任务；需要直通的 Sandbox 单独授权与验收，
不假定一个 GB10 可以硬隔离给多个租户。gVisor/Kata + ARM64/GB10 组合仍未实际验证。

建议下一轮 Smoke：CPU 沙盒写读文件 → 容器重建后卷恢复 → 真实 Agent 对话/断线接续 →
指定 commit 的 Reviewer → 可选 GPU CUDA 测试。不预报启动秒数；冷拉镜像与缓存热启动要分别实测。
