// Activity is execution observed by ETW, never an inference from an affinity mask.
function processActivity(activity) {
  if(activity?.status!=='running')return null;
  const map=new Map();
  for(const core of activity.cores||[])for(const row of core.rows||[]){
    if(row.ms<=0 || row.usage<=0)continue;
    if(!map.has(row.pid))map.set(row.pid,[]);
    if(!map.get(row.pid).includes(core.id))map.get(row.pid).push(core.id);
  }
  for(const list of map.values())list.sort((a,b)=>a-b);
  return map;
}
function compareProcesses(a,b,key,ascending) {
  let result=0;
  if(key==='name')result=a.name.localeCompare(b.name,undefined,{sensitivity:'base',numeric:true});
  else if(key==='affinity') {
    // Inaccessible values stay last in both directions. Compare CPU IDs numerically.
    if(a.affinity===null || b.affinity===null)return a.affinity===b.affinity?a.pid-b.pid:a.affinity===null?1:-1;
    const ac=[...a.affinity].sort((a,b)=>a-b),bc=[...b.affinity].sort((a,b)=>a-b);
    for(let i=0;i<Math.min(ac.length,bc.length);i++){if(ac[i]!==bc[i]){result=ac[i]-bc[i];break;}}
    if(!result)result=ac.length-bc.length;
  } else result=(a[key]||0)-(b[key]||0);
  return (ascending?result:-result)||a.pid-b.pid;
}
const cpuHistory=new Map();
let diagnosticTime=0;
function freshSensor(s,time=Date.now()/1000) {return Number.isFinite(s.value)&&Number.isFinite(s.sample_time)&&time-s.sample_time<20&&time>=s.sample_time-2;}
function temperatureReference(s) {
  return s.kind==='Temperature'&&/distance|tj\s*max|margin|sensor\s*resolution|thermal.*(?:limit|threshold)|(?:critical|warning).*temperature|temperature.*(?:limit|threshold)|hysteresis/i.test(s.name||'');
}
function liveTemperature(s,time=Date.now()/1000) {return s.kind==='Temperature'&&!temperatureReference(s)&&freshSensor(s,time);}
function sensorDevice(s) {
  const parent=String(s.parent||''),type=String(s.hardware_type||''),identity=parent||type||String(s.source||'unknown');
  let category='other',label='Остальные датчики';
  if(/^ACPI/i.test(parent)||/^windows\/acpi\//i.test(s.identifier||'')){category='acpi';label='ACPI · Термозоны';}
  else if(type==='Cpu'||/intelcpu|amdcpu|(^|\/)cpu(?:\/|$)/i.test(parent)){category='cpu';label='CPU';}
  else if(/^Gpu/i.test(type)||/(^|\/)gpu(?:-|\/)/i.test(parent)){category='gpu';label=/nvidia/i.test(type+' '+parent)?'GPU NVIDIA':/amd|ati/i.test(type+' '+parent)?'GPU AMD':/intel/i.test(type+' '+parent)?'GPU Intel':'GPU';}
  else if(/storage|nvme|hdd|ssd/i.test(type+' '+parent)||/^windows\/storage\//i.test(s.identifier||'')){category='storage';label=/^Storage /i.test(parent)&&s.name?'Накопитель · '+s.name:'Накопитель';}
  else if(type==='Memory'||/(^|\/)(?:ram|vram|memory)(?:\/|$)/i.test(parent)){
    category='memory';
    const dimm=parent.match(/\/dimm\/(\d+)/i);
    label=dimm?'Память DIMM #'+dimm[1]:/vram/i.test(parent)?'Видеопамять':'Оперативная память';
  } else if(/motherboard|superio|embeddedcontroller|lpc/i.test(type+' '+parent)){category='board';label='Материнская плата';}
  return {key:category+'|'+identity.toLowerCase(),category,label};
}
const sensorKindLabels={Temperature:'Температуры',Clock:'Частоты',Load:'Загрузка',Fan:'Вентиляторы',Power:'Мощность',Voltage:'Напряжения',Throttling:'Троттлинг',Reference:'Параметры датчиков'};
function groupSensors(sensors,time=Date.now()/1000) {
  const devices=new Map(),categoryOrder=['cpu','gpu','storage','memory','board','acpi','other'],kindOrder=Object.keys(sensorKindLabels);
  for(const sensor of sensors){
    const device=sensorDevice(sensor);
    if(!devices.has(device.key))devices.set(device.key,{...device,sensors:[],types:[],temperature:null});
    devices.get(device.key).sensors.push(sensor);
  }
  const result=[...devices.values()].sort((a,b)=>categoryOrder.indexOf(a.category)-categoryOrder.indexOf(b.category)||a.key.localeCompare(b.key,undefined,{numeric:true}));
  const labels=new Map();
  for(const group of result){
    const sameLabel=result.filter(g=>g.label===group.label).length;
    if(sameLabel>1){const index=(labels.get(group.label)||0)+1;labels.set(group.label,index);group.displayLabel=group.label+' #'+index;}else group.displayLabel=group.label;
    const types=new Map();
    for(const sensor of group.sensors){
      const kind=temperatureReference(sensor)?'Reference':sensor.kind||'Other';
      if(!types.has(kind))types.set(kind,[]);
      types.get(kind).push(sensor);
      if(liveTemperature(sensor,time))group.temperature=group.temperature===null?sensor.value:Math.max(group.temperature,sensor.value);
    }
    group.types=[...types].map(([kind,readings])=>({kind,key:group.key+'|'+kind,label:sensorKindLabels[kind]||kind,sensors:[...readings].sort((a,b)=>String(a.name).localeCompare(String(b.name),undefined,{numeric:true,sensitivity:'base'})||String(a.identifier).localeCompare(String(b.identifier)))})).sort((a,b)=>(kindOrder.indexOf(a.kind)<0?99:kindOrder.indexOf(a.kind))-(kindOrder.indexOf(b.kind)<0?99:kindOrder.indexOf(b.kind)));
  }
  return result;
}
function rememberSensorDisclosure(previous,entries) {const next=new Map(previous);for(const [key,open] of entries)next.set(key,!!open);return next;}
function sensorDisclosureOpen(state,key,fallback=false) {return state.has(key)?state.get(key):fallback;}
let sensorDisclosureState=new Map();
function sensorPanelSignature(list,telemetry,settings,locale,time=Date.now()/1000) {
  return JSON.stringify([locale,settings?.extended_sensors,telemetry?.errors,telemetry?.error,telemetry?.extended?.errors,telemetry?.extended?.error,telemetry?.extended?.diagnostics,list.map(s=>[s.identifier,s.name,s.parent,s.hardware_type,s.kind,s.value,s.sample_time,s.source,freshSensor(s,time),s.history?.length,s.history?.[0]?.[0],s.history?.[s.history.length-1]?.[0]])]);
}
function sensorReading(s) {
  const units={Temperature:'°C',Fan:'RPM',Power:'W',Voltage:'V',Clock:'MHz',Load:'%'};
  const value=Number.isFinite(s.value)?s.kind==='Throttling'?(s.value>0?'Да':'Нет'):s.value.toFixed(s.kind==='Voltage'?2:1):'Недоступно';
  return `<div class="sensor"><div>${esc(s.name)}<small>${esc(s.parent)} · ${esc(s.kind)}</small><small>${esc(s.source)} · ${Number.isFinite(s.sample_time)?new Date(s.sample_time*1000).toLocaleTimeString(language):'—'}${!freshSensor(s)?' · Данные устарели':''}</small>${sparkline(s)}</div><strong>${value} <small style="display:inline">${Number.isFinite(s.value)?units[s.kind]||'':''}</small></strong></div>`;
}
function groupedSensorMarkup(list) {
  return groupSensors(list).map(group=>`<details class="sensor-device" data-sensor-key="${esc(group.key)}" ${sensorDisclosureOpen(sensorDisclosureState,group.key)?'open':''}><summary><span class="sensor-device-title">${esc(group.displayLabel)}</span><span class="sensor-device-count">${group.sensors.length}</span>${group.temperature===null?'':`<span class="sensor-device-temperature">MAX ${group.temperature.toFixed(1)} °C</span>`}</summary><div class="sensor-types">${group.types.map(type=>`<details class="sensor-type" data-sensor-key="${esc(type.key)}" ${sensorDisclosureOpen(sensorDisclosureState,type.key,type.kind==='Temperature')?'open':''}><summary>${esc(type.label)} <span class="muted">· ${type.sensors.length}</span></summary>${type.sensors.map(sensorReading).join('')}</details>`).join('')}</div></details>`).join('');
}
function cpuSensor(s) {return s.hardware_type==='Cpu'||/(^|\/)cpu(?:\/|$)|intelcpu|amdcpu/i.test(s.parent||'');}
function thermalSummary(list,time=Date.now()/1000) {
  const cpu=list.filter(s=>cpuSensor(s)&&freshSensor(s,time));
  // Distance-to-TjMax is a margin, not a temperature reading to maximize.
  const temperatures=cpu.filter(s=>!temperatureReference(s)&&s.kind==='Temperature');
  const packageSensor=temperatures.find(s=>/package|tctl|tdie/i.test(s.name));
  const throttle=cpu.filter(s=>s.kind==='Throttling');
  return {cpu,packageTemperature:packageSensor?.value??null,maximum:temperatures.length?Math.max(...temperatures.map(s=>s.value)):null,throttled:throttle.length?throttle.some(s=>s.value>0):null};
}
function cpuTemperatureNotice() {
  if(!data.settings?.extended_sensors)return '<p class="notice">Для температур CPU включите расширенные датчики в настройках.</p>';
  const status=data.telemetry?.extended?.diagnostics?.cpu_temperature_status;
  if(status==='driver_missing')return '<p class="notice">Для температур CPU нужен драйвер PawnIO. Он не установлен. После установки запустите CoreScope от администратора. <a href="https://pawnio.eu/" target="_blank" rel="noopener noreferrer">Официальный сайт PawnIO</a></p>';
  if(status==='access_required')return '<p class="notice">PawnIO установлен. Для чтения температур CPU запустите CoreScope от администратора.</p>';
  if(status==='unavailable')return '<p class="notice">PawnIO установлен и права администратора доступны, но температур CPU пока нет. Подробности — в диагностике датчиков.</p>';
  return '<p class="notice">Температура CPU недоступна. Подробности — в диагностике датчиков.</p>';
}
function tinyHistory(values) {
  if(values.length<2)return '';
  return `<svg viewBox="0 0 100 22" width="100%" height="22" aria-hidden="true"><polyline points="${values.map((v,i)=>`${i*100/(values.length-1)},${21-Math.min(100,Math.max(0,v))*0.2}`).join(' ')}" fill="none" stroke="var(--lime)" stroke-width="1.5"/></svg>`;
}
function cpuExcluded(config,id) {return !!config?.cpu_guard?.enabled && !!config.cpu_guard.excluded_cores?.includes(id);}
function coreButton(c,config,selectedCore) {
  const protectedCpu=cpuExcluded(config,c.id);
  const title=protectedCpu?'Исключается для тяжёлых процессов':'Доступен для тяжёлых процессов';
  return `<button class="core ${selectedCore===c.id?'chosen':''} ${protectedCpu?'protected':''}" data-core="${c.id}" title="CPU ${c.id} · ${title}"><small>CPU ${c.id}${protectedCpu?' · ◉':''}</small><strong>${Math.round(c.usage)}<small>%</small></strong><div class="meter"><i style="width:${c.usage}%"></i></div>${tinyHistory(cpuHistory.get(c.id)||[])}<small>${c.mhz?Math.round(c.mhz)+' MHz':'— MHz'}</small></button>`;
}
function applicationGroups(processes) {
  const byPid=new Map(processes.map(p=>[p.pid,p])),groups=new Map();
  const sharedHelper=p=>/^(msedgewebview2|qtwebengineprocess|crashpad_handler)\.exe$/i.test(p.name||'');
  for(const p of processes){
    let target=p.application_rule||p.path||null;
    if(!p.application_rule && sharedHelper(p)){
      const seen=new Set([p.pid]);let child=p,parent=byPid.get(p.parent),found=false;
      while(parent&&!seen.has(parent.pid)){
        if(/^\d+$/.test(parent.identity||'')&&/^\d+$/.test(child.identity||'')&&BigInt(parent.identity)>BigInt(child.identity))break;
        seen.add(parent.pid);
        if(parent.application_rule||(!sharedHelper(parent)&&parent.path)){target=parent.application_rule||parent.path;found=true;break;}
        child=parent;
        parent=byPid.get(parent.parent);
      }
      // Shared helpers without a known owner must not be merged across applications.
      if(!found)target=null;
    }
    const key=target?target.toLowerCase():'pid:'+p.pid+':'+p.identity;
    if(!groups.has(key))groups.set(key,{key,target,name:target?target.split(/[\\/]/).pop():p.name,members:[],anchor:null});
    groups.get(key).members.push(p);
  }
  for(const group of groups.values()){
    const roots=group.members.filter(p=>p.path&&group.target&&p.path.toLowerCase()===group.target.toLowerCase());
    group.anchor=roots.find(p=>!roots.some(parent=>parent.pid===p.parent))||roots[0]||group.members[0];
  }
  return [...groups.values()];
}
function processPermissionRow(p,core) {
  const on=p.affinity?.includes(core.id),last=on&&p.affinity.length===1,canChange=p.identity&&p.allowed?.includes(core.id)&&!last;
  return `<div class="coreprocess"><button class="processlink" data-pid="${p.pid}" data-identity="${esc(p.identity||'')}"><strong>${esc(p.name)}</strong><small>PID ${p.pid} · ${pct(p.cpu)}% · ${bytes(p.memory)}</small></button><button class="coretoggle ${on?'deny':'allow'}" data-toggle-pid="${p.pid}" data-identity="${esc(p.identity||'')}" data-toggle-core="${core.id}" data-enable="${!on}" ${canChange?'':'disabled'}>${last?'Единственное':on?'Запретить':'Разрешить'}</button></div>`;
}
function renderApplicationGroups(core) {
  const focused=$('coreProcesses').contains(document.activeElement)?document.activeElement:null;
  if(focused?.matches(':active'))return;
  const focusKey=focused?.dataset.appPid?`[data-app-pid="${focused.dataset.appPid}"]`:focused?.dataset.togglePid?`[data-toggle-pid="${focused.dataset.togglePid}"]`:focused?.dataset.pid?`[data-pid="${focused.dataset.pid}"]`:null;
  const focusedGroup=focused?.tagName==='SUMMARY'?focused.parentElement?.dataset.appKey:null;
  const known=data.processes.filter(p=>p.allowed?.includes(core.id)),q=$('coreSearch').value.toLowerCase();
  const groups=applicationGroups(known).filter(g=>g.members.some(p=>(coreMode==='allowed')===p.affinity?.includes(core.id)));
  const visible=groups.filter(g=>matchesProcess({name:g.name,pid:g.anchor.pid},q)||g.members.some(p=>matchesProcess(p,q)));
  visible.sort((a,b)=>a.name.localeCompare(b.name,undefined,{sensitivity:'base',numeric:true})||a.anchor.pid-b.anchor.pid);
  const expanded=new Set([...$('coreProcesses').querySelectorAll('details[data-app-key][open]')].map(d=>d.dataset.appKey));
  $('coreMatches').textContent=visible.length+' / '+groups.length+' приложений';
  $('coreProcesses').innerHTML=visible.map(g=>{
    const p=g.anchor,allow=coreMode==='denied',allowed=g.members.filter(p=>p.affinity?.includes(core.id)).length;
    return `<div class="application-group"><div class="application-heading"><button class="processlink" data-pid="${p.pid}" data-identity="${esc(p.identity||'')}" title="${esc(g.target||'')}"><strong>${esc(g.name)}</strong><small>${g.members.length} процессов · Разрешено: ${allowed}</small></button><button class="coretoggle ${allow?'allow':'deny'}" data-app-pid="${p.pid}" data-identity="${esc(p.identity||'')}" data-app-core="${core.id}" data-app-enable="${allow}" ${p.identity&&g.target?'':'disabled'}>${allow?'Разрешить приложению':'Запретить приложению'}</button></div><details data-app-key="${esc(g.key)}" ${expanded.has(g.key)?'open':''}><summary>Отдельные процессы</summary>${g.members.map(p=>processPermissionRow(p,core)).join('')}</details></div>`;
  }).join('')||'<div class="empty">Процессы не найдены</div>';
  if(focusKey)$('coreProcesses').querySelector(focusKey)?.focus({preventScroll:true});
  if(focusedGroup)[...$('coreProcesses').querySelectorAll('details[data-app-key]')].find(d=>d.dataset.appKey===focusedGroup)?.querySelector('summary')?.focus({preventScroll:true});
}
document.addEventListener('click',async e=>{
  const button=e.target.closest('[data-app-pid]');if(!button||coreBusy||button.disabled)return;
  const pid=Number(button.dataset.appPid),identity=button.dataset.identity,core=Number(button.dataset.appCore),enabled=button.dataset.appEnable==='true';
  coreBusy=true;button.disabled=true;button.textContent='Применяем…';
  try {
    const r=await api(`/api/process/${pid}/application/core`,{identity,core,enabled});
    $('applicationResult').hidden=false;
    $('applicationResult').innerHTML=`${enabled?'Разрешено приложению':'Запрещено приложению'} · ${esc(r.rule_target.split(/[\\/]/).pop())} · CPU ${core}<br>Изменено процессов: ${r.changed} / ${r.matched}${r.failed.length?'<br>Недоступно: '+r.failed.length+'<br>'+r.failed.slice(0,5).map(f=>esc(f.name)+' · PID '+f.pid+' · '+esc(f.error)).join('<br>'):''}${r.guard_may_reapply?'<br>Общая автозащита CPU включена и может снова ограничить приложение при нагрузке.':''}`;
    data=await api('/api/snapshot');renderAutomationStatus();map();
  }catch(error){toast(error.message);}finally{coreBusy=false;renderCore();table();}
});
function cpu22Config(config,enabled) {
  const next=structuredClone(config),guard=next.cpu_guard;
  const others=guard.excluded_cores.filter(c=>c!==22);
  if(enabled){guard.enabled=true;guard.excluded_cores=[...new Set([...others,22])].sort((a,b)=>a-b);}
  else if(others.length){guard.excluded_cores=others;}
  else {guard.enabled=false;guard.excluded_cores=[22];}
  return next;
}
let guard22Busy=false;
function renderCpuProtection() {
  const exists=data.cpus.some(c=>c.id===22),enabled=cpuExcluded(data.automation?.config,22);
  $('guard22Toggle').checked=enabled;
  $('guard22Toggle').disabled=guard22Busy||!exists||!data.automation||!data.affinity_supported;
  $('guard22State').textContent=!exists?'CPU 22 недоступен':enabled?'Защита CPU 22 включена':'Защита CPU 22 выключена';
  $('watchCpu22').disabled=!exists;
}
$('guard22Toggle').onchange=async()=>{
  const enabled=$('guard22Toggle').checked;
  if(guard22Busy)return;
  guard22Busy=true;$('guard22Toggle').disabled=true;
  try {
    const latest=await api('/api/snapshot');
    const saved=await api('/api/automation',{config:cpu22Config(latest.automation.config,enabled),revision:latest.automation.revision});
    data.automation={...data.automation,...saved};
    map();renderAutomationStatus();toast(enabled?'Защита CPU 22 включена':'Защита CPU 22 выключена');
  }catch(e){toast(e.message);}finally{guard22Busy=false;renderCpuProtection();}
};
function renderDiagnostics() {
  if(!data?.ready)return;
  const summary=thermalSummary(data.telemetry?.sensors||[]);
  renderLatency();
  const temp=v=>v===null?'Недоступно':v.toFixed(1)+' °C';
  $('cpuSummary').innerHTML=`<div class="hint">Общая загрузка<strong>${pct(data.cpu_usage)}%</strong></div><div class="hint">CPU Package<strong>${temp(summary.packageTemperature)}</strong></div><div class="hint">Максимум CPU<strong>${temp(summary.maximum)}</strong></div><div class="hint">Троттлинг<strong>${summary.throttled===null?'Нет данных':summary.throttled?'Да':'Нет'}</strong></div>`;
  $('cpuTemperatureStatus').innerHTML=summary.maximum===null?cpuTemperatureNotice():'';
  if(data.time!==diagnosticTime){
    for(const c of data.cpus){const h=cpuHistory.get(c.id)||[];h.push(c.usage);if(h.length>60)h.shift();cpuHistory.set(c.id,h);}
    diagnosticTime=data.time;
  }
  renderCpuProtection();
  const rows=(data.telemetry?.sensors||[]).filter(cpuSensor).filter(s=>['Temperature','Clock','Load','Power','Throttling'].includes(s.kind));
  const units={Temperature:'°C',Clock:'MHz',Load:'%',Power:'W'};
  $('thermalDetails').innerHTML=rows.length?`<table class="sensor-table"><thead><tr><th>Датчик</th><th>Сейчас</th><th>Min / Max</th><th>Источник</th></tr></thead><tbody>${rows.map(s=>{
    const valid=freshSensor(s),h=(s.history||[]).map(p=>p[1]).filter(Number.isFinite),unit=units[s.kind]||'';
    return `<tr><td>${esc(s.name)}<br><small class="muted">${esc(s.kind)}</small></td><td>${valid?s.kind==='Throttling'?(s.value>0?'Да':'Нет'):s.value.toFixed(1)+' '+unit:'Данные устарели'}</td><td>${h.length?Math.min(...h).toFixed(1)+' / '+Math.max(...h).toFixed(1)+' '+unit:'—'}${s.kind==='Temperature'?sparkline(s):''}</td><td>${esc(s.source)}<br><small>${new Date(s.sample_time*1000).toLocaleTimeString(language)}</small></td></tr>`;
  }).join('')}</tbody></table>`:'<p class="notice">Датчики CPU недоступны. Проверьте расширенные датчики и диагностику источника.</p>';
}
$('watchCpu22').onclick=()=>{filterCore=22;coreMode='activity';activityOrder=[];$('coreSearch').value='';map();renderCore();$('corePanel').scrollIntoView({behavior:'smooth',block:'start'});};
function renderLatency() {
  const l=data.latency||{},h=l.history||[],last=h[h.length-1];
  const valid=last && Date.now()/1000-last.time<5;
  const irq=data.activity?.status==='running'?data.activity.interrupts:null;
  const dpc=irq?.length?Math.max(...irq.map(c=>c.dpc_max_us)):null,isr=irq?.length?Math.max(...irq.map(c=>c.isr_max_us)):null;
  const us=v=>v===null?'Недоступно':Number(v).toFixed(0)+' µs';
  $('latencySummary').innerHTML=`<div class="hint">Пробуждение · максимум за замер<strong>${us(valid?last.max_us:null)}</strong></div><div class="hint">Пробуждение · пик с запуска<strong>${us(h.length?l.peak_us:null)}</strong></div><div class="hint">DPC · максимум за замер<strong>${us(dpc)}</strong></div><div class="hint">ISR · максимум за замер<strong>${us(isr)}</strong></div>`;
  const recent=h.slice(-120),max=Math.max(1,...recent.map(p=>p.max_us));
  $('latencyHistory').innerHTML=tinyHistory(recent.map(p=>p.max_us/max*100))+`<p class="hint">Последние 120 замеров · максимум шкалы ${us(max)}${l.error?' · '+esc(l.error):''}${data.activity?.lost?' · Потеряно событий ETW: '+data.activity.lost:''}</p>`;
}
let historyCore=null,historyData=null,historyRequest=0;
$('coreLogToggle').onclick=async()=>{
  if(filterCore===null)return;
  $('coreLogToggle').disabled=true;
  try{data.settings=await api('/api/activity/logging',{core:filterCore,enabled:!data.settings.activity_log_cores?.includes(filterCore)});renderCore();}
  catch(e){toast(e.message);}finally{$('coreLogToggle').disabled=false;}
};
function logRows(rows,aggregate=false) {
  return `<table class="sensor-table"><thead><tr><th>Процесс</th><th>PID</th><th>Время CPU</th>${aggregate?'<th>Последний раз</th>':''}</tr></thead><tbody>${rows.map(r=>`<tr><td>${esc(r.name||'—')}</td><td>${r.pid}</td><td>${Number(r.ms).toFixed(1)} ms</td>${aggregate?`<td>${new Date(r.last*1000).toLocaleTimeString(language)}</td>`:''}</tr>`).join('')}</tbody></table>`;
}
async function loadHistory() {
  const request=++historyRequest;$('historyStatus').textContent='Загрузка…';$('historyExport').disabled=true;
  try{
    const result=await api(`/api/activity/history?core=${historyCore}&minutes=${$('historyMinutes').value}`);
    if(request!==historyRequest)return;historyData=result;
    $('historyTitle').textContent='Журнал CPU '+result.core;
    $('historyRows').innerHTML=result.rows.length?logRows(result.rows,true):`<p class="hint">${result.samples.length?'Процессы не зафиксированы. Проверьте статус ETW в замере.':'Нет записей. Включите запись на выбранном CPU.'}</p>`;
    const valid=result.samples.filter(s=>s.status==='running').length;
    $('historyStatus').textContent=`${result.samples.length} замеров · ETW: ${valid} · ${result.damaged_lines} повреждённых строк`+(data.activity_log?.error?' · '+data.activity_log.error:'');
    $('historyPosition').max=Math.max(0,result.samples.length-1);$('historyPosition').value=$('historyPosition').max;
    $('historyPosition').disabled=!result.samples.length;$('historyExport').disabled=false;renderHistorySample();
  }catch(e){if(request===historyRequest){historyData=null;$('historyStatus').textContent=e.message;$('historyRows').innerHTML='';$('historySample').innerHTML='';}}
}
function renderHistorySample(){
  const s=historyData?.samples[Number($('historyPosition').value)];
  const statuses={running:'ETW работает',permission:'Нет доступа к ETW',starting:'Ожидание событий ETW…',stale:'События ETW задерживаются',stopped:'Сбор ETW остановлен',unsupported:'Активность для нескольких групп CPU пока недоступна',error:'Не удалось запустить ETW'};
  $('historyAt').textContent=s?`${new Date(s.time*1000).toLocaleTimeString(language)} · ${statuses[s.status]||'ETW недоступен'} · CPU ${pct(s.cpu_usage)}% · DPC ${s.interrupts?.dpc_max_us?.toFixed(0)??'—'} µs · ISR ${s.interrupts?.isr_max_us?.toFixed(0)??'—'} µs${s.cpu_temperature_max!=null?' · CPU max '+s.cpu_temperature_max.toFixed(1)+' °C':''}${s.truncated?' · частичный список':''}`:'Нет записей';
  $('historySample').innerHTML=s?logRows(s.rows||[]):'';
}
$('coreHistoryOpen').onclick=()=>{if(filterCore===null)return;historyCore=filterCore;$('historyDialog').showModal();loadHistory();};
$('historyMinutes').onchange=loadHistory;$('historyRefresh').onclick=loadHistory;$('historyPosition').oninput=renderHistorySample;
$('historyClose').onclick=()=>{$('historyDialog').close();historyRequest++;};
$('historyExport').onclick=()=>{
  if(!historyData)return;
  const url=URL.createObjectURL(new Blob([JSON.stringify(historyData,null,2)],{type:'application/json'}));
  const a=document.createElement('a');a.href=url;a.download=`corescope-cpu-${historyData.core}-${historyData.to}.json`;document.body.append(a);a.click();a.remove();setTimeout(()=>URL.revokeObjectURL(url),10000);
};
