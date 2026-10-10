// SSE over fetch: the CSRF token stays in a header, never in a URL or event log.
export async function readSse(response, receive) {
  if (!response.ok || !response.headers.get('content-type')?.includes('text/event-stream')) throw new Error('SSE connection rejected');
  const reader = response.body.getReader(), decoder = new TextDecoder();
  let pending = '', name = 'message', id, data = [];
  const line = value => {
    if (value === '') {
      if (data.length) receive({name, id, data:JSON.parse(data.join('\n'))});
      name='message';id=undefined;data=[];return;
    }
    if (value.startsWith(':')) return;
    const colon=value.indexOf(':'), key=colon<0?value:value.slice(0,colon);
    let content=colon<0?'':value.slice(colon+1); if(content.startsWith(' '))content=content.slice(1);
    if(key==='event')name=content;
    if(key==='id'&&!content.includes('\0'))id=content;
    if(key==='data')data.push(content);
  };
  try {
    while (true) {
      let timeout;
      const {value,done}=await Promise.race([reader.read(),new Promise((_,reject)=>{timeout=setTimeout(()=>{reader.cancel().catch(()=>{});reject(new Error('SSE heartbeat timed out'));},45000);})]).finally(()=>clearTimeout(timeout)); if(done)break;
      pending+=decoder.decode(value,{stream:true});
      if(pending.length>16*1024*1024)throw new Error('SSE frame too large');
      let end;
      while((end=pending.indexOf('\n'))>=0) {line(pending.slice(0,end).replace(/\r$/,''));pending=pending.slice(end+1);}
    }
    throw new Error('SSE disconnected');
  } finally {await reader.cancel().catch(()=>{});reader.releaseLock();}
}
