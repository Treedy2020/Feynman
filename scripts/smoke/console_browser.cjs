/* Real browser clicks against an isolated Rust fixture; preview mode is read-only.
 * Select an installed Playwright module via FEYNMAN_PLAYWRIGHT_MODULE. No npm install.
 */
const assert = require('node:assert/strict');
const path = require('node:path');
const { chromium } = require(process.env.FEYNMAN_PLAYWRIGHT_MODULE || 'playwright');
const url = new URL(process.env.FEYNMAN_CONSOLE_URL || 'http://127.0.0.1:4318/console/');
assert.equal(url.protocol, 'http:');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname));
const preview = process.argv.includes('--preview');
if (!preview) assert.equal(process.env.FEYNMAN_CONSOLE_FIXTURE, 'true', 'Mutation tests require an explicitly isolated fixture');
const output = path.resolve(__dirname, '../../output');

(async () => {
  const browser = await chromium.launch({channel:'chrome',headless:true});
  const context = await browser.newContext({viewport:{width:1440,height:1000},colorScheme:'light'});
  const page = await context.newPage();
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  try {
    await page.goto(url.href);
    await page.getByRole('heading',{name:'让项目持续向前。'}).waitFor();
    assert.equal(await page.locator('.integration-mini').count(),5,'Home shows the real provider catalog');
    if (preview) {
      await page.screenshot({path:path.join(output,'console-projects.png'),fullPage:true});
      await page.getByRole('link',{name:'接入能力',exact:true}).click();
      await page.getByRole('heading',{name:'系统接入能力',exact:true}).waitFor();
      assert.equal(await page.locator('.integration-detail').count(),5);
      await page.screenshot({path:path.join(output,'console-integrations.png'),fullPage:true});
      await page.setViewportSize({width:390,height:844});
      assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Catalog has no mobile overflow');
      await page.screenshot({path:path.join(output,'console-integrations-mobile.png'),fullPage:true});
      await page.setViewportSize({width:1440,height:1000});
      const project=process.env.FEYNMAN_CONSOLE_PREVIEW_PROJECT;
      if(project) {
        await page.goto(url.href+'#'+new URLSearchParams({project,view:'overview'}));
        await page.getByRole('heading',{name:'项目 Agent',exact:true}).waitFor();
        await page.screenshot({path:path.join(output,'console-workspace.png'),fullPage:true});
        for(const view of ['access','chat','environment','memory','sources','inbox','schedules','tasks','agents','workspace']){
          await page.goto(url.href+'#'+new URLSearchParams({project,view}));
          await page.locator('#main .page-heading').waitFor();
          assert.equal(await page.locator('.error-state').count(),0,view);
          if(view==='access'){
            await page.screenshot({path:path.join(output,'console-project-integrations.png'),fullPage:true});
            await page.setViewportSize({width:390,height:844});
            assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Project capability groups have no mobile overflow');
            await page.setViewportSize({width:1440,height:1000});
          }
          if(view==='chat')await page.screenshot({path:path.join(output,'console-chat.png'),fullPage:true});
        }
      }
      assert.deepEqual(errors,[]);
      console.log(JSON.stringify({read_only_preview:true,views_checked:project?13:2,page_errors:errors,screenshots:output}));
      return;
    }
    await page.getByRole('link',{name:'接入能力',exact:true}).click();
    await page.getByRole('heading',{name:'系统接入能力',exact:true}).waitFor();
    assert.equal(await page.locator('.integration-detail').count(),5);
    await page.locator('.provider-tabs').getByRole('link',{name:'DWS / 钉钉',exact:true}).click();
    await page.locator('.provider-tabs [aria-current=page]').filter({hasText:'DWS / 钉钉'}).waitFor();
    await page.locator('.integration-detail[data-provider=dws]').waitFor();
    assert.equal(await page.locator('.integration-detail').count(),1);
    await page.getByText('历史轮询 / 事件监听',{exact:true}).waitFor();
    await page.locator('.provider-tabs').getByRole('link',{name:'Git',exact:true}).click();
    await page.locator('.integration-detail[data-provider=git]').waitFor();
    await page.getByText('尚未接入自动采集，仅支持工具按需读取',{exact:true}).waitFor();
    await page.reload();await page.locator('.integration-detail[data-provider=git]').waitFor();
    assert.equal(await page.locator('.integration-detail').count(),1,'Provider filter survives reload');
    await page.getByRole('link',{name:'所有项目',exact:true}).click();
    await page.getByRole('heading',{name:'让项目持续向前。'}).waitFor();
    const name='Browser <img src=x onerror=alert(1)> Project';
    await page.getByRole('button',{name:'＋ 新建项目'}).click();
    await page.locator('#dialog [name=name]').fill(name);
    await page.locator('#dialog [name=agent]').uncheck();
    await page.locator('#dialog-submit').click();
    await page.getByRole('heading',{name,exact:true}).waitFor();
    let project=new URLSearchParams(new URL(page.url()).hash.slice(1)).get('project');
    assert.ok(project);
    assert.equal(await page.locator('#main img').count(),0,'Project name must be inert text');
    await page.getByRole('link',{name:'Agent 设置',exact:true}).click();
    await page.getByRole('button',{name:'＋ 创建 Agent'}).click();
    await page.locator('#dialog-submit').click();
    await page.getByRole('heading',{name:'main',exact:true}).waitFor();
    await page.getByRole('link',{name:'观测消息',exact:true}).click();
    await page.getByRole('button',{name:'＋ 登记来源'}).click();
    await page.locator('#dialog [name=name]').fill('Synthetic channel');
    await page.locator('#dialog-submit').click();
    await page.locator('.channel-button strong').filter({hasText:'Synthetic channel'}).waitFor();
    async function owner(method,suffix,body){
      const r=await fetch(url.origin+'/v1/'+suffix,{method,headers:body?{'content-type':'application/json'}:{},body:body?JSON.stringify(body):undefined});
      assert.ok(r.ok,'fixture owner request '+r.status);return r.status===204?null:r.json();
    }
    const source=(await owner('GET',`projects/${project}/sources`)).sources[0];
    assert.equal(source.enabled,false);
    await owner('POST',`projects/${project}/sources/${source.id}/records`,{records:[{event_id:'fixture-1',occurred_at:new Date().toISOString(),body:{content:'Synthetic observation <img src=x onerror=alert(1)>'}}]});
    await page.getByText('Synthetic observation <img src=x onerror=alert(1)>',{exact:true}).waitFor();
    assert.equal(await page.locator('#feed-records img').count(),0);
    await page.locator('.channel-group[data-provider=manual]').getByRole('heading',{name:'手工来源'}).waitFor();
    await page.locator('.provider-tabs').getByRole('link',{name:'DWS / 钉钉',exact:true}).click();
    await page.getByText('本来源暂无渠道，登记后默认关闭，不自动监听。',{exact:true}).waitFor();
    assert.equal(await page.locator('.channel-group[data-provider=manual]').count(),0);
    assert.equal(await page.getByText('Synthetic observation <img src=x onerror=alert(1)>',{exact:true}).count(),0,'Provider filtering applies to messages, not just the channel list');
    await page.locator('.provider-tabs').getByRole('link',{name:'手工来源',exact:true}).click();
    await page.getByText('Synthetic observation <img src=x onerror=alert(1)>',{exact:true}).waitFor();
    const popupPromise=page.waitForEvent('popup');
    await page.getByRole('link',{name:'独立窗口 ↗'}).click();
    const popup=await popupPromise;
    await popup.getByText('仅新增消息 · 2s 轮询').waitFor();
    assert.equal(await popup.getByText('Synthetic observation <img src=x onerror=alert(1)>',{exact:true}).count(),0);
    await owner('POST',`projects/${project}/sources/${source.id}/records`,{records:[{event_id:'fixture-2',occurred_at:new Date().toISOString(),body:{content:'Second synthetic observation'}}]});
    await popup.getByText('Second synthetic observation',{exact:true}).waitFor();
    await popup.close();
    assert.equal((await owner('GET',`projects/${project}/sources`)).sources[0].enabled,false);
    await page.getByRole('link',{name:'Schedule',exact:true}).click();
    await page.getByRole('button',{name:'＋ 新建 Schedule'}).click();
    await page.locator('#dialog [name=title]').fill('Browser hourly inspection');
    await page.locator('#dialog [name=prompt]').fill('Inspect only; no external writes');
    await page.locator('#dialog-submit').click();
    await page.getByRole('heading',{name:'Browser hourly inspection'}).waitFor();
    assert.equal((await owner('GET',`projects/${project}/schedules`)).schedules[0].enabled,false);
    await page.getByRole('button',{name:'启用',exact:true}).click();await page.locator('#dialog-submit').click();
    await page.getByRole('button',{name:'暂停',exact:true}).waitFor();
    await page.getByRole('button',{name:'暂停',exact:true}).click();await page.locator('#dialog-submit').click();
    await page.getByRole('button',{name:'启用',exact:true}).waitFor();
    await page.getByRole('link',{name:'Agent 对话',exact:true}).click();
    await page.locator('.composer-note').filter({hasText:'SSE 实时连接'}).waitFor();
    assert.equal(await page.locator('#delivery').inputValue(),'steer_if_active');
    await page.route('**/console/api/v1/projects/*/inbox',async route=>{
      if(route.request().method()==='POST')await new Promise(resolve=>setTimeout(resolve,600));
      await route.continue();
    });
    await page.locator('#prompt').fill('BLOCK: read-only inspection requiring approval');
    await page.locator('#composer button').click();
    await page.locator('.receipt').getByText('发送中',{exact:true}).waitFor({timeout:500});
    await page.getByRole('button',{name:'允许本次操作'}).waitFor();
    await page.unroute('**/console/api/v1/projects/*/inbox');
    await page.getByRole('link',{name:'消息中心',exact:true}).click();
    await page.getByRole('heading',{name:'需要你批准操作',exact:true}).waitFor();
    await page.getByRole('button',{name:'标为已读',exact:true}).click();
    assert.equal((await owner('GET',`projects/${project}/conversation`)).active_run.status,'waiting_approval','Read must not approve');
    await page.getByRole('button',{name:'打开项目',exact:true}).click();
    await page.getByRole('button',{name:'允许本次操作'}).waitFor();
    await page.screenshot({path:path.join(output,'console-fixture-approval.png'),fullPage:true});
    await page.getByRole('button',{name:'允许本次操作'}).click();await page.locator('#dialog-submit').click();
    await page.getByText('fixture reply',{exact:true}).waitFor();
    const before=await owner('GET',`projects/${project}/conversation`);
    await page.reload();await page.getByText('fixture reply',{exact:true}).waitFor();
    const after=await owner('GET',`projects/${project}/conversation`);
    assert.equal(after.conversation.runtime_thread_id,before.conversation.runtime_thread_id);
    assert.equal(after.runs.length,1);
    await page.locator('#prompt').fill('FAIL_START: simulate a startup failure without any tool execution');
    await page.locator('#composer button').click();
    await page.locator('.message-failure').getByText(/fixture runtime startup failure/).waitFor();
    await page.locator('.message-failure').getByText(/系统没有自动重试/).waitFor();
    assert.equal((await owner('GET',`projects/${project}/conversation`)).runs.length,2);
    await page.getByRole('link',{name:'外部授权',exact:true}).click();
    await page.getByRole('heading',{name:'项目外部接入',exact:true}).waitFor();
    assert.equal(await page.locator('.integration-group').count(),5);
    const git=page.locator('.integration-group[data-provider=git]');
    assert.equal(await git.locator('tbody tr').count(),2);
    assert.equal(await git.locator('code').filter({hasText:/^gitlab\./}).count(),0);
    await page.locator('.provider-tabs').getByRole('link',{name:'Drlee',exact:true}).click();
    await page.locator('.provider-tabs [aria-current=page]').filter({hasText:'Drlee'}).waitFor();
    await page.locator('.integration-group[data-provider=drlee]').waitFor();
    assert.equal(await page.locator('.integration-group').count(),1);
    assert.equal(await page.locator('.integration-group tbody tr').count(),4);
    await page.locator('.provider-tabs').getByRole('link',{name:'全部来源',exact:true}).click();
    await page.locator('.integration-group[data-provider=dws]').waitFor();
    const grant=page.locator('tr').filter({has:page.locator('code').filter({hasText:/^external\.access$/})});
    await grant.getByRole('button',{name:'授予',exact:true}).click();await page.locator('#dialog-submit').click();
    await grant.getByRole('button',{name:'撤销',exact:true}).waitFor();
    await grant.getByRole('button',{name:'撤销',exact:true}).click();await page.locator('#dialog-submit').click();
    await grant.getByRole('button',{name:'授予',exact:true}).waitFor();
    for(const view of ['memory','environment','inbox','tasks','workspace']){
      await page.goto(url.href+'#'+new URLSearchParams({project,view}));
      await page.locator('#main .page-heading').waitFor();
      assert.equal(await page.locator('.error-state').count(),0,view);
    }
    await page.goto(url.href+'#'+new URLSearchParams({project,view:'overview'}));
    await page.getByRole('heading',{name,exact:true}).waitFor();
    await page.setViewportSize({width:390,height:844});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'No horizontal overflow on mobile');
    await page.screenshot({path:path.join(output,'console-mobile.png'),fullPage:true});
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({browser_smoke:'passed',project,checks:['home_provider_catalog','system_catalog_filter_reload','create_project_without_agent','create_agent','inert_external_text','manual_source_registration','channel_provider_filter','independent_live_window','paused_schedule_create_toggle','optimistic_send_receipt','sse_chat','notification_center_read_does_not_approve','durable_inbox_chat','native_approval','conversation_reload','visible_failed_reply_without_retry','project_capability_groups','grant_revoke','all_read_views','mobile_layout'],page_errors:errors}));
  } catch(error) {
    await page.screenshot({path:path.join(output,'console-browser-failure.png'),fullPage:true});
    throw error;
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});
