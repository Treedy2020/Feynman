#!/usr/bin/env python3
"""Render preserved historical Markdown to the canonical HTML reading surface.

Curated current architecture and user-Home pages are HTML-authored. Existing
Markdown/DOCX sources are retained, not deleted or silently rewritten. Uses Pandoc
to preserve tables, code, reference links and raw HTML from the DOCX conversion.
"""
import argparse
import base64
import html
from html.parser import HTMLParser
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit
import zipfile

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"
PAGES = [
    ("architecture/local-first-main-agent-v1.md", "operations", "CLI 与运行指南", "当前实现", "当前操作说明。HTML 是阅读入口；原始 Markdown 仅保留作兼容来源。"),
    ("architecture/backend-cli-v1.md", "api", "后端与 CLI 契约", "接口演进", "记录原始 Rust slice 及后来追加的接口。专用 Codex Home、远端执行和任务调度，以当前架构与运行指南为准。"),
    ("architecture/native-executor-smoke-2026-09-28.md", "native", "原生 Executor 验证", "历史验证 · 2026-09-28", "历史测试记录，不代表有常驻服务。本轮仅改变阅读界面，没有重测或扩大安全保证。"),
    ("architecture/opensandbox-docker-smoke-2026-09-28.md", "docker", "OpenSandbox 验证", "历史验证 · 2026-09-28", "受限文件读取与恢复 smoke 的历史记录；当前生产代码的 WorkspaceProvider 是后来的实现。"),
    ("architecture/self-hosted-sandbox-options-2026-09-28.md", "options", "自托管环境选型", "历史研究 · 2026-09-28", "保留当时的研究来源和结论。许可、产品能力与版本信息未在本轮重新核验。"),
    ("architecture/gke-sandbox-smoke-2026-09-28.md", "gke", "GKE Sandbox 验证", "历史验证 · 2026-09-28", "历史 GKE 测试集群已销毁；当前 MVP 不依赖 GKE，本页面不表示重新创建或启用计费。"),
    ("architecture/gke-sandbox-model-and-startup.md", "gke-model", "GKE 模型与启动", "历史记录 · 2026-09-28", "单次实测和当时官方资料，不是启动 SLA。文件恢复、进程恢复与 Agent 会话恢复是不同保证。"),
    ("architecture/gke-sandbox-costs-2026-09-28.md", "gke-cost", "GKE 成本快照", "历史报价 · 2026-09-28", "保留当时地区、SKU 和时间边界。不是当前报价；再次购买或开资源前必须重新核价。"),
    ("adr/0001-provider-agnostic-domain-gates.md", "adr1", "001 · Provider 无关的内核", "已被替代", "保留原决策。TypeScript 实现选择已由 ADR-002 替代，不是当前可运行后端。"),
    ("adr/0002-rust-cli-first-multitenant-platform.md", "adr2", "002 · Rust / CLI 优先", "设计决策", "保留当时的中心服务目标。当前是 local-first 开发身份，尚未具备对外多租户认证与生产凭证库。"),
    ("adr/0003-project-scoped-mcp-tool-grants.md", "adr3", "003 · 项目级 MCP 授权", "设计决策", "业务工具权限在每次调用时核对，原生认证和工具 Grant 不混为一谈。"),
    ("reference/Agent_Web_Application_Platform_Architecture_v0.1.md", "reference", "初版产品与架构设计", "原始资料 · v0.1", "原始 DOCX 转换资料，保留全部内容。云发布链路、Strong Reviewer 和原技术选型不表示当前 MVP 已实现。"),
    ("../scripts/smoke/README.md", "smoke-guide", "Smoke 操作指南", "操作与历史验证", "脚本可能创建外部资源或产生模型调用；仅在理解各段作用与清理边界后，显式运行所需的 smoke。"),
]


class Headings(HTMLParser):
    def __init__(self):
        super().__init__()
        self.items = []
        self.active = None

    def handle_starttag(self, tag, attrs):
        if tag == "h2":
            self.active = [dict(attrs).get("id"), ""]

    def handle_data(self, data):
        if self.active is not None:
            self.active[1] += data

    def handle_endtag(self, tag):
        if tag == "h2" and self.active:
            if self.active[0]:
                self.items.append(self.active)
            self.active = None


def link_target(source, value):
    parts = urlsplit(html.unescape(value))
    if parts.scheme or parts.netloc or not parts.path:
        return value
    target = (source.parent / unquote(parts.path)).resolve()
    fragment = parts.fragment
    if target in (DOCS / "architecture/dws-device-authorization-v1.md", DOCS / "architecture/dws-device-authorization-v1.html"):
        destination = DOCS / "modules/external-access/dws.html"
        fragment = {"dws-device-authorization-local-first-v1": "authentication", "policy-and-ownership": "authentication", "flow-and-frontend-contract": "interfaces", "persistence-process-lifecycle-and-credentials": "recovery"}.get(fragment, fragment)
    elif target == ROOT / "scripts/smoke/README.md":
        destination = DOCS / "testing/smoke-guide.html"
    elif target.suffix == ".md" and target.is_relative_to(DOCS):
        destination = target.with_suffix(".html")
    else:
        return value
    import os
    output = DOCS / "testing/smoke-guide.html" if source == ROOT / "scripts/smoke/README.md" else source.with_suffix(".html")
    relative = os.path.relpath(destination, output.parent)
    # GitHub drops '+' while preserving surrounding hyphens; Pandoc collapses
    # those separators. Keep preserved sources unchanged, fix the HTML target.
    fragment = "opensandbox-docker-read-smoke" if fragment == "opensandbox--docker-read-smoke" else fragment
    return html.escape(relative + ("?" + parts.query if parts.query else "") + ("#" + fragment if fragment else ""), quote=True)


