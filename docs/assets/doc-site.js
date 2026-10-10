/* Shared local-file-compatible shell. Deliberately no fetch, CDN or analytics. */
(() => {
  "use strict";
  const root = new URL((document.body.dataset.docsRoot || ".") + "/", document.baseURI);
  const current = document.body.dataset.doc || "overview";
  const groups = [
    ["开始使用", [
      ["overview", "架构总览", "index.html"],
      ["operations", "CLI 与运行指南", "architecture/local-first-main-agent-v1.html"],
      ["local-docker", "本机 Docker Workspace", "architecture/local-docker-workspace.html", "初版"]
    ]],
    ["核心架构", [
      ["home", "本地 Home 与会话", "architecture/local-user-home.html"],
      ["api", "后端与 CLI 契约", "architecture/backend-cli-v1.html"],
      ["agent-schedules", "Agent 与 Schedule", "architecture/agent-schedules.html", "初版"],
      ["observation-feed", "事实流与消息窗口", "architecture/observation-feed.html", "初版"],
      ["resources", "环境、租约与记忆", "architecture/environment-leases-memory.html", "初版"],
      ["visual-console", "可视化工作台", "architecture/visual-console.html", "设计"]
    ]],
    ["外部权限", [
      ["access", "模块总览与 V1 能力", "modules/external-access/index.html"],
      ["access-model", "认证与安全策略", "architecture/external-access-abstraction.html", "设计"],
      ["dws-module", "DWS · 事实源", "modules/external-access/dws.html"],
      ["gitlab-module", "GitLab · 协作", "modules/external-access/gitlab.html"],
      ["git-module", "Git · 远端访问", "modules/external-access/git.html"],
      ["slate-module", "Slate · 项目事实", "modules/external-access/slate.html", "初版"],
      ["drlee-module", "Drlee · 观测渠道", "modules/external-access/drlee.html", "初版"],
      ["auth-flow", "认证与授权流程", "reference/external-authentication-flow.html"]
    ], { submenus: [{ id: "sidebar-providers", label: "接入渠道 / CLI", keys: ["dws-module", "gitlab-module", "git-module", "slate-module", "drlee-module"] }] }],
    ["测试与验证", [
      ["iteration-direction", "迭代方向测试设计", "testing/iteration-direction.html", "设计"],
      ["code-iteration", "代码迭代与 MR 数据集", "testing/code-iteration.html", "初版"],
      ["historical-source-map", "事实源与版本映射", "testing/historical-source-map.html"],
      ["edc-evaluation", "EDC 证据与评分", "testing/edc-evaluation-pack.html"],
      ["cohort-workbench", "Cohort 本地工作台验证", "testing/cohort-workbench-20261009.html"],
      ["mvp-validation", "MVP 验证报告", "testing/mvp-validation-20260930.html"],
      ["smoke-guide", "Smoke 操作指南", "testing/smoke-guide.html"]
    ], { collapsed: true, id: "sidebar-testing" }],
    ["设计决策", [
      ["adr2", "002 · Rust / CLI 优先", "adr/0002-rust-cli-first-multitenant-platform.html"],
      ["adr3", "003 · 项目级 MCP 授权", "adr/0003-project-scoped-mcp-tool-grants.html"]
    ], { collapsed: true, id: "sidebar-decisions" }],
    ["历史归档", [
      ["native", "原生 Executor 验证", "architecture/native-executor-smoke-2026-09-28.html"],
      ["docker", "OpenSandbox 验证", "architecture/opensandbox-docker-smoke-2026-09-28.html"],
      ["options", "自托管环境选型", "architecture/self-hosted-sandbox-options-2026-09-28.html"],
      ["gke", "GKE Sandbox 验证", "architecture/gke-sandbox-smoke-2026-09-28.html"],
      ["gke-model", "GKE 模型与启动", "architecture/gke-sandbox-model-and-startup.html"],
      ["gke-cost", "GKE 成本快照", "architecture/gke-sandbox-costs-2026-09-28.html"],
      ["evolution", "运行时早期设计", "architecture/agent-runtime-workspace.html"],
      ["adr1", "001 · 早期 Provider 内核", "adr/0001-provider-agnostic-domain-gates.html", "已替代"],
      ["reference", "初版产品与架构设计", "reference/Agent_Web_Application_Platform_Architecture_v0.1.html"]
    ], { collapsed: true, id: "sidebar-history", submenus: [
      { id: "sidebar-sandbox-history", label: "沙盒选型与技术验证", keys: ["native", "docker", "options", "gke", "gke-model", "gke-cost"] },
      { id: "sidebar-design-history", label: "早期设计与原始资料", keys: ["evolution", "adr1", "reference"] }
    ] }]
  ];
  const all = groups.flatMap(([, items]) => items);
  const entry = all.find(([key]) => key === current) || all[0];
  const currentGroup = groups.find(([, items]) => items.includes(entry));
  const category = currentGroup[0];
  const currentSubmenu = currentGroup[2]?.submenus?.find(({ keys }) => keys.includes(entry[0]));
  const element = (tag, className, text) => {
    const el = document.createElement(tag);
    if (className) el.className = className;
    if (text !== undefined) el.textContent = text;
    return el;
  };
  const skip = element("a", "skip-link", "跳至正文");
  skip.href = "#content";
  document.body.prepend(skip);
  const sidebar = element("aside", "doc-sidebar");
  sidebar.id = "docs-navigation";
  sidebar.setAttribute("aria-label", "Feynman 文档目录");
  const brand = element("a", "sidebar-brand");
  brand.href = new URL("index.html", root).href;
  brand.append(element("span", "brand-symbol", "F"), element("span", "", "Feynman Docs"));
  sidebar.append(brand);
  const makeLink = ([key, label, path, hint]) => {
    const link = element("a", "sidebar-link");
    link.href = new URL(path, root).href;
    link.append(element("span", "", label));
    if (hint) link.append(element("span", "sidebar-hint", hint));
    if (key === current) link.setAttribute("aria-current", "page");
    return link;
  };
  const disclosure = (id, label, active) => {
    const details = element("details", "sidebar-disclosure");
    details.id = id;
    details.open = active;
    details.append(element("summary", "sidebar-summary", label));
    return details;
  };
  for (const [name, items, options = {}] of groups) {
    const section = element("nav", "sidebar-section");
    section.setAttribute("aria-label", name);
    let container = section;
    if (options.collapsed) {
      container = disclosure(options.id, name, items.some(([key]) => key === current));
      container.classList.add("sidebar-group-disclosure");
      section.append(container);
    } else section.append(element("h2", "", name));
    const childLists = new Map();
    for (const item of items) {
      const submenu = options.submenus?.find(({ keys }) => keys.includes(item[0]));
      if (submenu) {
        let children = childLists.get(submenu.id);
        if (!children) {
          const parent = disclosure(submenu.id, submenu.label, submenu.keys.includes(current));
          children = element("ul", "sidebar-sublist");
          childLists.set(submenu.id, children);
          parent.append(children);
          container.append(parent);
        }
        const child = element("li", "");
        child.append(makeLink(item));
        children.append(child);
      } else container.append(makeLink(item));
    }
    sidebar.append(section);
  }
  sidebar.append(element("div", "sidebar-bottom", "Local-first · Rust · 多项目管理\nV1 单 Agent / 下一阶段投递设计"));
  document.body.prepend(sidebar);
  const backdrop = element("div", "mobile-backdrop");
  backdrop.hidden = true;
  document.body.append(backdrop);
  let header = document.querySelector(".topbar");
  if (!header) {
    header = element("header", "topbar");
    document.querySelector(".page").prepend(header);
  }
  header.replaceChildren();
  const menu = element("button", "icon-button menu-button", "☰");
  menu.type = "button";
  menu.setAttribute("aria-label", "打开文档目录");
  menu.setAttribute("aria-controls", sidebar.id);
  menu.setAttribute("aria-expanded", "false");
  const crumb = element("div", "breadcrumbs");
  const trail = [category, ...(currentSubmenu ? [currentSubmenu.label] : []), entry[1]];
  for (const [index, label] of trail.entries()) {
    if (index) crumb.append(element("span", "", "/"));
    crumb.append(element("span", "", label));
  }
  const actions = element("div", "header-actions");
  const theme = element("button", "icon-button", "◐");
  theme.type = "button";
  function applyTheme(value) {
    document.documentElement.dataset.theme = value;
    theme.setAttribute("aria-label", value === "dark" ? "切换浅色主题" : "切换深色主题");
    theme.setAttribute("aria-pressed", String(value === "dark"));
  }
  let saved;
  try { saved = localStorage.getItem("feynman-docs-theme"); } catch (_) { /* private/file mode */ }
  applyTheme(saved === "dark" || saved === "light" ? saved : matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  theme.addEventListener("click", () => {
    const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    applyTheme(next);
    try { localStorage.setItem("feynman-docs-theme", next); } catch (_) { /* optional */ }
  });
  actions.append(theme);
  header.append(menu, crumb, actions);
  const mobile = matchMedia("(max-width: 820px)");
  sidebar.inert = mobile.matches;
  const setMenu = (open) => {
    document.body.classList.toggle("menu-open", open);
    backdrop.hidden = !open;
    sidebar.inert = mobile.matches && !open;
    menu.setAttribute("aria-expanded", String(open));
    menu.setAttribute("aria-label", open ? "关闭文档目录" : "打开文档目录");
    if (open) (sidebar.querySelector("[aria-current]") || brand).focus();
    else menu.focus();
  };
  menu.addEventListener("click", () => setMenu(!document.body.classList.contains("menu-open")));
  mobile.addEventListener("change", () => {
    document.body.classList.remove("menu-open");
    backdrop.hidden = true;
    menu.setAttribute("aria-expanded", "false");
    sidebar.inert = mobile.matches;
  });
  backdrop.addEventListener("click", () => setMenu(false));
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && document.body.classList.contains("menu-open")) setMenu(false);
    if (event.key === "Tab" && document.body.classList.contains("menu-open")) {
      // Closed native disclosures must not put hidden links into the focus loop.
      const controls = [...sidebar.querySelectorAll("a, summary")].filter(node => node.getClientRects().length);
      if (event.shiftKey && document.activeElement === controls[0]) { event.preventDefault(); controls.at(-1).focus(); }
      else if (!event.shiftKey && document.activeElement === controls.at(-1)) { event.preventDefault(); controls[0].focus(); }
    }
  });
  document.querySelector("main").id = "content";
  for (const table of document.querySelectorAll("table")) {
    if (!table.parentElement.classList.contains("table-wrap")) {
      const wrapper = element("div", "table-wrap");
      table.before(wrapper); wrapper.append(table);
    }
  }
  for (const pre of document.querySelectorAll("pre:not([data-no-copy])")) {
    const text = pre.textContent;
    const copy = element("button", "copy-code", "复制");
    copy.type = "button";
    copy.setAttribute("aria-label", "复制代码块");
    copy.addEventListener("click", async () => {
      try {
        if (navigator.clipboard && window.isSecureContext) await navigator.clipboard.writeText(text);
        else {
          const input = element("textarea", "");
          input.value = text;
          input.style.position = "fixed";
          input.style.opacity = "0";
          document.body.append(input); input.select();
          const ok = document.execCommand("copy"); input.remove();
          if (!ok) throw new Error("copy unavailable");
          copy.focus();
        }
        copy.textContent = "已复制";
      } catch (_) { copy.textContent = "请手动复制"; }
      setTimeout(() => { copy.textContent = "复制"; }, 2000);
    });
    pre.append(copy);
  }
  window.FeynmanDocs = { root, groups, url: (path) => new URL(path, root).href };
  document.body.prepend(skip);
})();
