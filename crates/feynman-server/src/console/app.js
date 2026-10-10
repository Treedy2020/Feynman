// Local Console is a browser adapter, not a second scheduler or credential store.
import {readSse} from './stream.js';
const $ = (s, root = document) => root.querySelector(s);
const esc = v => String(v ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const pretty = v => JSON.stringify(v, null, 2);
const date = v => v ? new Date(v).toLocaleString('zh-CN', {month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit'}) : '—';
const safeId = v => /^[a-zA-Z0-9_.-]+$/.test(v || '') ? v : '';
const names = {overview:'项目概览',chat:'Agent 对话',agents:'Agent 设置',tasks:'任务与运行',schedules:'Schedule',sources:'观测消息',inbox:'Agent 收件箱',workspace:'Workspace',access:'外部授权',memory:'长期记忆',environment:'环境与租约'};
const icons = {overview:'◫',chat:'◇',agents:'◎',tasks:'≡',schedules:'◷',sources:'⌁',inbox:'↓',workspace:'▱',access:'⊙',memory:'▤',environment:'⌘'};
const labels = {active:'已启用',paused:'已暂停',archived:'已归档',running:'执行中',waiting_approval:'等待审批',completed:'已完成',failed:'失败',interrupted:'已中断',queued:'排队中',pending:'待投递',dispatching:'投递中',accepted:'已接收',unknown:'结果未知',cancelled:'已取消',authorized:'认证通过',waiting_for_user:'等待用户授权',preparing:'准备中',expired:'已过期',authentication_required:'需重新认证',stopped:'已停止',ready:'就绪',exhausted:'已结束',inference:'推断',fact:'事实',preference:'偏好',decision:'决策'};
const badge = (s, label) => `<span class="badge ${['active','running','completed','authorized','ready','accepted'].includes(s) ? 'good' : ['waiting_approval','waiting_for_user','pending','unknown','expired','authentication_required'].includes(s) ? 'warning' : ['failed','interrupted'].includes(s) ? 'bad' : ''}">${esc(label || labels[s] || s || '未配置')}</span>`;
const button = (label, action, extra = '', cls = '') => `<button class="button ${cls}" data-action="${action}" ${extra}>${label}</button>`;
const empty = (title, detail = '') => `<div class="empty"><h3>${esc(title)}</h3>${esc(detail)}</div>`;
const heading = (title, detail, actions = '', scope = 'PROJECT WORKSPACE') => `<div class="page-heading"><div><div class="eyebrow">${esc(scope)}</div><h1>${esc(title)}</h1><p>${esc(detail)}</p></div><div class="heading-actions">${actions}</div></div>`;
const panel = (title, body, actions = '') => `<section class="panel"><div class="panel-heading"><h2>${title}</h2>${actions}</div><div class="panel-body">${body}</div></section>`;
const kv = (key, value) => `<div class="kv"><dt>${esc(key)}</dt><dd>${value}</dd></div>`;
const state = {token:null, route:{}, generation:0, busy:false, refreshing:false, data:{}, projects:[], catalog:{providers:[],connections:[]}, events:[], run:null, cursor:0, feed:[], feedAfter:0, feedMore:false, feedSignature:'', drafts:new Map(), updated:null, pending:[], notifications:[], unread:0, notificationAfter:null, projectLive:false,streamRevision:0};
let noticeTimer;
function notice(message, error = false) {
  clearTimeout(noticeTimer); $('#notice').textContent = message; $('#notice').className = 'notice' + (error ? ' error' : ''); $('#notice').hidden = false;
  noticeTimer = setTimeout(() => $('#notice').hidden = true, error ? 18000 : 5000);
}
async function session() {
  const r = await fetch('/console/session', {cache:'no-store', credentials:'omit'});
  if (!r.ok) throw new Error('无法连接同源 Console。请使用本机页面地址。');
  state.token = (await r.json()).csrf_token;
}
async function api(path, method = 'GET', body = undefined) {
  if (!state.token) await session();
  const controller = new AbortController(), timer = setTimeout(() => controller.abort(), method === 'GET' ? 20000 : 120000);
  let r;
  try {
    r = await fetch('/console/api/v1/' + path, {method,cache:'no-store',credentials:'omit',signal:controller.signal,
      headers:{'x-feynman-console':state.token,...(method !== 'GET' ? {'content-type':'application/json'} : {})},
      body:method === 'GET' ? undefined : JSON.stringify(body ?? {})});
  } catch (_) {
    throw new Error(method === 'GET' ? '连接中断，正在等待本机服务恢复。' : '操作结果未知。请刷新并核对记录，不要直接重复提交；页面不会自动重试写操作。');
  } finally { clearTimeout(timer); }
  let v = {}; if (r.status !== 204) { try { v = await r.json(); } catch (_) { throw new Error('服务返回了非预期内容。请刷新后核对状态。'); } }
  if (!r.ok) {
    if (r.status === 403 && /Console/.test(v.error?.message || '')) { state.token = null; throw new Error('Console 安全会话已更新，请刷新页面后操作。'); }
    const error=new Error(typeof v.error === 'string' ? v.error : v.error?.message || `请求失败 (${r.status})`);
    error.rejected=r.status>=400 && r.status<500;throw error;
  }
  state.updated = new Date(); $('#connection').classList.remove('offline'); $('#connection').innerHTML = '<i></i>本机已连接';
  return v;
}
const base = () => 'projects/' + state.route.project;
const resource = (operation, args = {}) => api(base() + '/resources', 'POST', {operation, arguments:args});
function href(view = 'overview', extra = {}) { return '#' + new URLSearchParams({project:state.route.project,view,...extra}); }
function navigate(project, view = 'overview', extra = {}) { location.hash = new URLSearchParams({project,view,...extra}).toString(); }
function parseRoute() {
  const q = new URLSearchParams(location.hash.slice(1)), project = safeId(q.get('project')), provider = safeId(q.get('provider'));
  return project ? {project,view:Object.hasOwn(names, q.get('view')) ? q.get('view') : 'overview',source:safeId(q.get('source')),provider,live:q.get('live') === '1'} : ['integrations','notifications'].includes(q.get('view')) ? {view:q.get('view'),provider} : {};
}
function navigation() {
  const p = state.data.status?.project;
  const catalog = state.route.view === 'integrations';
  $('#navigation').innerHTML = `<a class="nav-item ${!state.route.project && !state.route.view ? 'active' : ''}" href="#" aria-label="所有项目"><span class="nav-icon" aria-hidden="true">▦</span><span class="nav-text">所有项目</span></a><a class="nav-item ${catalog ? 'active' : ''}" href="#view=integrations" aria-label="接入能力"><span class="nav-icon" aria-hidden="true">⌁</span><span class="nav-text">接入能力</span></a><a class="nav-item ${state.route.view==='notifications'?'active':''}" href="#view=notifications" aria-label="消息中心"><span class="nav-icon" aria-hidden="true">♧</span><span class="nav-text">消息中心 ${state.unread?`<span class="notification-count">${state.unread}</span>`:''}</span></a>`;
  $('#project-navigation').innerHTML = state.route.project ? `<div class="project-nav-label">${esc(p?.name || '项目')}</div>` + Object.entries(names).map(([id,title]) => `<a class="nav-item ${state.route.view === id ? 'active' : ''}" href="${href(id)}" title="${title}" aria-label="${title}"><span class="nav-icon" aria-hidden="true">${icons[id]}</span><span class="nav-text">${title}</span></a>`).join('') : '';
  $('#breadcrumb').innerHTML = state.route.project ? `<a href="#">项目</a><span>/</span><a href="${href()}">${esc(p?.name || '加载中')}</a><span>/</span><span>${names[state.route.view]}</span>` : state.route.view ? `<span>系统</span><span>/</span><span>${catalog?'接入能力':'消息中心'}</span>` : '<span>所有项目</span>';
  document.title = `${state.route.project ? (p?.name || '项目') + ' · ' + names[state.route.view] : catalog ? '接入能力' : state.route.view==='notifications'?'消息中心':'Projects'} · Feynman`;
}
function markdown(text) {
  // Escape all external content first; only create a small inert formatting subset.
  return String(text || '').split(/```[^\n]*\n([\s\S]*?)```/g).map((part,i) => i % 2 ? `<pre><code>${esc(part)}</code></pre>` : esc(part).replace(/\*\*([^*\n]+)\*\*/g,'<strong>$1</strong>').replace(/`([^`\n]+)`/g,'<code>$1</code>')).join('');
}
function renderProjects() {
  $('#main').innerHTML = `<div class="welcome"><div class="eyebrow">A PLACE FOR ONGOING WORK</div><h1>让项目持续向前。</h1><p>为每个项目保留上下文、执行环境和外部事实。<br>进入一个项目，与它的 Agent 一起继续工作。</p></div>${integrationSummary()}<div class="search-row"><input id="project-search" class="search" type="search" placeholder="搜索项目…" aria-label="搜索项目"><div class="heading-actions"><span class="muted tiny">${state.projects.length} 个项目</span>${button('＋ 新建项目','new-project','','primary')}</div></div><div id="project-cards" class="project-grid"></div><p class="footnote"><span class="status-dot"></span>本机 Rust Server · 持久会话 · 独立项目 Workspace</p>`;
  $('#project-search').addEventListener('input', renderProjectCards); renderProjectCards();
}
function renderProjectCards() {
  const query = ($('#project-search')?.value || '').toLowerCase(), projects = state.projects.filter(p => p.name.toLowerCase().includes(query));
  $('#project-cards').innerHTML = projects.map(p => `<a class="project-card" href="#${new URLSearchParams({project:p.id,view:'overview'})}"><div class="project-card-top"><span class="project-monogram">${esc(p.name.slice(0,1).toUpperCase())}</span>${badge('',p.main_agent_id ? 'MAIN AGENT' : '未创建 Agent')}</div><h2>${esc(p.name)}</h2><div class="project-id">${esc(p.id)}</div><div class="project-card-bottom"><span>创建于 ${date(p.created_at)}</span><b>↗</b></div></a>`).join('') || empty(query ? '没有匹配的项目' : '从你的第一个项目开始', '创建项目后，再选择 Agent、沙盒和所需权限。');
}

const capabilityLabels = {'dws.auth.device':'设备授权申请','dws.aisearch.enterprise':'企业内容搜索','dws.chat.messages.read':'历史消息读取','dws.event.messages.listen':'托管消息监听','gitlab.repo.read':'仓库元信息','gitlab.mr.read':'开放合并请求','git.remote.read':'分支与标签','git.workspace.clone':'克隆到项目沙盒','slate.identity.read':'账号身份','slate.tasks.read':'我的任务','slate.projects.read':'可见项目','drlee.identity.read':'账号身份','drlee.channels.read':'渠道列表','drlee.messages.read':'限定渠道消息','drlee.knowledge.read':'个人知识空间'};
const authLabels = {device:'Device 设备授权',owner_token_stdin:'所有者凭据导入',delegated_gitlab_https:'委托 GitLab HTTPS',browser_pkce:'浏览器 PKCE',owner_keychain_import:'本机 Keychain 导入',native_browser:'原生浏览器登录',owner_account_import:'原生账号导入'};
const sourceKindLabels = {manual:'手工采集',dws_messages:'历史轮询',dws_events:'事件监听'};
const providerName = p => p.display_name || p.executable || p.provider;
const providerAccounts = (p, connections) => connections.filter(c => c.provider === (p.provider === 'git' ? 'gitlab' : p.provider));
const integrationHref = (provider = '') => '#' + new URLSearchParams({view:'integrations',...(provider ? {provider} : {})});
const sourceProvider = (source, providers = state.data.providers || []) => providers.find(p => p.source_kinds.includes(source.kind))?.provider || (source.kind === 'manual' ? 'manual' : 'other');
function providerTabs(providers, view, extra = []) {
  const project = state.route.project, selected = state.route.provider || '';
  const link = provider => project ? href(view,{...(provider ? {provider} : {}),...(view === 'sources' && state.route.live ? {live:'1'} : {})}) : integrationHref(provider);
  return `<nav class="provider-tabs" aria-label="按来源筛选">${[['','全部来源'],...providers.map(p => [p.provider,providerName(p)]),...extra].map(([id,name]) => `<a class="button small ${selected === id ? 'selected' : ''}" href="${link(id)}" ${selected === id ? 'aria-current="page"' : ''}>${esc(name)}</a>`).join('')}</nav>`;
}
function integrationSummary() {
  const {providers,connections} = state.catalog;
  return `<section class="integration-summary" aria-label="外部接入能力"><div class="split-header"><div><h2>外部接入能力</h2><p class="tiny muted">${providers.length} 个已实现接入 · 系统支持不等于项目授权</p></div><a class="button small" href="${integrationHref()}">查看完整目录 ↗</a></div><div class="integration-strip">${providers.map(p => `<a class="integration-mini" data-provider="${esc(p.provider)}" href="${integrationHref(p.provider)}"><div class="integration-mini-top"><strong>${esc(providerName(p))}</strong><span aria-hidden="true">↗</span></div><p>${esc(p.summary)}</p><div class="integration-mini-bottom"><span>${p.source_kinds.length ? '工具 + 消息订阅' : '受限工具调用'}</span><span>${providerAccounts(p,connections).filter(c => !c.disabled).length} 个登记账号</span></div></a>`).join('')}</div></section>`;
}
function renderIntegrations() {
  const {providers,connections} = state.catalog, selected = state.route.provider;
  const visible = providers.filter(p => !selected || p.provider === selected);
  $('#main').innerHTML = heading('系统接入能力','来自 Rust Provider 目录。这里查看系统支持什么；账号绑定和能力授权仍在每个项目内管理。','', 'SYSTEM CAPABILITIES') + providerTabs(providers,'integrations') + `<div class="integration-catalog">${visible.map(p => {
    const accounts = providerAccounts(p,connections);
    return `<section class="panel integration-detail" data-provider="${esc(p.provider)}"><div class="panel-heading"><h2>${esc(providerName(p))}</h2>${badge('ready','已实现接入')}</div><div class="panel-body"><p class="muted">${esc(p.summary)}</p><dl>${kv('CLI',`<code>${esc(p.executable)}</code>`)}${kv('认证方式',p.authentication.map(a => esc(authLabels[a] || a)).join(' / '))}${kv('执行位置',p.delivery === 'server_proxy_fixed_git' ? 'Server 代理 · 固定 Git 操作可进入沙盒' : 'Server 受限代理')}${kv('消息采集',p.source_kinds.length ? p.source_kinds.map(k => esc(sourceKindLabels[k] || k)).join(' / ') : '尚未接入自动采集，仅支持工具按需读取')}</dl><h3 class="section-title">已支持能力</h3><ul class="capability-list">${p.capabilities.map(id => `<li><span>${esc(capabilityLabels[id] || id)}</span><code>${esc(id)}</code></li>`).join('')}</ul><details><summary>${accounts.length} 个已登记账号${p.provider === 'git' ? ' · 复用 GitLab Connection' : ''}</summary>${accounts.map(c => `<div class="row"><div><h3>${esc(c.display_name || c.identity)}</h3><div class="row-meta">${esc(c.hostname || 'Provider 原生范围')}</div></div>${badge(c.disabled ? 'paused' : '',c.disabled ? '已禁用' : '已登记 · 非在线认证证明')}</div>`).join('') || '<p class="muted tiny">尚未登记，登录与导入不会在打开目录时自动发生。</p>'}</details><a class="button small" href="#">进入项目配置授权 ↗</a></div></section>`;
  }).join('') || empty('没有匹配的接入','选择上方已支持的来源。')}</div><div class="callout">目录不会检查远端认证、发起登录、创建观测来源或授予权限。已登记不代表 Token 当前有效；只有显式启用的来源才会后台收取。Git 当前不支持推送，其他 CLI 也不是任意命令代理。</div>`;
}
function renderOverview() {
  const s = state.data.status, a = s.agents.filter(a => a.state !== 'archived'), w = s.workspace, r = s.main_agent?.active_run;
  $('#main').innerHTML = heading(s.project.name,'这个项目的会话、执行环境与外部事实，都在这里。',`<a class="button primary" href="${href('chat')}">进入 Agent 对话 ↗</a>`) + `<div class="stats"><div class="stat"><div class="stat-label">Agent</div><div class="stat-value">${a.length ? esc(a[0].name) : '未创建'}</div><small>${a.length ? esc(labels[a[0].state] || a[0].state) : '项目可以暂时没有 Agent'}</small></div><div class="stat"><div class="stat-label">当前运行</div><div class="stat-value">${r ? esc(labels[r.status] || r.status) : '空闲'}</div><small>${s.tasks.queued} 个任务排队中</small></div><div class="stat"><div class="stat-label">观测渠道</div><div class="stat-value">${s.sources.length}</div><small>各自保留独立来源历史</small></div><div class="stat"><div class="stat-label">Schedule</div><div class="stat-value">${s.schedules.enabled} <small>已启用</small></div><small>下次 ${date(s.schedules.next_due_at)}</small></div></div><div class="two-columns"><div class="stack">${panel('项目 Agent',a.map(a => `<div class="row"><div><h3>${esc(a.name)}</h3><p>长期会话 · 原生 Codex Runtime</p><div class="row-meta">${esc(a.id)}</div></div><div class="row-actions">${badge(a.state)}<a class="button small" href="${href('chat')}">打开对话</a></div></div>`).join('') || empty('没有活动 Agent','在 Agent 设置中显式创建。'))}${panel('待处理与收件箱',s.inbox.filter(i => ['pending','unknown','dispatching'].includes(i.state)).slice(-6).map(i => `<div class="row"><div><h3>${esc(i.prompt.slice(0,90))}</h3><div class="row-meta">${date(i.created_at)} · ${esc(i.delivery)}</div></div>${badge(i.state)}</div>`).join('') || '<p class="muted">暂时没有待处理的投递。</p>',`<a class="tiny muted" href="${href('inbox')}">查看全部 ↗</a>`)}</div><div class="stack">${panel('Execution World',w ? `<dl>${kv('Provider',esc(w.provider))}${kv('状态记录',badge(w.state))}${kv('工作目录',`<code>${esc(w.cwd)}</code>`)}${kv('持久卷',`<code>${esc(w.volume_name || '本机目录')}</code>`)}${kv('有效至',date(w.expires_at))}</dl><a class="button" href="${href('workspace')}">管理 Workspace</a>` : empty('还没有 Workspace','在 Workspace 页显式创建沙盒。'))}${panel('外部事实与权限',`<p class="muted">登录、项目绑定和业务权限分别管理。已有账号不会自动给这个项目授权。</p><div class="row"><a href="${href('access')}">外部授权</a><span>↗</span></div><div class="row"><a href="${href('sources')}">观测消息与独立渠道</a><span>↗</span></div><div class="row"><a href="${href('memory')}">项目长期记忆</a><span>↗</span></div>`)}</div></div><p class="footnote">状态更新时间 ${date(state.updated)} · Workspace 状态是持久记录，实时检查需在 Workspace 页刷新。</p>`;
}
function renderChatShell() {
  const s = state.data.status, a = s.agents.find(a => a.id === s.project.main_agent_id && a.state !== 'archived');
  $('#main').innerHTML = `<div class="chat-page">${heading(a?.name || '项目 Agent','同一长期会话 · 原生工具执行在项目 Workspace',badge(a?.state || '未创建'))}<div class="chat-columns"><div class="chat-main"><div id="chat-messages" class="chat-messages" aria-live="polite" aria-label="Agent 会话"></div><div id="chat-approvals" aria-live="polite"></div><form id="composer" class="composer"><textarea id="prompt" placeholder="交代一个目标，或继续刚才的讨论…" aria-label="发送给 Agent 的消息" maxlength="16000" ${a?.state !== 'active' ? 'disabled' : ''}></textarea><div class="composer-bottom"><select id="delivery" aria-label="消息投递方式"><option value="steer_if_active">立即交给 Agent · 活动 Run 中追加提示</option><option value="queue">排队 · 当前工作完成后处理</option></select><button class="button primary" type="submit" ${a?.state !== 'active' ? 'disabled' : ''}>发送 ↑</button></div></form><div class="composer-note">⌘ / Ctrl + Enter 发送 · 立即显示接收状态，回复通过 SSE 推送 · 审批中不强行插队</div></div><aside class="chat-context"><div id="chat-context"></div><div id="chat-activity"></div></aside></div></div>`;
  $('#prompt').value = state.drafts.get(state.route.project) || '';
  $('#prompt').addEventListener('input', e => state.drafts.set(state.route.project,e.target.value));
  $('#prompt').addEventListener('keydown', e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); $('#composer').requestSubmit(); } });
  $('#composer').addEventListener('submit', sendPrompt);
}
function updateChat() {
  const c = state.data.conversation, s = state.data.status, messages = $('#chat-messages'); if (!messages || !c) return;
  const bottom = messages.scrollHeight - messages.scrollTop - messages.clientHeight < 110;
  const durablePending=(state.data.inbox || []).filter(i=>!i.schedule_id && !c.messages.some(m=>m.role==='user'&&m.content.includes(`[Feynman Inbox ID: ${i.id}]`)));
  const pending=state.pending.filter(i=>i.project===state.route.project && (!i.id || !(state.data.inbox || []).some(m=>m.id===i.id)));
  const receiptHtml=[...durablePending.map(i=>({content:i.prompt,status:i.state,created_at:i.created_at})),...pending].map(i=>`<article class="message user receipt"><div class="avatar">你</div><div class="message-body"><div class="message-meta"><strong>你</strong><span class="date">${date(i.created_at)}</span>${badge(i.status,i.status==='sending'?'发送中':i.status==='pending'?'已收到 · 等待 Agent':undefined)}</div><div class="message-text">${markdown(i.content)}</div>${i.status==='unknown'?'<p class="tiny muted">结果未知；请核对收件箱，不要直接重复发送。</p>':''}</div></article>`).join('');
  const html = c.messages.map(m => {
    const failed = m.role === 'assistant' && ['failed','interrupted'].includes(m.status);
    const run = failed ? c.runs.find(r => r.id === m.run_id) : null;
    const failure = failed ? `<div class="message-failure" role="status"><p>${esc(run?.error || '本次运行未完成。')}</p><p class="tiny muted">系统没有自动重试。请先检查错误，再决定是否重新发送消息。</p></div>` : '';
    return `<article class="message ${m.role}"><div class="avatar">${m.role === 'user' ? '你' : 'F'}</div><div class="message-body"><div class="message-meta"><strong>${m.role === 'user' ? '你' : m.role === 'assistant' ? 'Main Agent' : '系统'}</strong><span class="date">${date(m.created_at)}</span>${m.status === 'streaming' ? badge('running','生成中') : failed ? badge(m.status) : ''}</div><div class="message-text">${markdown(m.content || (m.status === 'streaming' ? '正在准备与执行…' : ''))}${failure}</div></div></article>`;
  }).join('') + receiptHtml || `<div class="chat-empty"><span class="project-monogram">F</span><h2>从一个清晰的目标开始。</h2><p>Agent 会沿用这个项目的上下文、工具和执行环境。</p><div class="suggestions">${button('检查项目和执行环境','suggest','data-prompt="先只读检查这个项目的 Workspace 与现有授权，说明可以做什么，不修改文件或申请额外权限。"')}${button('讨论下一步迭代方向','suggest','data-prompt="先分析仓库结构和已有证据，给出下一步迭代建议，暂时不要修改代码。"')}</div></div>`;
  if (messages.innerHTML !== html) { messages.innerHTML = html; if (bottom) messages.scrollTop = messages.scrollHeight; }
  const resolved = new Set(state.events.filter(e => e.kind === 'approval_resolved').map(e => e.payload.requestId));
  const approvals = state.run && !['completed','failed','interrupted'].includes(state.run.status) ? state.events.filter(e => e.kind === 'approval_requested' && !resolved.has(e.payload.requestId)) : [];
  $('#chat-approvals').innerHTML = approvals.map(e => `<div class="approval-card"><h3>需要你批准 · ${esc(state.run.id)}</h3><pre>${esc(e.payload.summary || '原生 Codex 审批请求')}</pre><div class="row-actions">${button('允许本次操作','approval',`data-request="${esc(e.payload.requestId)}" data-decision="accept"`,'small primary')}${button('拒绝','approval',`data-request="${esc(e.payload.requestId)}" data-decision="decline"`,'small')}</div></div>`).join('');
  const w = s.workspace, run = c.active_run || c.runs.at(-1);
  $('#chat-context').innerHTML = panel('当前上下文',`<dl>${kv('Run',badge(run?.status || '空闲'))}${kv('Workspace',esc(w?.provider || '未配置'))}${kv('目录',`<code>${esc(w?.cwd || '—')}</code>`)}${kv('Thread',`<code>${esc(c.conversation.runtime_thread_id || '尚未建立')}</code>`)}</dl><a class="button small" href="${href('access')}">查看外部授权 ↗</a><a class="button small" href="${href('inbox')}">收件箱与投递 ↗</a>`);
  const tools = state.events.filter(e => e.kind === 'tool_activity' || e.kind === 'run_failed').slice(-14).reverse();
  $('#chat-activity').innerHTML = `<h3 class="section-title">运行活动</h3>` + (tools.map(e => `<div class="activity-item"><strong>${esc(e.payload.tool || 'Run 错误')}</strong><p>${esc(e.payload.status || e.payload.error || e.kind)}</p><span class="date">${date(e.created_at)}</span></div>`).join('') || '<p class="tiny muted">启动运行后，这里展示工具活动。审批始终显示在输入框上方。</p>');
}
async function sendPrompt(e) {
  e.preventDefault(); if (state.busy || !$('#prompt').value.trim()) return;
  const project = state.route.project, content = $('#prompt').value.trim(), delivery = $('#delivery').value, a = state.data.status.project.main_agent_id;
  state.busy = true; const submit = $('#composer button'); submit.disabled = true;
  const pending={project,content,status:'sending',created_at:new Date().toISOString()}; state.pending.push(pending);updateChat();
  try {
    const response=await api(`projects/${project}/inbox`,'POST',{agent_id:a,content,delivery});
    pending.id=response.message.id;pending.status=response.message.state;
    if(state.route.project===project && state.data.conversation) {
      const existing=(state.data.inbox || []).find(i=>i.id===pending.id);
      state.data.inbox=[...(state.data.inbox || []).filter(i=>i.id!==pending.id),existing && existing.updated_at>=response.message.updated_at ? existing : response.message]; updateChat();
    }
    state.drafts.delete(project); if (state.route.project === project && $('#prompt')) $('#prompt').value = '';
    notice('消息已收到，Agent 接收与回复状态会实时更新。'); if(!state.projectLive)await refresh();
  } catch (error) { pending.status=error.rejected?'failed':'unknown';if(state.route.project===project)updateChat();notice(error.message,true); } finally { state.busy = false; if (submit.isConnected) submit.disabled = false; }
}
const notificationTitles={run_completed:'Agent 工作已完成',run_failed:'Agent 运行失败',run_interrupted:'Agent 运行已中断',approval_requested:'需要你批准操作'};
function renderNotifications() {
  $('#main').innerHTML=heading('消息中心','最近 100 条持久提醒。已读仅代表浏览，不代表任务验收或批准申请。','', 'YOUR ATTENTION')+panel('项目动态',state.notifications.slice().reverse().map(n=>`<div class="row notification-row ${n.read?'':'unread'}"><div><h3>${esc(notificationTitles[n.kind] || n.kind)}</h3><p>${esc(n.project_name)}</p><div class="row-meta">${date(n.created_at)} · ${esc(n.run_id)}</div></div><div class="row-actions">${n.resolved?badge('completed','已处理'):n.read?badge('','已读'):badge('pending','未读')}${button('打开项目','open-notification',`data-id="${esc(n.id)}"`,'small')}${!n.read?button('标为已读','read-notification',`data-id="${esc(n.id)}"`,'small'):''}</div></div>`).join('') || '<p class="muted">新产生的任务完成、失败和审批提醒会保留在这里。</p>')+'<div class="callout">系统横幅默认只展示项目名和事件类型。macOS 版可在菜单栏的 Feynman 菜单中启用系统通知。通知不携带外部消息正文，也不会代替你批准授权。</div>';
}
async function loadNotifications() {
  const head=await api('notifications?limit=1');
  const feed=await api('notifications?after='+Math.max(0,head.head-100)+'&limit=100');
  state.notifications=feed.items;state.unread=feed.unread;
}
function renderAgents() {
  const agents = state.data.status.agents, tools = state.data.tools;
  $('#main').innerHTML = heading('项目 Agent','当前后端支持一个未归档 Agent；Reviewer 和多 Agent 仍是预留能力。',!agents.some(a => a.state !== 'archived') ? button('＋ 创建 Agent','new-agent','','primary') : '') + agents.map(a => panel(esc(a.name),`<dl>${kv('Agent ID',`<code>${esc(a.id)}</code>`)}${kv('生命周期',badge(a.state))}${kv('默认路由',a.is_default ? '默认 Agent' : '非默认')}${kv('能力策略',a.capabilities === null ? '迁移的原策略 · 仍受项目 Grant 约束' : `${a.capabilities.length} 项声明能力`)}</dl>${a.capabilities ? `<div class="pill-tabs">${a.capabilities.map(c => badge('',c)).join('')}</div>` : ''}<div class="row-actions">${a.state !== 'archived' ? button('编辑状态与能力','edit-agent',`data-id="${a.id}"`) + `<a class="button primary" href="${href('chat')}">进入对话</a>` : '<p class="muted tiny">已归档的历史保持持久化；不会自动转移给新 Agent。</p>'}</div>`)).join('') + (agents.length ? '' : empty('这个项目还没有 Agent')) + `<div class="callout">声明能力与项目 Grant 是交集。增加 Agent 或改名不会自动批准工具，也不会转移旧来源的身份。</div>`;
}
function renderTasks() {
  const tasks = state.data.tasks, runs = state.data.conversation.runs;
  $('#main').innerHTML = heading('任务与运行','长期任务和运行记录来自同一 Rust Server。取消排队任务不会强行杀死正在执行的 Run。',button('＋ 新建任务','new-task','','primary')) + panel('持久任务',tasks.map(t => `<div class="row"><div><h3>${esc(t.title)}</h3><p>${esc(t.prompt)}</p><div class="row-meta">${date(t.created_at)} · ${esc(t.id)}</div></div><div class="row-actions">${badge(t.status)}${t.status === 'queued' ? button('取消','cancel-task',`data-id="${t.id}"`,'small danger') : ''}</div></div>`).join('') || '<p class="muted">暂无任务。</p>') + `<h3 class="section-title">运行历史</h3>` + panel('Agent Runs',runs.slice().reverse().map(r => `<div class="row"><div><h3><code>${esc(r.id)}</code></h3><div class="row-meta">${date(r.created_at)} → ${date(r.completed_at)}</div>${r.error ? `<p>${esc(r.error)}</p>` : ''}</div><div class="row-actions">${badge(r.status)}${button('查看事件','run-events',`data-id="${r.id}"`,'small')}</div></div>`).join('') || '<p class="muted">还没有运行记录。</p>');
}
function renderSchedules() {
  const schedules = state.data.schedules;
  $('#main').innerHTML = heading('Schedule','定时投递，或在整条 Schedule 的固定窗口内汇总来源变化。新计划默认暂停。',button('＋ 新建 Schedule','new-schedule','','primary')) + panel('项目计划',schedules.map(s => `<div class="row"><div><h3>${esc(s.title)}</h3><p>${esc(s.prompt)}</p><div class="row-meta">${s.source_ids.length ? `${s.source_ids.length} 个来源 · 固定窗口 ${s.collect_for_seconds}s` : s.interval_seconds ? `每 ${s.interval_seconds}s` : '一次性'} · 目标 ${esc(s.agent_id || '旧任务模型')} · ${esc(s.delivery)}</div><div class="row-meta">下次 ${date(s.next_due_at)} · ${esc(s.id)}</div></div><div class="row-actions">${badge(s.exhausted ? 'exhausted' : s.enabled ? 'active' : 'paused')}${!s.exhausted ? button(s.enabled ? '暂停' : '启用','toggle-schedule',`data-id="${s.id}" data-enabled="${!s.enabled}"`,'small') : ''}${button('投递记录','deliveries',`data-id="${s.id}"`,'small')}</div></div>`).join('') || '<p class="muted">还没有 Schedule。创建不会自动启用。</p>') + '<div class="callout">Schedule 固定绑定一个 Agent。查看消息窗口不会改变采集或投递水位；unknown 投递必须先对账，不自动重放。</div>';
}
function renderInbox() {
  const inbox = state.data.inbox;
  $('#main').innerHTML = heading('Agent 收件箱','这里是投递给 Agent 的 Prompt，不是外部消息原文。查看不会触发处理或重发。') + panel('投递记录',inbox.slice().reverse().map(i => `<div class="row"><div><h3>${esc(i.prompt.slice(0,130))}</h3><div class="row-meta">${date(i.created_at)} · ${esc(i.delivery)} · ${esc(i.id)}</div><div class="row-meta">Schedule ${esc(i.schedule_id || '用户消息')} → Run ${esc(i.run_id || '尚未绑定')}</div><details><summary>查看完整 Prompt</summary><pre>${esc(i.prompt)}</pre></details></div>${badge(i.state)}</div>`).join('') || '<p class="muted">暂时没有投递。向 Agent 发送的消息也会记录在这里。</p>') + '<div class="callout">accepted 表示已被运行时接收，不等于任务成功完成。unknown 表示结果不明，首版页面不提供无条件重发。</div>';
}
function observationBody(body) {
  if (typeof body === 'string') return body;
  return body?.text?.content || body?.content || body?.body || body?.text || pretty(body);
}
function renderSources() {
  const sources = visibleSources(), selected = state.route.source, live = state.route.live, providers = state.data.providers;
  const extra = {...(state.route.provider ? {provider:state.route.provider} : {}),...(live ? {live:'1'} : {})};
  const categories = [...providers.filter(p => p.source_kinds.length).map(p => [p.provider,providerName(p)]),['manual','手工来源'],['other','其他来源']];
  const groups = categories.map(([id,name]) => {
    const channels = sources.filter(s => sourceProvider(s) === id);
    if (!channels.length) return '';
    return `<section class="channel-group" data-provider="${esc(id)}"><h3>${esc(name)}<span>${channels.length} 条</span></h3>${channels.map(s => `<div><a class="channel-button ${selected === s.id ? 'selected' : ''}" href="${href('sources',{...extra,source:s.id})}"><strong>${esc(s.name)}</strong><small>${esc(sourceKindLabels[s.kind] || s.kind)} · ${esc(s.listener_state)} · ${s.enabled ? '开启' : '暂停'}</small></a><div class="panel-body"><div class="row-actions">${button(s.enabled ? '暂停' : '启用','toggle-source',`data-id="${s.id}" data-enabled="${!s.enabled}"`,'small')}<a class="button small" href="${href('sources',{...extra,source:s.id,live:'1'})}" target="_blank" rel="noopener">独立窗口 ↗</a></div><div class="row-meta">覆盖 ${esc(s.coverage)}<br>水位 ${date(s.cursor)}</div></div></div>`).join('')}</section>`;
  }).join('');
  $('#main').innerHTML = heading(live ? '渠道监听窗口' : '观测消息','按来源聚类，每条渠道保留独立历史；查看不启动监听、不唤醒 Agent。',button('＋ 登记来源','new-source','','primary')) + providerTabs(providers.filter(p => p.source_kinds.length),'sources',[['manual','手工来源'],...(state.data.sources.some(s => sourceProvider(s) === 'other') ? [['other','其他来源']] : [])]) + `<div class="feed-layout"><section class="panel"><div class="panel-heading"><h2>消息渠道</h2><span class="tiny muted">${sources.length}</span></div><a class="channel-button ${!selected ? 'selected' : ''}" href="${href('sources',extra)}"><strong>${state.route.provider ? '本来源的全部渠道' : '所有已授权渠道'}</strong><small>统一持久消息流</small></a>${groups || '<div class="panel-body"><p class="muted tiny">本来源暂无渠道，登记后默认关闭，不自动监听。</p></div>'}</section><section class="panel"><div class="panel-heading"><h2>${esc(sources.find(s => s.id === selected)?.name || '项目消息流')}</h2><span class="badge good">${live ? '仅新增消息' : '持久历史'} · 2s 轮询</span></div><div id="feed-records"></div><div class="panel-body">${button('加载下一页','feed-more','','small')}<span id="feed-state" class="muted tiny"></span></div></section></div><div class="callout">来源分类只是查看过滤，不合并 Source 身份或持久历史。查看游标属于本窗口，独立于后台采集、Schedule 和 Agent Inbox。窗口保留最近 300 条，长期历史仍在 Server；Slate / Drlee 当前只支持按需工具读取。</div>`;
  updateFeed();
}
function visibleSources() {
  return (state.data.sources || []).filter(s => !state.route.provider || sourceProvider(s) === state.route.provider);
}
function updateFeed() {
  if (!$('#feed-records')) return;
  const sources = visibleSources(), allowed = new Set(sources.map(s => s.id));
  $('#feed-records').innerHTML = state.feed.filter(r => allowed.has(r.source_id)).map(r => `<article class="observation"><div class="observation-header"><strong>${esc(sources.find(s => s.id === r.source_id)?.name || r.source_id)}</strong><span>${date(r.occurred_at)} · #${r.sequence}</span></div><pre>${esc(typeof observationBody(r.body) === 'object' ? pretty(observationBody(r.body)) : observationBody(r.body))}</pre></article>`).join('') || empty(state.route.live ? '正在等待新消息' : '尚无可读消息','后台收取依赖有效授权与来源启用，不依赖此窗口是否打开。');
  $('#feed-state').textContent = `  游标 ${state.feedAfter} · ${state.feedMore ? '还有持久历史可读取' : '已读至当前可见水位'}`;
  $('[data-action="feed-more"]').disabled = !state.feedMore;
}
function renderWorkspace() {
  const w = state.data.status.workspace;
  $('#main').innerHTML = heading('Execution World','项目代码与依赖保留在持久卷。控制面、模型凭据和长期会话留在本机。',w ? button('刷新实时状态','workspace-action','data-operation="refresh"') : '') + (w ? panel('Workspace',`<dl>${kv('Provider',esc(w.provider))}${kv('状态记录',badge(w.state))}${kv('环境 ID',`<code>${esc(w.environment_id)}</code>`)}${kv('Sandbox',`<code>${esc(w.sandbox_id || '—')}</code>`)}${kv('工作目录',`<code>${esc(w.cwd)}</code>`)}${kv('持久卷',`<code>${esc(w.volume_name || '本机目录')}</code>`)}${kv('到期时间',date(w.expires_at))}${kv('记录更新',date(w.updated_at))}</dl><div class="row-actions">${['resume','restore','renew','stop'].map(action => button({resume:'恢复运行',restore:'恢复缺失容器',renew:'续期',stop:'暂停容器'}[action],'workspace-action',`data-operation="${action}"`,action === 'stop' ? 'danger' : '')).join('')}</div>`) : panel('创建执行环境',`<p class="muted">优先使用当前本机 OpenSandbox。创建会占用本机 Docker 资源，但不启动 GKE。</p><div class="heading-actions">${button('创建 Docker 沙盒','create-workspace','data-provider="opensandbox"','primary')}${button('使用本机目录','create-workspace','data-provider="local"')}</div>`)) + '<div class="callout warning">当前为可信单用户 Docker 环境，不是 gVisor / VM。状态需要显式刷新，页面不会自动创建、恢复或续期资源。首版没有文件树；让 Agent 通过原生执行工具检查文件。</div>';
}
function renderAccess() {
  const {access,connections,tools,providers,authorizations} = state.data;
  const selected = state.route.provider, sources = state.data.sources;
  const assigned = new Set(providers.flatMap(p => p.capabilities)), platformTools = tools.filter(t => !assigned.has(t.id));
  const groups = providers.filter(p => !selected || selected === p.provider).map(p => {
    const binding = access.bindings.find(b => b.provider === p.provider), connection = connections.find(c => c.id === binding?.connection_id);
    const capabilities = tools.filter(t => p.capabilities.includes(t.id)), channels = sources.filter(s => sourceProvider(s,providers) === p.provider);
    const expired = binding && Date.parse(binding.expires_at) <= Date.now();
    const bindingStatus = !binding ? '未绑定账号' : !connection || connection.disabled ? '绑定账号不可用' : expired ? '绑定已过期' : '配置已绑定 · 非在线认证证明';
    const requests = access.requests.filter(r => r.provider === p.provider);
    return `<section class="panel integration-group" data-provider="${esc(p.provider)}"><div class="panel-heading"><div><h2>${esc(providerName(p))}</h2><p class="tiny muted">${esc(p.summary)}</p></div>${badge('',`${capabilities.filter(t => t.granted).length} / ${capabilities.length} 项项目授权`)}</div><div class="panel-body"><div class="integration-binding"><div>${badge(expired ? 'expired' : connection?.disabled ? 'paused' : '',bindingStatus)}${binding ? `<p>${esc(connection?.display_name || connection?.identity || binding.connection_id)} · ${esc(connection?.hostname || '')}</p><div class="row-meta">${esc(binding.repository || binding.resource_id || '原生账号范围')} · 到期 ${date(binding.expires_at)}</div>` : `<p class="tiny muted">${providerAccounts(p,connections).filter(c => !c.disabled).length} 个已登记账号可供选择${p.provider === 'git' ? ' · 复用 GitLab Connection' : ''}</p>`}</div><div class="row-actions">${button(binding ? '更换绑定' : '绑定账号','bind',`data-provider="${esc(p.provider)}"`,'small')}${binding ? button('解绑','unbind',`data-provider="${esc(p.provider)}"`,'small danger') : ''}${p.provider === 'dws' ? button('发起 DWS 设备授权','dws-login','','small') : ''}</div></div><h3 class="section-title">能力与项目许可</h3>${toolTable(capabilities)}<div class="integration-channel-summary"><div><strong>${p.source_kinds.length ? `${channels.length} 条独立观测渠道` : '按需工具读取'}</strong><p class="tiny muted">${p.source_kinds.length ? p.source_kinds.map(k => esc(sourceKindLabels[k] || k)).join(' / ') + ' · 每条渠道保留独立历史' : '尚未接入自动采集，不会随授权自动订阅消息'}</p></div>${p.source_kinds.length ? `<a class="button small" href="${href('sources',{provider:p.provider})}">查看来源渠道 ↗</a>` : ''}</div>${requests.length ? `<details><summary>权限申请 · ${requests.length}</summary>${requests.map(r => `<div class="row"><div><h3>${esc(capabilityLabels[r.capability] || r.capability)}</h3><p>${esc(r.reason)}</p><div class="row-meta">${date(r.created_at)} · 申请不等于批准</div></div></div>`).join('')}</details>` : ''}${p.provider === 'dws' ? `<details ${authorizations.some(a => a.status === 'waiting_for_user') ? 'open' : ''}><summary>DWS 授权进度与历史 · ${authorizations.length}</summary>${deviceAttempts(authorizations)}</details>` : ''}</div></section>`;
  }).join('');
  $('#main').innerHTML = heading('项目外部接入','按来源查看绑定、具体能力与观测渠道；目录支持、账号认证和项目授权是三件不同的事。',`<a class="button small" href="${integrationHref(selected && selected !== 'platform' ? selected : '')}">系统接入目录 ↗</a>`) + providerTabs(providers,'access',platformTools.length ? [['platform','平台工具']] : []) + `<div class="stack">${groups}${(!selected || selected === 'platform') && platformTools.length ? panel('平台工具',toolTable(platformTools)) : ''}${!groups && selected !== 'platform' ? empty('没有匹配的来源','选择上方已支持的来源。') : ''}</div><div class="callout">已授予表示项目 Grant，不代表 Agent 当前可以调用。实际调用还受 Agent 声明能力、绑定期限、账号认证和资源范围校验。查看、筛选、切换项目都不会发起认证或隐式授权。</div>` + panel('原生 CLI 登录',`<p class="muted">Slate / Drlee 新登录、GitLab 原生导入与密钥登记保留在所有者 CLI；页面仅绑定经验证的公开 Connection，不接收密钥。</p><pre class="code-block">python3.13 scripts/opensandbox/local.py cli -- access connections list
python3.13 scripts/opensandbox/local.py cli -- access connections login-slate --hostname sh.slate.star-x.dev
python3.13 scripts/opensandbox/local.py cli -- access connections import-drlee --hostname drlee.dip-aitech.com</pre>`);
}
function toolTable(tools) {
  return `<div class="table-wrap"><table><thead><tr><th>能力</th><th>用途与限制</th><th>项目许可</th></tr></thead><tbody>${tools.map(t => `<tr><td><strong>${esc(capabilityLabels[t.id] || t.name)}</strong><br><code>${esc(t.id)}</code></td><td><p class="tiny muted">${esc(t.description)}</p></td><td>${button(t.granted ? '撤销' : '授予','grant',`data-tool="${esc(t.id)}"`,'small'+(t.granted ? ' danger' : ''))}</td></tr>`).join('')}</tbody></table></div>`;
}
function deviceAttempts(authorizations) {
  return authorizations.slice().reverse().map(a => {
    let url = a.verification_uri_complete || a.verification_uri;
    try {if(new URL(url).protocol !== 'https:')url=null;}catch(_){url=null;}
    return `<div class="row"><div><h3>${badge(a.status)} ${esc(a.reason)}</h3><div class="row-meta">${esc(a.id)} · 有效至 ${date(a.expires_at)}</div>${a.user_code ? `<p>用户码 <code>${esc(a.user_code)}</code></p>` : ''}${a.error_code ? `<p>${esc(a.error_code)}</p>` : ''}${a.status === 'authorized' ? '<p>成功缓存可通过 CLI register-dws 登记，再在此页绑定。新登录不会自动授予业务权限。</p>' : ''}</div><div class="row-actions">${url && a.status === 'waiting_for_user' ? `<a class="button small primary" href="${esc(url)}" target="_blank" rel="noopener noreferrer">打开授权页 ↗</a>` : ''}${button('检查','dws-check',`data-id="${a.id}"`,'small')}</div></div>`;
  }).join('') || '<p class="muted">暂无设备授权任务。</p>';
}
function renderMemory() {
  const memories = state.data.memories;
  $('#main').innerHTML = heading('项目长期记忆','经证据权限校验的持久记录。事实、推断与决策保持区分；本页只读，不替 Agent 拼接会话。') + `<div class="search-row"><form id="memory-search"><input class="search" name="query" placeholder="搜索项目记忆…" aria-label="搜索项目记忆" value="${esc(state.memoryQuery || '')}"> <button class="button">搜索</button></form></div>` + '<div class="stack">' + memories.map(m => panel(esc(m.content.title),`${badge('',labels[m.content.kind] || m.content.kind)} ${badge('',m.content.state)}<div class="memory-body">${esc(m.content.body)}</div><div class="row-meta">${esc(m.id)} · v${m.revision} · ${esc(m.content.scope)}${m.pinned ? ' · 已固定' : ''}</div><details><summary>证据引用</summary><pre>${esc(pretty(m.content.evidence))}</pre></details>`)).join('') + (memories.length ? '' : empty('没有可见的记忆记录','让 Agent 使用 feynman_memory 记录结论与证据。凭据不应写入长期记忆。')) + '</div>';
  $('#memory-search').addEventListener('submit', async e => {e.preventDefault(); state.memoryQuery = new FormData(e.target).get('query'); await refresh();});
}
function renderEnvironment() {
  const {profiles,envRequests,leases} = state.data;
  const list = (items, description) => items.map(r => `<div class="row"><div><h3>${esc(r.data?.title || r.data?.name || r.id)}</h3><div class="row-meta">${esc(r.id)} · revision ${r.revision}</div><details><summary>公开状态与配置</summary><pre>${esc(pretty(r.data))}</pre></details></div>${badge(r.data?.state || '')}</div>`).join('') || `<p class="muted">${description}</p>`;
  $('#main').innerHTML = heading('环境与租约','环境审批、使用租约与 Provider 认证是不同对象。本页只查看公开状态，不展示或登记秘密值。') + '<div class="stack">' + panel('执行 Profiles',list(profiles,'尚未配置执行 Profile。')) + panel('环境使用申请',list(envRequests,'没有环境使用申请。')) + panel('使用租约',list(leases,'没有使用租约。')) + '</div><div class="callout">敏感环境变量登记、批准和租约撤销仍通过所有者 CLI 完成。usage_lease 约束已批准 workload 的使用，不代表 Provider Token 已到期。</div>';
}
const renders = {overview:renderOverview,chat:renderChatShell,agents:renderAgents,tasks:renderTasks,schedules:renderSchedules,sources:renderSources,inbox:renderInbox,workspace:renderWorkspace,access:renderAccess,memory:renderMemory,environment:renderEnvironment};

let renderedSignature = '';
async function refresh(force = false) {
  if (state.refreshing) return;
  if(!force && state.route.view==='chat' && state.projectLive)return;
  let resumeStream=false;
  if(force && projectStream) {projectStream.abort();projectStream=null;state.projectLive=false;resumeStream=true;}
  state.refreshing = true; const generation = state.generation, route = {...state.route};
  const streamRevision=state.streamRevision;
  try {
    if (!route.project) {
      if(route.view==='notifications') {
        await loadNotifications();if(generation!==state.generation)return;
        renderNotifications();navigation();return;
      }
      const [projectList,providerList,connectionList] = await Promise.all([api('projects'),api('access/providers'),api('access/connections')]);
      if (generation !== state.generation) return;
      state.projects = projectList.projects; state.catalog = {providers:providerList.providers,connections:connectionList.connections};
      const signature = pretty([state.projects,state.catalog]);
      if (force || renderedSignature !== signature) {route.view === 'integrations' ? renderIntegrations() : renderProjects();renderedSignature = signature;}
      navigation(); return;
    }
    const p = 'projects/' + route.project, view = route.view, data = {status:await api(p+'/status')};
    const queries = {
      chat:[['conversation',p+'/conversation'],['inbox',p+'/inbox','inbox']], agents:[['tools',p+'/external-tools','tools']],
      tasks:[['tasks',p+'/tasks','tasks'],['conversation',p+'/conversation']],
      schedules:[['schedules',p+'/schedules','schedules'],['sources',p+'/sources','sources']],
      sources:[['sources',p+'/sources','sources'],['providers','access/providers','providers']], inbox:[['inbox',p+'/inbox','inbox']],
      access:[['access',p+'/access'],['connections','access/connections','connections'],['providers','access/providers','providers'],['tools',p+'/external-tools','tools'],['authorizations',p+'/dws-authorizations','authorizations'],['sources',p+'/sources','sources']]
    };
    await Promise.all((queries[view] || []).map(async ([key,path,field]) => {const value = await api(path);data[key] = field ? value[field] : value;}));
    if (view === 'memory') data.memories = (await api(p+'/resources','POST',{operation:'memory.search',arguments:{query:state.memoryQuery || '',limit:100}})).memories;
    if (view === 'environment') {
      const [profiles,requests,leases] = await Promise.all(['profile.list','env.status','lease.list'].map(operation => api(p+'/resources','POST',{operation,arguments:{}})));
      data.profiles = profiles.profiles; data.envRequests = requests.items; data.leases = leases.items;
    }
    let events = state.events, cursor = state.cursor, run = state.run;
    if (view === 'chat') {
      const current = data.conversation.active_run || data.conversation.runs.at(-1);
      if (current?.id !== run?.id) {events=[];cursor=0;run=current || null;}
      if (run) {
        const feed = await api(p+'/runs/'+run.id+'/events?after='+cursor);
        const seen = new Set(events.map(e => e.id)); events = [...events,...feed.events.filter(e => !seen.has(e.id))];
        cursor = Math.max(cursor,...feed.events.map(e => e.sequence)); run=feed.run;
      }
    }
    let feed = state.feed, after = state.feedAfter, more = state.feedMore;
    if (view === 'sources') {
      const qs = new URLSearchParams({after:String(after),limit:'100',...(route.source ? {source_id:route.source} : {})});
      if (route.live && !state.feedInitialized) qs.set('head','true');
      const page = await api(p+'/feed?'+qs);
      const authorized = new Set(page.channels.map(c => c.source_id));
      feed = feed.filter(r => authorized.has(r.source_id));
      const known = new Set(feed.map(r => r.source_id+':'+r.event_id));
      feed = [...feed,...page.records.filter(r => !known.has(r.source_id+':'+r.event_id))].slice(-300);
      after=page.next_after;more=page.has_more;
    }
    if (generation !== state.generation) return;
    // Never let a slower REST read overwrite newer pushed text or receipts.
    if(view==='chat' && streamRevision!==state.streamRevision) {
      data.conversation=state.data.conversation;data.inbox=state.data.inbox;
      events=state.events;cursor=state.cursor;run=state.run;
    }
    state.data=data;state.events=events;state.cursor=cursor;state.run=run;
    state.feed=feed;state.feedAfter=after;state.feedMore=more;state.feedInitialized=true;
    const signature=pretty(data);navigation();
    if (view === 'chat') {if(force || !$('#chat-messages'))renderChatShell();updateChat();}
    else if (force || signature !== renderedSignature) {
      if (!$('#dialog').open && !document.activeElement?.matches('#main input,#main textarea')) {renders[view]();renderedSignature=signature;}
    }
    if(view === 'sources')updateFeed();
  } catch (error) {
    if (generation !== state.generation) return;
    if (route.view === 'sources') {state.feed=[];updateFeed();}
    $('#connection').classList.add('offline');$('#connection').innerHTML='<i></i>读取失败 · 可刷新重连';
    if (force || !state.data.status && route.project) $('#main').innerHTML=`<div class="error-state"><h2>暂时无法读取工作台</h2><p>${esc(error.message)}</p><div class="heading-actions">${button('重新连接','reload','','primary')}<a class="button" href="#">返回项目列表</a></div></div>`;
    else notice(error.message,true);
  } finally {state.refreshing=false;if(resumeStream && generation===state.generation && state.data.conversation)startProjectStream();}
}
async function routeChanged() {
  projectStream?.abort();projectStream=null;state.projectLive=false;
  state.generation++;state.route=parseRoute();state.data={};state.events=[];state.cursor=0;state.run=null;
  state.feed=[];state.feedAfter=0;state.feedMore=false;state.feedInitialized=false;renderedSignature='';state.memoryQuery='';
  if($('#dialog').open)$('#dialog').close();navigation();$('#main').innerHTML='<div class="loading">正在读取项目工作台…</div>';
  // A previous read may still be in flight; its generation cannot commit here.
  while(state.refreshing)await new Promise(resolve => setTimeout(resolve,50));
  await refresh(true);
  if(state.route.project && state.route.view==='chat' && state.data.conversation)startProjectStream();
}

// Reads reconnect automatically; writes do not. A reconnect always begins with an
// authoritative persisted snapshot, so deltas cannot be appended twice.
let projectStream=null;
function stream(path,receive,status=()=>{}) {
  const controller=new AbortController();
  (async()=>{
    let delay=500;
    while(!controller.signal.aborted) {
      try {
        if(!state.token)await session();
        const target=path();
        const response=await fetch('/console/api/v1/'+target,{headers:{'x-feynman-console':state.token},credentials:'omit',cache:'no-store',signal:controller.signal});
        if(response.status===403)state.token=null;
        if(response.status===400 && target.startsWith('notifications/stream'))state.notificationAfter=null;
        await readSse(response,event=>{delay=500;status(true);receive(event);});
      } catch (_) {if(controller.signal.aborted)break;status(false);}
      await new Promise(resolve=>{const timeout=setTimeout(done,delay);function done(){clearTimeout(timeout);controller.signal.removeEventListener('abort',done);resolve();}controller.signal.addEventListener('abort',done,{once:true});});
      delay=Math.min(delay*2,10000);
    }
  })();
  return controller;
}
function startProjectStream() {
  const project=state.route.project,generation=state.generation;
  projectStream=stream(()=>`projects/${project}/stream`,e=>{
    if(generation!==state.generation)return;
    state.streamRevision++;
    if(e.name==='snapshot') {
      state.data.conversation=e.data.conversation;state.data.inbox=e.data.inbox;
      state.run=e.data.feed?.run || null;state.events=e.data.feed?.events || [];
      state.cursor=state.events.at(-1)?.sequence || 0;
    } else if(e.name==='inbox')state.data.inbox=e.data.inbox;
    else if(e.name==='run_event') {
      const {run,event}=e.data;
      if(state.events.some(x=>x.id===event.id))return;
      state.run=run;state.events.push(event);state.cursor=Math.max(state.cursor,event.sequence);
      if(event.kind==='agent_text_delta') {
        const message=state.data.conversation.messages.find(m=>m.role==='assistant'&&m.run_id===run.id);
        if(message)message.content+=event.payload.text || '';
      }
    }
    state.pending=state.pending.filter(i=>!i.id || !(state.data.inbox || []).some(m=>m.id===i.id));
    updateChat();
  },connected=>{if(generation===state.generation){state.projectLive=connected;const note=$('.composer-note');if(note)note.textContent=connected?'SSE 实时连接 · ⌘ / Ctrl + Enter 发送 · 收件确认不代表任务完成':'SSE 断线重连中 · 消息仍保存在 Server · 不自动重试发送';}});
}
stream(()=>`notifications/stream${state.notificationAfter===null?'':'?after='+state.notificationAfter}`,e=>{
  if(e.id!==undefined)state.notificationAfter=Number(e.id);
  if(e.name==='notifications') {
    const items=new Map(state.notifications.map(n=>[n.id,n]));for(const n of e.data.items)items.set(n.id,n);
    state.notifications=[...items.values()].sort((a,b)=>a.sequence-b.sequence).slice(-100);state.unread=e.data.unread;
    navigation();if(state.route.view==='notifications')renderNotifications();
    else notice(`${e.data.items.at(-1)?.project_name || '项目'} · ${notificationTitles[e.data.items.at(-1)?.kind] || '有新动态'}，可在消息中心查看。`);
  }
});
loadNotifications().then(()=>navigation()).catch(()=>{});

const field = (label,name,type='text',value='',hint='',attrs='') => `<label class="field">${label}<input name="${name}" type="${type}" value="${esc(value)}" ${attrs}><span class="field-hint">${hint}</span></label>`;
const area = (label,name,value='',hint='') => `<label class="field">${label}<textarea name="${name}" required maxlength="16000">${esc(value)}</textarea><span class="field-hint">${hint}</span></label>`;
const select = (label,name,options,value) => `<label class="field">${label}<select name="${name}">${options.map(([v,t]) => `<option value="${esc(v)}" ${v === value ? 'selected' : ''}>${esc(t)}</option>`).join('')}</select></label>`;
function modal(title,body,onSubmit,label='确认') {
  const generation=state.generation;
  $('#dialog-title').textContent=title;$('#dialog-body').innerHTML=body;$('#dialog-error').hidden=true;$('#dialog-submit').textContent=label;
  $('#dialog-submit').disabled=!onSubmit;$('#dialog-cancel').textContent=onSubmit ? '取消' : '关闭';
  $('#dialog-form').onsubmit=async e => {
    e.preventDefault();if(state.busy || !onSubmit)return;
    if(state.generation!==generation){$('#dialog').close();notice('项目上下文已变化，请重新打开操作。',true);return;}
    state.busy=true;$('#dialog-submit').disabled=true;$('#dialog-error').hidden=true;
    try {await onSubmit(new FormData(e.target));$('#dialog').close();notice('操作已完成。');await refresh(true);}
    catch(error){$('#dialog-error').textContent=error.message;$('#dialog-error').hidden=false;}
    finally {state.busy=false;$('#dialog-submit').disabled=false;}
  };
  $('#dialog').showModal();
}
function confirmation(title,description,action,label='确认操作') {modal(title,`<p class="muted">${esc(description)}</p>`,action,label);}
function dialogJSON(title,value) {modal(title,`<pre class="code-block">${esc(pretty(value))}</pre>`,null);}
function agentForm(id) {
  const existing=state.data.status.agents.find(a=>a.id === id), tools=state.data.tools;
  const capabilities=existing?.capabilities ?? (existing ? tools.map(t=>t.id) : []);
  const checks=tools.map(t=>`<label><input type="checkbox" name="capabilities" value="${t.id}" ${capabilities.includes(t.id)?'checked':''}><code>${esc(t.id)}</code></label>`).join('');
  const path=base()+'/agents'+(existing?'/'+existing.id:'');
  modal(existing?'Agent 状态与能力':'创建项目 Agent',field('名称','name','text',existing?.name||'main','','required maxlength="80"')+select('生命周期','state',[['active','活动'],['paused','暂停'],['archived','归档 · 保留历史，不可恢复原身份']],existing?.state||'active')+`<p class="tiny muted">声明能力（仍需项目 Grant 与有效账号绑定）</p><div class="checkbox-list">${checks}</div><div class="callout warning">归档后新 Agent 不继承旧会话与来源身份。${existing?.capabilities === null?'旧的迁移策略将明确为当前勾选的能力；未来新增能力不会自动继承。':''}</div>`,async f=>api(path,existing?'PUT':'POST',{name:f.get('name'),state:f.get('state'),is_default:true,capabilities:f.getAll('capabilities')}),'保存 Agent');
}
function scheduleForm() {
  const sources=state.data.sources,p=base(),agent=state.data.status.project.main_agent_id;
  modal('新建 Schedule',field('计划名称','title','text','','','required maxlength="120"')+area('发送给 Agent 的 Prompt','prompt','','明确目标、允许范围与结果要求。')+select('触发模式','mode',[['interval','周期定时'],['once','一次性'],['event','来源变化 · 固定收集窗口']],'interval')+`<div id="schedule-clock">${field('周期（秒）','interval','number','3600','最少 60 秒。','min="60" max="31536000"')}</div><div id="schedule-once" hidden>${field('触发时间（本机时区）','at','datetime-local')}</div><div id="schedule-event" hidden>${field('固定收集窗口（秒）','window','number','1800','从首个事件起按整条 Schedule 开窗。','min="1" max="86400"')}<p class="tiny muted">选择这个 Agent 的消息来源</p><div class="checkbox-list">${sources.filter(s=>s.agent_id===agent).map(s=>`<label><input type="checkbox" name="sources" value="${s.id}">${esc(s.name)}</label>`).join('') || '请先登记观测来源。'}</div></div>`+select('投递方式','delivery',[['queue','排队'],['steer_if_active','活动 Run 中插队']],'queue')+'<div class="callout">默认暂停。保存后，在计划列表中明确启用。</div>',async f=>{
    const mode=f.get('mode'),sources=mode==='event'?f.getAll('sources'):[];
    if(mode==='event'&&!sources.length)throw new Error('事件型 Schedule 至少选择一个来源。');
    if(mode==='once'&&!f.get('at'))throw new Error('请选择触发时间。');
    return api(p+'/schedules','POST',{title:f.get('title'),prompt:f.get('prompt'),agent_id:agent,enabled:false,delivery:f.get('delivery'),source_ids:sources,interval_seconds:mode==='interval'?Number(f.get('interval')):null,at:mode==='once'?new Date(f.get('at')).toISOString():null,collect_for_seconds:mode==='event'?Number(f.get('window')):null});
  },'创建 · 保持暂停');
  $('[name="mode"]',$('#dialog')).addEventListener('change',e=>{for(const [id,mode] of [['schedule-clock','interval'],['schedule-once','once'],['schedule-event','event']])$('#'+id).hidden=e.target.value!==mode;});
}
async function sourceForm() {
  const generation=state.generation;
  const p=base(),agent=state.data.status.project.main_agent_id,access=await api(p+'/access'),connections=(await api('access/connections')).connections;
  const dws=connections.filter(c=>c.provider==='dws'&&!c.disabled),now=new Date(),tomorrow=new Date(now.getTime()+86400000);
  if(state.generation!==generation)return;
  modal('登记观测来源',field('渠道名称','name','text','','','required maxlength="120"')+select('来源类型','kind',[['manual','手工来源 · 通过 CLI / API 采集'],['dws_messages','DWS 历史轮询'],['dws_events','DWS 被动事件监听']],'manual')+`<div id="source-dws" hidden>${select('DWS Connection','connection',dws.map(c=>[c.id,c.display_name || c.identity]),access.bindings.find(b=>b.provider==='dws')?.connection_id)}${select('目标类型','target_kind',[['group','群'],['open_dingtalk_id','个人 open_dingtalk_id'],['user','个人数字 user ID · 仅历史']],'group')}${field('稳定目标 ID','target','text','','群名或姓名不能代替 ID。')}${field('起始时间','start','text',now.toISOString(),'RFC3339，必须落在用户批准的 DWS scope 内。')}${field('截止时间','end','text',tomorrow.toISOString(),'固定、有界的认证资源范围。')}</div>`+field('轮询间隔（秒）','poll','number','60','','min="60" max="86400"')+'<div class="callout">默认关闭，不发起真实监听。DWS 需要已绑定账号、业务 Grant 与 CLI 配置的 dws-scope；不回退主机默认 profile。</div>',async f=>{
    const kind=f.get('kind'),isDws=kind!=='manual';
    if(isDws&&(!f.get('connection')||!f.get('target')))throw new Error('需要明确 DWS Connection 和稳定目标 ID。');
    return api(p+'/sources','POST',{name:f.get('name'),agent_id:agent,kind,enabled:false,poll_seconds:Number(f.get('poll')),connection_id:isDws?f.get('connection'):null,arguments:isDws?{target_kind:f.get('target_kind'),target_id:f.get('target'),start:f.get('start'),end:f.get('end')}:{}});
  },'登记 · 保持关闭');
  if(state.route.provider === 'dws'){$('[name="kind"]',$('#dialog')).value='dws_messages';$('#source-dws').hidden=false;}
  $('[name="kind"]',$('#dialog')).addEventListener('change',e=>$('#source-dws').hidden=e.target.value==='manual');
}
function bindForm(provider) {
  const p=base(),existing=state.data.access.bindings.find(b=>b.provider===provider),accountProvider=provider==='git'?'gitlab':provider;
  const choices=state.data.connections.filter(c=>c.provider===accountProvider&&!c.disabled).map(c=>[c.id,`${c.display_name || c.identity} · ${c.hostname || c.provider}`]);
  modal('绑定 '+provider+' 到本项目',select('已验证的 Connection','connection',choices,existing?.connection_id)+(['git','gitlab'].includes(provider)?field('仓库 namespace/repository','repository','text',existing?.repository||'slateteam/slate','','required'): '')+(provider==='drlee'?field('消息渠道 UUID（可选）','resource','text',existing?.resource_id||'','只有读取消息时才需要。由用户限定，不接受 Agent 自选渠道。'): '')+field('有效期（秒）','expires','number','86400','范围 60 秒至 7 天。','min="60" max="604800"')+'<div class="callout warning">绑定账号不授予工具。共享同一 Connection 的项目也需要各自 Grant。</div>',async f=>{
    if(!f.get('connection'))throw new Error('没有可选 Connection，请先在所有者 CLI 完成认证。');
    return api(p+'/access/bindings','POST',{provider,connection_id:f.get('connection'),repository:f.get('repository')||null,resource_id:f.get('resource')||null,ttl_seconds:Number(f.get('expires'))});
  },'确认项目绑定');
}
async function action(e) {
  const b=e.target.closest('[data-action]');if(!b || state.busy)return;
  const act=b.dataset.action,p=base(),id=safeId(b.dataset.id),operation=b.dataset.operation;
  try {
    if(act==='reload'){state.token=null;await refresh(true);}
    else if(act==='read-notification'||act==='open-notification') {
      const n=state.notifications.find(n=>n.id===id);if(!n)return;
      await api(`notifications/${id}/read`,'POST');await loadNotifications();navigation();
      if(act==='open-notification')navigate(n.project_id,'chat');else renderNotifications();
    }
    else if(act==='new-project')modal('创建一个项目',field('项目名称','name','text','','','required maxlength="120"')+'<label class="field"><span><input type="checkbox" name="agent" checked> 同时创建默认 Main Agent</span><span class="field-hint">不创建沙盒、不授予工具、不启动模型。</span></label>',async f=>{
      const r=await api('projects','POST',{name:f.get('name'),without_agent:!f.has('agent')});navigate(r.project.id);
    },'创建项目');
    else if(act==='suggest'){if($('#prompt')){$('#prompt').value=b.dataset.prompt;state.drafts.set(state.route.project,b.dataset.prompt);$('#prompt').focus();}}
    else if(act==='new-agent'||act==='edit-agent')agentForm(id);
    else if(act==='new-task')modal('新建持久任务',field('任务名称','title','text','','','required maxlength="120"')+area('任务目标与边界','prompt')+'<div class="callout warning">保存后任务进入队列，Agent 空闲时可能开始执行。请明确允许的操作。</div>',async f=>api(p+'/tasks','POST',{title:f.get('title'),prompt:f.get('prompt')}),'提交到任务队列');
    else if(act==='cancel-task')confirmation('取消排队任务','只取消这个任务，不删除历史。',()=>api(p+'/tasks/'+id,'DELETE'));
    else if(act==='new-schedule')scheduleForm();
    else if(act==='toggle-schedule')confirmation(b.dataset.enabled==='true'?'启用 Schedule':'暂停 Schedule','启用会在满足触发条件时向固定 Agent 投递 Prompt；暂停不撤销已接收的运行。',()=>api(p+'/schedules/'+id,'PUT',{enabled:b.dataset.enabled==='true'}));
    else if(act==='deliveries')dialogJSON('Schedule 投递记录',await api(p+'/schedules/'+id+'/deliveries'));
    else if(act==='run-events')dialogJSON('Run 事件',await api(p+'/runs/'+id+'/events?after=0'));
    else if(act==='new-source')await sourceForm();
    else if(act==='toggle-source')confirmation(b.dataset.enabled==='true'?'启用来源收取':'暂停来源收取','启用可能开始真实的外部消息读取；独立查看窗口不会改变此设置。',()=>api(p+'/sources/'+id,'PUT',{enabled:b.dataset.enabled==='true'}));
    else if(act==='feed-more')await refresh();
    else if(act==='create-workspace')confirmation('创建项目 Workspace',b.dataset.provider==='opensandbox'?'使用当前运营配置，在本机 Docker 创建独立容器与持久卷。不使用 GKE。':'Agent 将在本机项目目录执行，不具有容器隔离。请明确接受。',()=>api(p+'/workspace','POST',{provider:b.dataset.provider}),'确认创建');
    else if(act==='workspace-action'){
      if(operation==='refresh'){state.busy=true;b.disabled=true;try{await api(p+'/workspace/actions','POST',{action:'refresh'});await refresh(true);}finally{state.busy=false;if(b.isConnected)b.disabled=false;}}
      else confirmation('Workspace · '+({stop:'暂停容器',resume:'恢复运行',restore:'恢复缺失容器',renew:'续期'}[operation]||operation),'只操作本项目已绑定的执行环境。不会删除持久卷；有活动 Run 时可能被后端拒绝。',()=>api(p+'/workspace/actions','POST',{action:operation}));
    }
    else if(act==='bind')bindForm(b.dataset.provider);
    else if(act==='unbind')confirmation('解除项目绑定','阻止这个项目的新调用，不吊销外部 Token，也不删除已获取的事实。',()=>api(p+'/access/bindings/'+b.dataset.provider,'DELETE'));
    else if(act==='grant'){
      const t=state.data.tools.find(t=>t.id===b.dataset.tool),revoke=t.granted;
      confirmation(`${revoke?'撤销':'授予'} ${t.id}`,`${t.description}\n仅影响当前项目；业务许可仍受 Agent 声明能力、账号绑定和外部账号范围约束。`,()=>api(p+'/external-tools/'+t.id,revoke?'DELETE':'PUT'));
    }
    else if(act==='dws-login')confirmation('发起 DWS Device 授权','需要已授予 dws.auth.device。Server 发起设备登录，用户自行在钉钉页面确认；不会自动登记 Connection、绑定或授予业务权限。',()=>api(p+'/dws-authorizations','POST',{reason:'用户在本地 Console 为项目授权 DWS'}));
    else if(act==='dws-check')confirmation('检查 DWS 认证状态','使用该项目的私有设备缓存进行新鲜验证，不回退个人默认授权。',()=>api(p+'/dws-authorizations/'+id+'/check','POST',{}));
    else if(act==='approval'){
      const run=state.run?.id,request=b.dataset.request,decision=b.dataset.decision;
      if(!run||!request||request.length>512)throw new Error('审批记录不完整，请刷新。');
      const event=state.events.find(e=>e.payload.requestId===request);
      confirmation(decision==='accept'?'允许这次原生操作':'拒绝这次操作',event?.payload.summary||'仅响应这个 Run 的当前审批，不授予未来操作。',()=>api(p+'/runs/'+run+'/approvals/'+encodeURIComponent(request),'POST',{decision}),decision==='accept'?'允许本次':'拒绝');
    }
  } catch(error){notice(error.message,true);}
}
document.addEventListener('click',action);
for(const id of ['dialog-close','dialog-cancel'])$('#'+id).addEventListener('click',()=>{if(!state.busy)$('#dialog').close();});
$('#dialog').addEventListener('cancel',e=>{if(state.busy)e.preventDefault();});
$('#refresh').addEventListener('click',()=>refresh(true));
$('#theme-toggle').addEventListener('click',()=>{
  const next=document.documentElement.dataset.theme==='dark'?'light':'dark';document.documentElement.dataset.theme=next;
  try{localStorage.setItem('feynman-console-theme',next);}catch(_){}
});
try{document.documentElement.dataset.theme=localStorage.getItem('feynman-console-theme')||(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light');}catch(_){}
window.addEventListener('hashchange',routeChanged);
document.addEventListener('visibilitychange',()=>{if(!document.hidden)refresh();});
setInterval(()=>{if(!document.hidden&&!state.busy&&!$('#dialog').open && !(state.route.view==='chat'&&state.projectLive))refresh();},2000);
await routeChanged();