def reading_body(body, key):
    """Preserve source wording/anchors without a second page-level title."""
    body = re.sub(r'\A<h1([^>]*)>(.*?)</h1>', r'<p class="source-title"\1>\2</p>', body, count=1, flags=re.S)
    # The original DOCX uses h1 for chapters. Nest them under the page title.
    if re.search(r'<h1(?:\s|>)', body):
        body = re.sub(r'<(/?)h([1-5])(\s[^>]*|)>', lambda m: f'<{m[1]}h{int(m[2]) + 1}{m[3]}>', body)
    if key == "reference":
        with zipfile.ZipFile(DOCS / "reference/Agent_Web_Application_Platform_Architecture_v0.1.docx") as archive:
            original_image = base64.b64encode(archive.read("word/media/image1.png")).decode("ascii")
        body = body.replace('src="media/image1.png"', f' alt="初版架构示意图（历史设计）" src="data:image/png;base64,{original_image}"')
    return body


def wrap(body, key, title, status, note):
    headings = Headings()
    headings.feed(body)
    outline = "".join(f'<a href="#{html.escape(identifier, quote=True)}">{html.escape(label)}</a>' for identifier, label in headings.items)
    historical = " historical" if any(word in status for word in ["历史", "原始", "替代"]) else ""
    current_notice = '<p class="note">2026-10-09 更新：环境、使用租约与长期记忆见 <a href="environment-leases-memory.html">当前资源模块契约</a>。本页保留既有兼容入口说明。Agent CRUD、独立来源历史、固定窗口与 Inbox 的当前可运行命令见 <a href="agent-schedules.html#cli">Agent 与 Schedule</a>；旧 send / messages 仍为立即启动入口，不提供 Inbox 排队。</p>' if key in ("operations", "api") else ""
    return f'''<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <meta name="description" content="{html.escape(note, quote=True)}">
  <title>Feynman Docs · {html.escape(title)}</title>
  <link rel="icon" href="data:,">
  <link rel="stylesheet" href="../assets/site.css">
  <script src="../assets/doc-site.js" defer></script>
</head>
<body data-doc="{key}" data-docs-root="..">
<div class="page">
  <header class="topbar"></header>
  <main id="content">
    <section class="hero"><span class="doc-status{historical}">{html.escape(status)}</span><h1>{html.escape(title)}</h1><p class="lead">{html.escape(note)}</p>{current_notice}</section>
{f'    <nav class="doc-outline" aria-label="本页章节">{outline}</nav>' if outline else ''}
    <article class="doc-body">{body}</article>
  </main>
  <footer><a href="../index.html">← 架构总览</a><span>Feynman Docs · HTML 阅读入口 / 2026-09-29</span></footer>
</div>
</body>
</html>
'''


def write_or_check(path, value, check):
    if check:
        if not path.is_file() or path.read_text() != value:
            raise SystemExit(f"HTML out of date: {path.relative_to(ROOT)}")
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for path, key, title, status, note in PAGES:
        source = (DOCS / path).resolve()
        body = subprocess.run(["pandoc", "--from=markdown", "--to=html5", "--wrap=none", str(source)], capture_output=True, text=True, check=True).stdout
        # Preserve source facts and escaped code; rewrite only reading links.
        body = re.sub(r'href="([^"]+)"', lambda m: 'href="' + link_target(source, m[1]) + '"', body)
        # Pandoc keeps script links relative to the original source; the smoke
        # guide is now inside docs/testing. Prefix remaining relative paths.
        if key == "smoke-guide":
            def rebase(match):
                value = html.unescape(match[1])
                parts = urlsplit(value)
                if parts.scheme or parts.netloc or not parts.path or parts.path.endswith(".html"):
                    return match[0]
                import os
                target = (source.parent / unquote(parts.path)).resolve()
                relative = os.path.relpath(target, DOCS / "testing")
                return 'href="' + html.escape(relative + ("#" + parts.fragment if parts.fragment else ""), quote=True) + '"'
            body = re.sub(r'href="([^"]+)"', rebase, body)
        output = DOCS / "testing/smoke-guide.html" if key == "smoke-guide" else source.with_suffix(".html")
        write_or_check(output, wrap(reading_body(body, key), key, title, status, note), args.check)
    source = DOCS / "architecture/local-first-architecture.html"
    index = source.read_text().replace('../assets/', 'assets/').replace('data-docs-root=".."', 'data-docs-root="."').replace('href="../architecture/', 'href="architecture/').replace('href="../modules/', 'href="modules/').replace('href="../reference/', 'href="reference/').replace('href="../testing/', 'href="testing/')
    write_or_check(DOCS / "index.html", index, args.check)
    print(f"{'Checked' if args.check else 'Built'} {len(PAGES) + 1} HTML pages; preserved source files")


if __name__ == "__main__":
    main()
