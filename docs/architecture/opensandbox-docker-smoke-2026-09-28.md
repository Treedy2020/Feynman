# OpenSandbox + Docker：Feynman 文件观测 Smoke

日期：2026-09-28。结论：**真实 Rust CLI、MCP 和 Feynman Main Agent 均已读取到 Spark Docker 沙盒中的文件；替换容器后，同一个 Volume 中的文件仍可读取。**

## 本次实现范围

沿用 `ExternalToolAdapter` 和项目 `Tool Grant`，增加可选 Rust 工具
`sandbox.files.smoke`，MCP 名称为 `sandbox_smoke_files`。
补齐 `POST /v1/projects/{project_id}/external-tools/{tool_id}/invoke` 和
`feynman external-tools invoke … --arguments JSON`。
CLI 授权返回值的包装解析错误一并修复。

```text
CLI / 本地 Main Agent
        │ HTTP / 项目作用域 MCP
        ▼
Feynman Server：授权校验、固定沙盒绑定、服务端持有凭证
        │ SSH 回环隧道 + OpenSandbox 文件 API
        ▼
Spark：OpenSandbox Server → Docker Container → /workspace named volume
```

测试沙盒仅占用 0.5 CPU / 512 MiB，不使用 GPU。运行时为普通 Docker/runc，
不是 gVisor，也没有创建 GKE 资源。原有 `uptime-kuma` 未修改，清理后仍健康运行。

## 已通过的实际验证

- 由独立 operator 向容器写入随机 `marker`，并用 Docker exec 读取校验。
- CLI 列出 `probe.json` / `note.txt`，读取内容与独立 fixture 完全一致。
- 无 Grant：403；撤销 Grant 后：403；路径穿越参数：400。
- 另一个 Project 即使授权了同名工具，也无法访问该沙盒：404。
- 实际 Feynman STDIO MCP 进程完成 tools/list 和 tools/call 文件读取。
- Feynman Main Agent 通过正常消息入口启动，原生事件中有 **3 次完成的
  `sandbox_smoke_files` 调用**，无 Shell 活动；回答中的随机 marker 与
  operator 记录一致，prompt 中没有 marker。
- 删除原容器，创建挂回同一 Volume 的新容器；独立校验及 Feynman 再读均成功。
- OpenSandbox 服务日志记录 **8 次成功的文件 API 请求**，与 CLI、MCP、Agent
  及重建后读取对应；拒绝的调用没有追加文件 API 请求。
- 清理后测试容器与 Volume 为零，远程 18080 无监听；本地测试服务和隧道已停。

完整 Agent Smoke（最终复跑）的临时证据在
`/tmp/feynman-os-smoke-yPXESx/verification.json`、`agent-answer.txt`、
`agent-events.json`；远端日志在 `/tmp/feynman-os-run-vRfd6uxB/server.log`。
marker 仅在临时 fixture/回答中保留，仓库不记录原文或任何密钥。

| 计时 | 实测 |
| --- | --- |
| 创建至 Running + execd ping | 0.78 秒 |
| 新容器挂回已有 Volume 至可用 | 0.35 秒 |
| CLI + MCP + 真实 Agent + 恢复 + 清理 | 19.08 秒 |

以上是暖缓存单次测量，不含 venv 安装、拉镜像及 OpenSandbox 服务部署，
不代表生产 SLO。首次包安装/镜像下载明显更久。

## 凭证与限制

工具只接受 `list` 或 `read` 两种操作，根目录固定为 `/workspace/smoke`，
只能读取两个 dummy 文件。不接受客户端 sandbox ID、URL、宿主路径或命令。
服务端的私有连接 JSON 绑定一个 Tenant / Project / sandbox ID；API key 不进镜像、
容器或 Agent prompt，MCP 只获得配置文件路径。响应上限 64 KiB，禁用重定向和
环境代理，不向客户端转发上游错误正文。

这是 operator 控制的 fixture，不是通用的防符号链接文件网关或多租户隔离审计。
OpenSandbox key 本身有生命周期权限，文件只读限制由 Feynman broker 实施。
当前 DevIdentity 仍只允许本机固定 Tenant，不应对外暴露 Feynman HTTP 服务。

## 实测发现与兼容修正

Spark 的 Docker Hub 连接超时，改用项目的
[官方阿里云镜像源](https://github.com/opensandbox-group/OpenSandbox#official-container-images)。
下载到的 server 1.1.0 wheel 缺少 `fast_sandbox.generated`，启动失败；改为
`opensandbox-server==0.2.3` 和官方 ARM64 `execd:v1.0.22`。
未使用的 v1.1.0 测试镜像已移除，需要时可重新拉取。

0.2.3 缺少可配置的 loopback publish 选项，因此仅在独立测试进程内覆盖
port allocator 常量，并把 metadata 目录指向该次临时目录。
未修改 Docker daemon、已安装包文件或用户全局 Codex 配置。
实际 inspect 校验所有测试容器端口都绑定 `127.0.0.1`。

本机 Codex 为 0.157.1。真实对话测试发现旧 `onRequest`、`readOnlyAccess`
参数不兼容，适配器已按本机生成的 Schema 和
[官方 Permission Profile 文档](https://learn.chatgpt.com/docs/permissions)
改用明确的 `on-request` / `permissions: "feynman_project"`。
只读放行已解析的 Codex 可执行文件以运行其文件系统 helper，并用真实路径启动
以避免 Homebrew symlink 的访问失败。
另用受控 dummy 文件和可达的本地 HTTP 服务验证：项目目录读成功、目录外读被
OS 拒绝、profile 内网络连接失败。不是完整的权限安全审计。

`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、
`cargo fmt --all --check` 通过。废弃的 JSON policy 形状测试随旧接口移除，
实际 Agent 和 native sandbox 命令验证覆盖本次兼容迁移。

## 复跑与下一步

见 [操作说明](../../scripts/smoke/README.md#opensandbox--docker-read-smoke)。
Spark 仅留可复用的独立 venv、v1.0.22 镜像缓存及临时证据文件，没有常驻服务。

本次 Agent 仍运行在本地，只有 Workspace 文件位于远端。恢复是 **文件持久化 +
重建容器 + operator 重新绑定 sandbox ID**，不是进程 checkpoint、自动恢复长期
Agent Session 或 GKE API 的替代实现。下一步应实现正式的 OpenSandbox
WorkspaceProvider 与持久化 Workspace → sandbox/volume 映射，再把 Agent Runtime
迁入沙盒；不需要继续扩展这个临时文件工具。
