# GKE Sandbox 成本 · 2026-09-28

范围：Singapore `asia-southeast1`、general-purpose Autopilot、USD 按需；只读官方资料，无云资源、计费或权限变更。官网默认 Iowa 价不可代替 Singapore。

| 资源 | USD 单价 | 官方 Catalog 证据（主线转交） |
| --- | --- | --- |
| CPU | $0.0549/vCPU·h | [GKE API][catalog]；`589B-F840-EC19`，54900 nanos/mCPU·h × 1000 |
| 内存 | $0.0060729/GiB·h | [GKE API][catalog]；`6500-7637-6ACA`，6072900 nanos/GiB·h |
| ephemeral | 实际适用价未知 | `3577-D694-91BC`：$0.0000676/GiB·h；尚未核对实际存储类型，不纳入估算 |
| zonal pd-balanced | $0.11/GiB·月 | [Disk API][disk-catalog]；`16D2-FC49-A127`，110000000 nanos/GiB·月；10Gi=$1.10/月、50Gi=$5.50/月 |

CPU/内存/磁盘 API `effectiveTime=2026-09-28T07:00:00Z`，SKU 地区为 `asia-southeast1`；磁盘 SKU 描述为 `Balanced PD Capacity in Singapore`，来自第一页。

[原生 Agent Sandbox][sandbox]（要求 gVisor）无附加费。warm pool 保持 Pod 运行，idle 也按 requests 收费（由运行规则推得）。

[普通 Pod 计费][pricing]逐秒、无最低时长：ContainerCreating/Running 收费；终止或待调度不收费。未修改系统 DaemonSet、OS 开销、未分配节点容量免费。指定机型/GPU 等例外按整节点 Compute Engine 价＋管理溢价，停 Pod 不等于停节点费。

管理费 $0.10/集群·h，730h 毛额 $73；每 billing account 每月 $74.40 共享抵扣，仅 Autopilot/zonal Standard 管理费；Regional Standard 不适用，也不能抵计算或磁盘费。符合条件且额度未占用时，管理费可抵至 $0。[规则][pricing]

[standard-rwo][csi] 使用 pd-balanced。关闭 Sandbox 后独立 PVC/磁盘若保留，仍按全部预配容量收费，直到磁盘删除。[磁盘规则][disk]

因此，普通 Pod 终止、保留 10Gi PVC 和集群：$1.10/月磁盘费＋未抵扣管理费。据[smoke 记录][smoke]，本次临时集群和 PVC 已删除。

[YAML][fixture] requests=250m/512Mi、limits=500m/1Gi、PVC=10Gi；实际 Pod requests 未存档，GKE 可能调整。ephemeral 未声明，官方默认普通容器 1Gi，实际待核。[默认规则][requests]

以下假设整月运行 730h、表中数值即实际计费 requests；仅 CPU＋内存，另加 PVC、ephemeral 和未抵扣管理费，非 smoke 实账。

| vCPU / 内存 GiB | USD/730h |
| --- | ---: |
| 0.5 / 1 | $24.47 |
| 1 / 2 | $48.94 |
| 2 / 4 | $97.89 |

[catalog]: https://cloudbilling.googleapis.com/v1/services/CCD8-9BF1-090E/skus?currencyCode=USD&pageSize=5000
[disk-catalog]: https://cloudbilling.googleapis.com/v1/services/6F81-5844-456A/skus?currencyCode=USD&pageSize=5000
[pricing]: https://cloud.google.com/kubernetes-engine/pricing
[disk]: https://cloud.google.com/compute/disks-image-pricing
[sandbox]: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/how-install-agent-sandbox
[csi]: https://docs.cloud.google.com/kubernetes-engine/docs/how-to/persistent-volumes/gce-pd-csi-driver
[requests]: https://docs.cloud.google.com/kubernetes-engine/docs/concepts/autopilot-resource-requests
[fixture]: ../../scripts/smoke/gke_sandbox.yaml
[smoke]: ./gke-sandbox-smoke-2026-09-28.md
