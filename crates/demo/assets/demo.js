// Only injected by ds-demo. The normal application is not changed.
const meta=await fetch('/demo/meta').then(r=>r.json());
let step=Number(meta.step||0),busy=false;
let taskId=meta.task_id;
const guide=document.createElement('aside');guide.id='demo-guide';document.body.append(guide);
const steps=[
  ['扫描合成桌面','0:00–0:12','课程文档、论文、音乐、视频和图片混在一起。所有内容均为合成；快捷方式自动保留。','扫描并进入权限'],
  ['权限与整体目录','0:12–0:25','INI 不发给模型；文档和媒体允许有限取样。成绩与工具文件夹不拆散，把“课程资料库”设为复用容器。','保存权限与容器，进入目标树'],
  ['AI 建议目标结构','0:25–0:40','真实运行分组检查和证据提取，本地模拟模型提供音乐一级目录及课程分类建议。点击建议复选框可观察父子依赖。','生成离线 AI 目录建议'],
  ['采纳与 few-shot','0:40–0:50','在主页面查看红绿差异，可取消某项。演示快捷键采纳当前整组建议，并把 progress1.docx 作为课程项目的文件示例。','采纳建议并设置文件示例'],
  ['并行生成计划','0:50–1:05','每批 2 项、最多 3 路。调用真实 read_file_evidence 和 submit_classifications，完成批次持久化；不会移动文件。','生成 AI 计划'],
  ['用一句话调整','1:05–1:18','输入预设反馈：“把视频从其他移出来，放到独立目录”。助手生成可采纳差异；旁边原计划保持不变。','向 AI 提出视频独立分类'],
  ['直接更新整理后','1:18–1:28','采纳后由 Rust 完成文件位置规划，更新原有“整理后”预览。此时仍未执行磁盘操作。','采纳调整并更新计划'],
  ['确认实际执行','1:28–1:42','对比前后目录树；操作清单支持折叠与每页 20 项。接下来只移动演示目录中的合成文件，成绩文件夹整体移动。','确认并执行演示文件移动'],
  ['清理建议与回收','1:42–1:52','只对演示 render.log 执行回收；不永久删除。先运行清理复核，再展示回收记录。','确认将演示日志移入回收站'],
  ['一键撤销与追溯','1:52–2:00','先从回收站恢复，再撤销整理。历史保留对话、工具轨迹、模拟 Token 与费用。导出完整任务归档可在补充功能中操作。','恢复回收文件并撤销整理'],
  ['演示完成','约 2 分钟','原文件位置已恢复，所有操作均有轨迹。重启仍保留本次进度；点击“重置固定场景”才会清空演示记录、回到同一起点。','查看任务历史'],
];
function el(tag,text,cls){const n=document.createElement(tag);if(text)n.textContent=text;if(cls)n.className=cls;return n;}
function button(label,fn,cls){const b=el('button',label,cls);b.onclick=fn;return b;}
function render(){
  guide.replaceChildren();guide.append(el('div','DownloadSweeper · 可操作演示','demo-brand'),el('div','离线预设 AI · 无需 Key · 实际费用 ¥0','demo-badge'));
  guide.append(el('p','AI 回答和 Token / 金额均为模拟；扫描、内容取样、计划、文件移动及恢复使用真实 Rust 引擎。','demo-disclosure'));
  const s=steps[Math.min(step,10)];guide.append(el('small',`${Math.min(step+1,10)} / 10　讲解节奏 ${s[1]}`),el('h2',s[0]),el('p',s[2]));
  const dots=el('div',null,'demo-dots');for(let i=0;i<10;i++)dots.append(el('i',null,i<step?'done':i===step?'current':''));guide.append(dots);
  const main=button(busy?'正在执行，请稍候…':s[3],()=>step>=10?window.dsDemoBridge?.page('history'):run(next),'demo-primary');main.disabled=busy;guide.append(main);
  const status=el('p','', 'demo-status');status.id='demo-status';guide.append(status);
  const reset=button('重置固定场景 · 从头演示',resetDemo);reset.disabled=busy;guide.append(reset);
  const extras=el('details');extras.append(el('summary','补充演示 / 自由操作'));
  for(const [label,fn] of [
    ['查看实际发给模型的媒体证据',showEvidence],
    ['打开可移动 AI 浮窗',()=>window.dsDemoBridge?.assistant()],
    ['查看历史、Token 和运行记录',()=>window.dsDemoBridge?.page('history')],
    ['导出当前任务完整 JSON 归档',()=>run(exportArchive)],
    ['文件名重生：读取标题生成名称',()=>run(renameDemo)],
    ['返回桌面整理任务',()=>{taskId=meta.task_id;window.dsDemoBridge?.open(taskId);}],
    ['暂停当前后台任务',()=>cancel().catch(e=>notify(e.message))],
    ['显示演示目录位置',()=>notify(meta.root)],
  ])extras.append(button(label,fn));
  guide.append(extras,el('p','固定样本、固定 AI 建议。重启继续同一进度，重置后可重复同一流程；自由修改后可按原生流程继续，或重置回到标准场景。','demo-foot'));
}
function notify(message){const e=document.querySelector('#demo-status');if(e)e.textContent=message;}
async function current(){const r=await fetch('/api/tasks/'+taskId);if(!r.ok)throw Error('无法读取任务');return r.json();}
async function action(action,args={}){
  const b=await fetch('/api/bootstrap').then(r=>r.json());const t=await current();
  const r=await fetch('/api/action',{method:'POST',headers:{'Content-Type':'application/json','x-ds-token':b.token},body:JSON.stringify({action,task_id:taskId,revision:t.revision,...args})});
  const v=await r.json();if(!r.ok)throw Error(v.error||'操作失败');
  if(v.job){await window.dsDemoBridge?.open(taskId);return wait(v.job.id);}
  return v;
}
async function wait(id){
  for(let i=0;i<1800;i++){
    const response=await fetch('/api/jobs').then(r=>r.json());const jobs=Array.isArray(response)?response:(response.jobs||[]);
    const job=jobs.find(j=>j.id===id);if(job){notify(job.message||'运行中');
      if(!['running','pausing'].includes(job.status)){if(job.status!=='completed')throw Error(job.error||job.recovery_note||job.message);return job;}}
    await new Promise(r=>setTimeout(r,150));
  }throw Error('等待超时；请查看原生运行记录。');
}
async function run(fn){if(busy)return;busy=true;render();try{await fn();await window.dsDemoBridge?.open(taskId);}catch(e){busy=false;render();notify(e.message);return;}busy=false;render();if(fn===next)focusStep();}
function focusStep(){
  const selector={1:'.permission-layout',2:'[aria-label="目标目录节点画布"]',3:'main .proposal',5:'.comparison',6:'.review-editor',7:'.comparison',8:'.cleanup-panel',9:'.cleanup-panel'}[step];
  if(selector)requestAnimationFrame(()=>requestAnimationFrame(()=>document.querySelector(selector)?.scrollIntoView({behavior:'smooth',block:'start'})));
}
async function phase(n){let t=await current();while(t.phase<n){await action('advance');t=await current();}if(t.phase>n)throw Error('当前已超过该演示阶段，可使用主界面继续操作。');}
async function merge(){const t=await current();if(!t.proposal)throw Error('没有待采纳建议；请在主界面检查当前任务。');await action('proposal',{proposal_id:t.proposal.id,scene:t.proposal.scene,ids:t.proposal.changes.map(c=>c.id)});}
async function next(){
  switch(step){
    case 0:await action('scan');await phase(1);break;
    case 1:{const t=await current();await action('permissions',{permissions:t.permissions});const dir=t.entries.find(e=>e.name==='课程资料库');await action('directory',{id:dir.id,class:'container'});await phase(2);break;}
    case 2:await action('suggest_tree',{message:'请根据文件内容完善课程和音乐类别，复用课程资料库，保持成绩与工具文件夹完整。'});break;
    case 3:{if((await current()).proposal)await merge();const t=await current();const example=t.entries.find(e=>e.name==='progress1.docx');const node=t.nodes.find(n=>n.name==='课程项目');if(node&&example){node.examples=[example.id];await action('tree',{nodes:t.nodes});}await phase(3);break;}
    case 4:await action('plan_ai',{batch_size:2,thinking:false});await phase(4);break;
    case 5:await action('chat',{scene:'review',message:'感觉应该把视频从“其他”类内移出来，放在新的单独一个“视频/剪辑素材”目录内。'});window.dsDemoBridge?.assistant();break;
    case 6:await merge();window.dsDemoBridge?.assistant(false);break;
    case 7:{const t=await current();if(t.phase<5){await action('review',{selected:t.operations.filter(o=>o.selected).map(o=>o.id),reviewed:true});await phase(5);}await action('execute');break;}
    case 8:{await action('cleanup_ai');const t=await current();const c=t.cleanup.find(c=>c.path?.endsWith('render.log'));if(!c)throw Error('演示日志不在清理候选中，请查看清理建议。');await action('cleanup_trash',{selected:[c.original_id],confirmed:true});break;}
    case 9:{const t=await current();for(const batch of new Set(t.recycled.filter(r=>r.status!=='restored').map(r=>r.batch)))await action('cleanup_restore',{batch});await action('rollback');break;}
    default:window.dsDemoBridge?.page('history');return;
  }
  step++;await saveControl({step});
}
async function saveControl(value){const b=await fetch('/api/bootstrap').then(r=>r.json());const r=await fetch('/demo/control',{method:'POST',headers:{'Content-Type':'application/json','x-ds-token':b.token},body:JSON.stringify(value)});const reply=await r.json();if(!r.ok)throw Error(reply.error||'演示进度保存失败');return reply;}
async function resetDemo(){
  if(busy)return;busy=true;render();notify('正在恢复合成文件并重置固定场景…');
  try{
    await saveControl({reset:true});
    for(let i=0;i<120;i++){
      await new Promise(r=>setTimeout(r,500));
      try{const r=await fetch('/demo/meta',{cache:'no-store'});if(r.ok){const fresh=await r.json();if(fresh.generation>meta.generation){for(const key of Object.keys(sessionStorage))if(key.startsWith('ds-graph-view-v3:'))sessionStorage.removeItem(key);location.hash='task='+fresh.task_id;location.reload();return;}}}catch{}
    }
    throw Error('重置未完成，请查看启动窗口；被修改的样本或新增文件会保留，不会强制删除。');
  }catch(e){busy=false;render();notify(e.message);}
}
async function exportArchive(){const j=await action('archive_export');const r=await fetch('/api/jobs/'+j.id+'/result');if(!r.ok)throw Error('归档读取失败');const blob=await r.blob();const url=URL.createObjectURL(blob);const a=el('a');a.href=url;a.download='DownloadSweeper-演示完整归档.json';a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}
async function cancel(){const b=await fetch('/api/bootstrap').then(r=>r.json());if(!b.job)throw Error('目前没有运行中的任务。运行 AI 规划时可使用主界面的暂停按钮；历史显示实际可恢复条件。');await action('cancel',{job_id:b.job.id});}
async function renameDemo(){
  const b=await fetch('/api/bootstrap').then(r=>r.json());if(b.job)throw Error('请先完成当前任务。');
  const tasks=await fetch('/api/tasks').then(r=>r.json());const existing=tasks.find(t=>t.mode==='rename'&&t.root===meta.root);
  if(existing){taskId=existing.id;return;}
  const r=await fetch('/api/action',{method:'POST',headers:{'Content-Type':'application/json','x-ds-token':b.token},body:JSON.stringify({action:'create',root:meta.root,mode:'rename'})});const t=await r.json();if(!r.ok)throw Error(t.error);taskId=t.id;
  await action('scan');await phase(2);await action('rename_scope',{extensions:['txt'],web_search:false});await phase(3);await action('rename');
}
async function showEvidence(){
  const data=await fetch('/demo/evidence').then(r=>r.json());const modal=el('dialog',null,'demo-modal');modal.append(el('h2','实际证据与模拟回答的边界'),el('p',data.notice),el('p',`请求 ${data.requests} 次；已观察到最多 ${data.maximum} 路并发。`));
  if(!data.items.length)modal.append(el('p','先执行 AI 计划或文件名重生，查看真正提取并传入请求的内容。'));
  for(const item of data.items){if(item.kind==='image'){const img=el('img');img.src=item.value;img.alt='从合成文件实际提取的图像证据';modal.append(img);}else{modal.append(el('pre',item.value));}}
  modal.append(button('关闭',()=>{modal.close();modal.remove();}));document.body.append(modal);modal.showModal();
}
render();
