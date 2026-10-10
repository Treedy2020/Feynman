"""Offline checks for the canonical static documentation reading surface."""
import base64
from html.parser import HTMLParser
from pathlib import Path
import subprocess
import unittest
from urllib.parse import unquote, urlsplit
import zipfile

import build_html


class Page(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.ids = set()
        self.links = []
        self.scripts = []
        self.styles = []
        self.body = {}
        self.h1_count = 0
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if attrs.get("id"):
            self.ids.add(attrs["id"])
        if tag == "h1":
            self.h1_count += 1
        if tag == "body":
            self.body = attrs
        if tag == "script" and attrs.get("src"):
            self.scripts.append(attrs["src"])
        if tag == "link" and attrs.get("rel") == "stylesheet":
            self.styles.append(attrs["href"])
        for attribute in ("src", "href"):
            if attrs.get(attribute):
                self.links.append(attrs[attribute])


class DocumentationTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.pages = {path: Page(path.read_text()) for path in build_html.DOCS.rglob("*.html")}

    def test_generated_html_is_current(self):
        subprocess.run(["python3", str(build_html.ROOT / "scripts/docs/build_html.py"), "--check"], check=True, capture_output=True, text=True)

    def test_all_pages_share_shell_and_single_title(self):
        # HTML-authored designs may grow independently of the generated pages.
        self.assertGreaterEqual(len(self.pages), len(build_html.PAGES) + 1)
        for path, page in self.pages.items():
            # The explicitly standalone UI prototype is not a documentation shell.
            if path == build_html.DOCS / "architecture/prototype-visual-console.html":
                continue
            with self.subTest(page=path.name):
                self.assertEqual(page.h1_count, 1)
                self.assertIn("content", page.ids)
                self.assertIn("data-doc", page.body)
                root = (path.parent / page.body["data-docs-root"]).resolve()
                self.assertEqual(root, build_html.DOCS)
                self.assertEqual([(path.parent / value).resolve() for value in page.styles], [build_html.DOCS / "assets/site.css"])
                self.assertIn(build_html.DOCS / "assets/doc-site.js", [(path.parent / value).resolve() for value in page.scripts])

    def test_local_links_assets_and_anchors_resolve(self):
        for path, page in self.pages.items():
            for value in page.links:
                parsed = urlsplit(value)
                if parsed.scheme or parsed.netloc:
                    continue
                with self.subTest(page=path.name, link=value):
                    target = (path.parent / unquote(parsed.path)).resolve() if parsed.path else path
                    self.assertTrue(target.exists(), str(target))
                    if target.is_relative_to(build_html.DOCS):
                        self.assertNotEqual(target.suffix, ".md", "HTML readers should stay in HTML")
                    if target in self.pages and parsed.fragment and not parsed.fragment.startswith(("node=", "view=")):
                        self.assertIn(unquote(parsed.fragment), self.pages[target].ids)

    def test_sources_and_original_image_are_preserved(self):
        for relative, *_ in build_html.PAGES:
            self.assertTrue((build_html.DOCS / relative).is_file())
        path = build_html.DOCS / "reference/Agent_Web_Application_Platform_Architecture_v0.1.html"
        images = [value for value in self.pages[path].links if value.startswith("data:image/png;base64,")]
        self.assertEqual(len(images), 1)
        with zipfile.ZipFile(path.with_suffix(".docx")) as archive:
            self.assertEqual(base64.b64decode(images[0].split(",", 1)[1]), archive.read("word/media/image1.png"))

    def test_link_rewrite_only_changes_document_reading_targets(self):
        source = build_html.DOCS / "architecture/opensandbox-docker-smoke-2026-09-28.md"
        self.assertEqual(build_html.link_target(source, "../../scripts/smoke/README.md#opensandbox--docker-read-smoke"), "../testing/smoke-guide.html#opensandbox-docker-read-smoke")
        for value in ("https://github.com/example/docs.md", "#ownership", "../../crates/feynman-cli/src/main.rs"):
            self.assertEqual(build_html.link_target(source, value), value)

    def test_no_external_runtime_dependencies(self):
        for path, page in self.pages.items():
            for value in page.scripts + page.styles:
                with self.subTest(page=path.name, asset=value):
                    self.assertFalse(urlsplit(value).scheme or urlsplit(value).netloc)

    def test_system_catalog_and_project_source_groups_are_documented(self):
        path = build_html.DOCS / 'architecture/visual-console.html'
        self.assertIn('integrations', self.pages[path].ids)
        for token in ('source_kinds', 'capability IDs', 'GitLab', 'Slate', 'Drlee',
                      '不按字符串前缀', '不改后台启停', '全局列 Project 与接入能力'):
            self.assertIn(token, path.read_text())

    def test_sidebar_structure_and_active_disclosures(self):
        subprocess.run(["node", str(build_html.ROOT / "scripts/docs/test_navigation.cjs")], check=True, capture_output=True, text=True)

    def test_observation_feed_has_separate_watch_collection_and_delivery_contracts(self):
        path = build_html.DOCS / "architecture/observation-feed.html"
        page = self.pages[path]
        self.assertEqual(page.body["data-doc"], "observation-feed")
        self.assertTrue({"flow","windows","cursors","permissions","cli","verification"}.issubset(page.ids))
        for token in ("sources watch", "feed watch", "不是 SSE", "关闭不 pause", "next_after", "Agent Inbox", "固定开窗"):
            self.assertIn(token,path.read_text())

    def test_edc_evidence_and_unscored_rubric_are_discoverable(self):
        import json
        path = build_html.DOCS / "testing/edc-evaluation-pack.html"
        page = self.pages[path]
        self.assertEqual(page.body["data-doc"], "edc-evaluation")
        self.assertTrue({"runtime", "scope", "snapshot", "collected", "scorecard", "gates", "next"}.issubset(page.ids))
        self.assertIn("edc-evaluation-pack.html", (build_html.DOCS / "assets/doc-site.js").read_text())
        rubric = json.loads((build_html.DOCS / "testing/edc-scorecard-template.json").read_text())
        self.assertTrue(rubric["operator_only"])
        self.assertFalse(rubric["owner_approved"])
        self.assertEqual(rubric["evaluation_status"], "not_run")
        self.assertIsNone(rubric["total"])
        self.assertEqual(sum(d["weight"] for d in rubric["dimensions"]), 100)
        for dimension in rubric["dimensions"]:
            self.assertEqual(sum(i["weight"] for i in dimension["items"]), dimension["weight"])
            self.assertTrue(all(i["rating"] is None for i in dimension["items"]))
        self.assertTrue(all(c["status"] == "pending" for c in rubric["critical_checks"]))

    def test_multi_project_and_permission_design_are_discoverable(self):
        overview = (build_html.DOCS / "index.html").read_text()
        for value in ('data-diagram-view="projects"', 'data-diagram-view="execution"', 'id="projects"', "modules/external-access/index.html"):
            self.assertIn(value, overview)
        access = self.pages[build_html.DOCS / "architecture/external-access-abstraction.html"]
        self.assertEqual(access.body["data-doc"], "access-model")
        self.assertTrue({"objects", "flow", "traits", "boundaries", "migration"}.issubset(access.ids))
        self.assertIn("external-access-abstraction.html", (build_html.DOCS / "assets/doc-site.js").read_text())

    def test_independent_access_module_and_first_batch_are_discoverable(self):
        navigation = (build_html.DOCS / "assets/doc-site.js").read_text()
        diagram = (build_html.DOCS / "assets/architecture.js").read_text()
        for name in ("index", "dws", "gitlab", "git", "slate", "drlee"):
            path = build_html.DOCS / f"modules/external-access/{name}.html"
            self.assertIn(path, self.pages)
            self.assertIn(f"modules/external-access/{name}.html", navigation)
        self.assertIn('data-diagram-view="access"', (build_html.DOCS / "index.html").read_text())
        self.assertIn("const accessPositions", diagram)
        for provider in ("gitlab", "git"):
            self.assertIn(f'details.{provider} =', diagram)

    def test_access_security_strategies_and_related_contracts_are_integrated(self):
        path = build_html.DOCS / "architecture/external-access-abstraction.html"
        content = path.read_text()
        self.assertTrue({"traits", "environment", "subscriptions", "memory"}.issubset(self.pages[path].ids))
        for strategy in ("ExternalAccessProvider", "AuthenticationStrategy", "AccessPolicy", "CredentialStore", "CredentialDeliveryStrategy", "ExternalToolAdapter", "ExecutionPolicy", "AuditSink"):
            self.assertIn(f"<code>{strategy}</code>", content)
        for boundary in ("本轮已实现 Rust 组合策略", "Agent 控制的进程可读取其 env / 文件", "真正短期凭证", "不能把长期 Key", "目标设计", "不保存 Token、密码、完整 env", "暂停 / 隔离", "usage_lease"):
            self.assertIn(boundary, content)
        provider_draft = content.split("pub trait ExternalAccessProvider:", 1)[1].split("pub trait AuthenticationStrategy:", 1)[0]
        self.assertNotIn("begin_auth", provider_draft)
        for relative, anchors in (
            ("index.html", ("traits", "environment", "subscriptions", "memory")),
            ("modules/external-access/index.html", ("traits", "environment", "subscriptions", "memory")),
            ("architecture/local-user-home.html", ("environment", "memory")),
            ("architecture/agent-schedules.html", ("subscriptions", "memory")),
        ):
            source = (build_html.DOCS / relative).read_text()
            for anchor in anchors:
                self.assertIn(f"external-access-abstraction.html#{anchor}", source)
        self.assertIn('doc: "modules/external-access/index.html"', (build_html.DOCS / "assets/architecture.js").read_text())

    def test_native_auth_and_fixed_sandbox_clone_boundaries_are_current(self):
        slate = (build_html.DOCS / 'modules/external-access/slate.html').read_text()
        drlee = (build_html.DOCS / 'modules/external-access/drlee.html').read_text()
        git = (build_html.DOCS / 'modules/external-access/git.html').read_text()
        for value in ('PKCE', 'login-slate', 'import-slate', 'Keychain'):
            self.assertIn(value, slate)
        for value in ('import-drlee', 'login-drlee', 'resource-id', 'refresh rotation'):
            self.assertIn(value, drlee)
        for value in ('git.workspace.clone', 'idempotency_key', 'root', '不自动删除'):
            self.assertIn(value, git)
        overview = build_html.DOCS / 'modules/external-access/index.html'
        self.assertIn('real-smoke', self.pages[overview].ids)
        diagram = (build_html.DOCS / 'assets/architecture.js').read_text()
        self.assertNotIn('目前尚未接入 Drlee', diagram)
        self.assertIn('["slate", 36, 816', diagram)

    def test_local_console_is_implemented_not_the_static_multi_agent_prototype(self):
        path = build_html.DOCS / 'architecture/visual-console.html'
        content = path.read_text()
        for value in ('本机 Console 初版已实现', 'http://127.0.0.1:4318/console/',
                      '持久 Inbox', 'CSRF token', 'CLI 原接口继续拒绝 Origin',
                      '写操作不自动重试', '记忆、环境及租约是只读视图',
                      '多 Agent 是目标模型', '关闭窗口不停止采集'):
            self.assertIn(value, content)
        self.assertTrue({'shell','surfaces','contract','security','phases'}.issubset(self.pages[path].ids))

    def test_agent_schedule_implementation_commands_and_boundaries_are_current(self):
        path = build_html.DOCS / "architecture/agent-schedules.html"
        source = path.read_text()
        self.assertTrue({"model","schedule","delivery","agents","access","cli","migration"}.issubset(self.pages[path].ids))
        for implemented in ("Rust 初版已实现", "--without-agent", "feynman inbox send", "feynman sources captures", "--collect-for-seconds", "steer_if_active", "feynman_observations", "不同时间登记", "source_id + event_id", "unknown"):
            self.assertIn(implemented, source)
        for boundary in ("旧的立即发送入口", "实时空窗自动补查", "通用 MemoryStore", "强租户隔离"):
            self.assertIn(boundary,source)
        self.assertNotIn("尚不支持这些子命令", source)

    def test_environment_lease_memory_contract_is_current_and_discoverable(self):
        path=build_html.DOCS/'architecture/environment-leases-memory.html'
        source=path.read_text()
        self.assertTrue({'architecture','environment','leases','memory','cli','verification'}.issubset(self.pages[path].ids))
        for value in ('usage_lease','provider_credential_lease','unsupported','idempotency_key','expected_revision','feynman_environment','feynman_memory','--stdin','tmpfs','Keychain','强租户隔离','逻辑隐藏','unreachable'):
            self.assertIn(value,source)
        self.assertIn('environment-leases-memory.html',(build_html.DOCS/'assets/doc-site.js').read_text())

    def test_dws_device_content_is_integrated_and_shared_reference_is_separate(self):
        dws = build_html.DOCS / "modules/external-access/dws.html"
        shared = build_html.DOCS / "reference/external-authentication-flow.html"
        legacy = build_html.DOCS / "architecture/dws-device-authorization-v1.html"
        navigation = (build_html.DOCS / "assets/doc-site.js").read_text()
        self.assertEqual(self.pages[dws].body["data-doc"], "dws-module")
        self.assertTrue({"authentication", "cli", "states", "interfaces", "execution", "recovery", "verification"}.issubset(self.pages[dws].ids))
        self.assertEqual(self.pages[shared].body["data-doc"], "auth-flow")
        self.assertTrue({"concepts", "flow", "device", "observation", "revocation", "security", "providers"}.issubset(self.pages[shared].ids))
        self.assertIn("认证与授权流程", navigation)
        self.assertIn("reference/external-authentication-flow.html", navigation)
        self.assertNotIn('"DWS Device 流程"', navigation)
        self.assertEqual(self.pages[legacy].body["data-doc"], "dws-module")
        self.assertTrue((legacy.with_suffix(".md")).is_file())
        self.assertNotIn("enterprise-search adapter still uses", legacy.read_text())
        self.assertNotIn("architecture/dws-device-authorization-v1.md", [item[0] for item in build_html.PAGES])
        source = build_html.DOCS / "architecture/backend-cli-v1.md"
        self.assertEqual(build_html.link_target(source, "dws-device-authorization-v1.md#flow-and-frontend-contract"), "../modules/external-access/dws.html#interfaces")

    def test_iteration_direction_design_is_discoverable_and_plan_only(self):
        path = build_html.DOCS / "testing/iteration-direction.html"
        self.assertEqual(self.pages[path].body["data-doc"], "iteration-direction")
        self.assertTrue({"question", "fixture", "output", "judgment", "cases", "candidates", "scoring", "execution", "integration"}.issubset(self.pages[path].ids))
        for relative in ("assets/doc-site.js", "index.html", "architecture/local-first-architecture.html"):
            self.assertIn("testing/iteration-direction.html", (build_html.DOCS / relative).read_text())
        content = path.read_text()
        for value in ("direction_only", "不要求写代码", "Invalid fixture", "TC-03", "EDC-01", "Cohort-01"):
            self.assertIn(value, content)
        evidence = build_html.DOCS / "testing/historical-source-map.html"
        self.assertEqual(self.pages[evidence].body["data-doc"], "historical-source-map")
        self.assertTrue({"sources", "edc", "cohort", "technical-judgment", "versions", "readiness", "edc-enrollment", "cohort-template"}.issubset(self.pages[evidence].ids))
        self.assertIn("historical-source-map.html", content)
        for value in ("648", "E-01", "C-01", "17:30", "14:05", "超过 3 个参数失败"):
            self.assertIn(value, evidence.read_text())

    def test_direction_evaluation_requires_technical_judgment_without_refusal_bias(self):
        design = (build_html.DOCS / "testing/iteration-direction.html").read_text()
        for value in ("propose_alternative", "recommend_against", "实现困难不自动等于需求不合理", "TC-08", "TC-09", "TC-10", "TC-11", "无依据拒绝率", "重新评估条件"):
            self.assertIn(value, design)
        evidence = (build_html.DOCS / "testing/historical-source-map.html").read_text()
        for value in ("合成变体", "不是真实消息", "不限数量立即并行", "转换值为空", "防止测试只奖励拒绝"):
            self.assertIn(value, evidence)

    def test_direction_runtime_repository_is_seeded_without_future_history(self):
        path = build_html.DOCS / "testing/iteration-direction.html"
        self.assertIn("runtime-repository", self.pages[path].ids)
        design = path.read_text()
        for value in ("git init", "新提交 SHA 不等于原始", "源码包 hash", "没有 remote", "Git LFS", "初始化接口已落地", "不是强制安全边界"):
            self.assertIn(value, design)
        evidence = (build_html.DOCS / "testing/historical-source-map.html").read_text()
        self.assertIn("iteration-direction.html#runtime-repository", evidence)


if __name__ == "__main__":
    unittest.main()
