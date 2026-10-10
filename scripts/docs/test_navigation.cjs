/* Offline DOM contract checks for the shared sidebar; no browser dependency. */
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

class Element {
  constructor(tag, text = "") {
    this.tag = tag;
    this.textContent = text;
    this.children = [];
    this.attributes = {};
    this.style = {};
    this.classList = { add() {}, toggle() {}, remove() {}, contains() { return false; } };
  }
  append(...children) { this.children.push(...children); }
  prepend(...children) {
    this.children = this.children.filter(child => !children.includes(child));
    this.children.unshift(...children);
  }
  replaceChildren(...children) { this.children = children; }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener() {}
}

const source = fs.readFileSync(path.join(__dirname, "../../docs/assets/doc-site.js"), "utf8");
const walk = node => [node, ...node.children.flatMap(walk)];
function render(current) {
  const body = new Element("body");
  body.dataset = { docsRoot: ".", doc: current };
  const main = new Element("main");
  const header = new Element("header");
  const document = {
    body, documentElement: { dataset: {} }, baseURI: "file:///docs/index.html",
    createElement: tag => new Element(tag),
    querySelector: selector => selector === ".topbar" ? header : main,
    querySelectorAll: () => [], addEventListener() {}
  };
  const window = {};
  vm.runInNewContext(source, {
    document, window, URL, navigator: {},
    localStorage: { getItem: () => null },
    matchMedia: () => ({ matches: false, addEventListener() {} })
  });
  const sidebar = body.children.find(node => node.id === "docs-navigation");
  return { sidebar, header, groups: window.FeynmanDocs.groups, nodes: walk(sidebar) };
}

const overview = render("overview");
assert.deepEqual(Array.from(overview.groups, group => group[0]), [
  "开始使用", "核心架构", "外部权限", "测试与验证", "设计决策", "历史归档"
]);
for (const id of ["sidebar-testing", "sidebar-decisions", "sidebar-history", "sidebar-providers", "sidebar-sandbox-history", "sidebar-design-history"]) {
  assert.equal(overview.nodes.find(node => node.id === id).open, false, `${id} defaults closed`);
}
const entries = Array.from(overview.groups).flatMap(group => Array.from(group[1]));
const keys = entries.map(entry => entry[0]);
assert.equal(new Set(keys).size, keys.length, "one location per document");
assert.equal(keys.length, 34, "existing entries preserved plus the Cohort workbench report");
assert.ok(keys.includes("cohort-workbench"), "Cohort verification is discoverable in the docs");
for (const [key, , target] of entries) {
  assert.ok(fs.existsSync(path.resolve(__dirname, "../../docs", target)), target);
  const page = render(key);
  const active = page.nodes.filter(node => node.attributes["aria-current"] === "page");
  assert.equal(active.length, 1, `${key}: single active page`);
  const visit = (node, parents) => {
    if (node === active[0]) {
      for (const parent of parents.filter(parent => parent.tag === "details")) {
        assert.equal(parent.open, true, `${key}: active ancestors expanded`);
      }
    }
    node.children.forEach(child => visit(child, [...parents, node]));
  };
  visit(page.sidebar, []);
}
assert.ok(render("gke").header.children.some(node => node.className === "breadcrumbs" && node.children.some(child => child.textContent === "沙盒选型与技术验证")));
assert.ok(overview.groups.find(group => group[0] === "测试与验证")[1].every(entry => entry[2].startsWith("testing/")));
console.log("Sidebar checks passed: 34 entries, default collapse, active expansion, nested breadcrumbs, local targets.");
