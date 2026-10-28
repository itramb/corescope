const priorityNames={idle:'Низкий',below_normal:'Ниже обычного',normal:'Обычный',above_normal:'Выше обычного',high:'Высокий',realtime:'Реального времени'};
let automationDraft=null,automationRevision=0,editingRule=-1;
function priorityOptions(value,optional=false) {
  return (optional?'<option value="">Не менять</option>':'')+Object.entries(priorityNames).map(([key,label])=>`<option value="${key}" ${key===value?'selected':''}>${label}</option>`).join('');
}
function parseCores(value,optional=false,checkAvailable=true) {
  if(!value.trim() && optional)return null;
  const pieces=value.trim().split(/[,\s]+/);
  if(!value.trim() || pieces.some(c=>!/^\d+$/.test(c)))throw Error('Укажите номера CPU через запятую');
  const cores=[...new Set(pieces.map(Number))].sort((a,b)=>a-b);
  if(cores.some(c=>c>=64||(checkAvailable&&!data.cpus.some(cpu=>cpu.id===c))))throw Error('Недопустимый номер CPU');
  return cores;
}
function without22(allowed) {
  if(!allowed.includes(22))throw Error('CPU 22 недоступен');
  const cores=allowed.filter(c=>c!==22);
  if(!cores.length)throw Error('Нельзя запретить последнее разрешённое ядро');
  return cores;
}
function processControls(p) {
  return `<div class="priority-controls"><div class="sectionlabel">Приоритет и правила</div><p class="hint" id="livePriority">${priorityNames[p.priority]||'Нет доступа'}</p><div class="actions"><select id="processPriority" aria-label="Приоритет">${priorityOptions(p.priority||'normal')}</select><button id="priorityApply" ${!p.identity?'disabled':''}>Применить приоритет</button></div><div class="actions"><button id="processAvoid22" ${!p.allowed?.includes(22)?'disabled':''}>Все, кроме CPU 22</button><button id="processRule" ${!p.identity?'disabled':''}>Сохранить правило</button></div><p class="hint">Кнопка CPU 22 меняет выбор. Нажмите «Применить» или сохраните правило.</p></div>`;
}
function renderAutomationStatus() {
  const a=data?.automation;if(!a)return;
  const c=a.config;
  $('automationStatus').textContent=`Защита CPU: ${c.cpu_guard.enabled?'включена':'выключена'} · Ограничено процессов: ${a.guarded} · Автобалансировка: ${c.balance.enabled?'включена':'выключена'} · Снижен приоритет: ${a.restrained} · Правил: ${c.rules.length}`;
  if(a.error)$('automationStatus').textContent+=' · '+a.error;
  const p=data.processes.find(p=>p.pid===selected && p.identity===selectedIdentity);
  if(p && $('livePriority'))$('livePriority').textContent=priorityNames[p.priority]||'Нет доступа';
  if($('automationDialog').open)renderEvents();
}
function renderEvents() {
  const labels={restrained:'Снижен приоритет',restored:'Восстановлен приоритет',cpu_guard:'Исключены CPU',affinity_restored:'Восстановлена привязка',rule_priority:'Приоритет по правилу',rule_affinity:'Привязка по правилу',error:'Ошибка'};
  $('automationEvents').innerHTML=(data.automation.events||[]).map(e=>`<p>${new Date(e.time*1000).toLocaleTimeString(language)} · ${esc(e.name)} (${e.pid}) · ${labels[e.action]||esc(e.action)}${e.detail?' · '+esc(e.detail):''}</p>`).join('')||'<p class="hint">Действий пока нет</p>';
}
function openAutomation(p=null) {
  if(!data?.automation)return;
  automationDraft=structuredClone(data.automation.config);automationRevision=data.automation.revision;
  const c=automationDraft;
  $('guardEnabled').checked=c.cpu_guard.enabled;$('guardCores').value=c.cpu_guard.excluded_cores.join(', ');
  $('guardThreshold').value=c.cpu_guard.process_threshold;$('guardSeconds').value=c.cpu_guard.sustain_seconds;
  $('balanceEnabled').checked=c.balance.enabled;$('balanceSystem').value=c.balance.system_threshold;
  $('balanceProcess').value=c.balance.process_threshold;$('balanceRecover').value=c.balance.recover_threshold;
  $('balanceSeconds').value=c.balance.sustain_seconds;$('balanceRecoverySeconds').value=c.balance.recover_seconds;
  $('governorWarning').hidden=!data.processes.some(p=>/^(processlasso|processgovernor)\.exe$/i.test(p.name));
  clearRule();renderRules();renderEvents();
  if(p){$('ruleTarget').value=p.path||p.name;$('ruleCores').value=[...draft].sort((a,b)=>a-b).join(', ');}
  $('automationDialog').showModal();
}
function clearRule() {
  editingRule=-1;$('ruleTarget').value='';$('rulePriority').innerHTML=priorityOptions('',true);$('ruleCores').value='';$('ruleExcludedCores').value='';$('ruleDescendants').checked=false;
  $('ruleExcludeBalance').checked=false;$('ruleExcludeGuard').checked=false;$('ruleAdd').textContent='Добавить правило';
}
function renderRules() {
  $('ruleList').innerHTML=automationDraft.rules.map((r,i)=>`<div class="rule-row"><span>${esc(r.target)}<br><small class="muted">${r.priority?priorityNames[r.priority]+' · ':''}${r.cores?'CPU '+r.cores.join(', ')+' · ':''}${r.excluded_cores?.length?'Не использовать CPU '+r.excluded_cores.join(', ')+' · ':''}${r.include_descendants?'Включая дочерние процессы · ':''}${r.exclude_balance?'Без автобалансировки · ':''}${r.exclude_guard?'Без защиты CPU':''}</small></span><button data-rule-edit="${i}">Изменить</button><button data-rule-delete="${i}">Удалить</button></div>`).join('')||'<p class="hint">Правил пока нет</p>';
}
$('automationOpen').onclick=()=>openAutomation();
$('automationCancel').onclick=()=>$('automationDialog').close();
$('ruleClear').onclick=clearRule;
$('ruleAvoid22').onclick=()=>{try{$('ruleCores').value=without22(data.cpus.map(c=>c.id)).join(', ');}catch(e){toast(e.message);}};
$('ruleAdd').onclick=()=>{
  try {
    const r={...(editingRule>=0?automationDraft.rules[editingRule]:{}),target:$('ruleTarget').value.trim(),priority:$('rulePriority').value||null,cores:parseCores($('ruleCores').value,true),excluded_cores:parseCores($('ruleExcludedCores').value,true)||[],include_descendants:$('ruleDescendants').checked,exclude_balance:$('ruleExcludeBalance').checked,exclude_guard:$('ruleExcludeGuard').checked};
    if(!r.target || (!r.priority && !r.cores && !r.excluded_cores.length && !r.exclude_balance && !r.exclude_guard))throw Error('Укажите программу и действие правила');
    const duplicate=automationDraft.rules.findIndex(x=>x.target.toLowerCase()===r.target.toLowerCase());
    if(duplicate!==-1 && duplicate!==editingRule)throw Error('Правило для этой программы уже есть');
    if(editingRule<0)automationDraft.rules.push(r);else automationDraft.rules[editingRule]=r;
    clearRule();renderRules();
  }catch(e){toast(e.message);}
};
$('ruleList').onclick=e=>{
  const edit=e.target.closest('[data-rule-edit]'),del=e.target.closest('[data-rule-delete]');
  if(del){automationDraft.rules.splice(+del.dataset.ruleDelete,1);clearRule();renderRules();}
  if(edit){editingRule=+edit.dataset.ruleEdit;const r=automationDraft.rules[editingRule];$('ruleTarget').value=r.target;$('rulePriority').innerHTML=priorityOptions(r.priority,true);$('ruleCores').value=r.cores?.join(', ')||'';$('ruleExcludedCores').value=r.excluded_cores?.join(', ')||'';$('ruleDescendants').checked=!!r.include_descendants;$('ruleExcludeBalance').checked=r.exclude_balance;$('ruleExcludeGuard').checked=r.exclude_guard;$('ruleAdd').textContent='Обновить правило';}
};
$('automationSave').onclick=async()=>{
  $('automationSave').disabled=true;
  try {
    if($('ruleTarget').value.trim())throw Error('Добавьте или обновите редактируемое правило перед сохранением');
    automationDraft.cpu_guard={enabled:$('guardEnabled').checked,excluded_cores:parseCores($('guardCores').value,false,$('guardEnabled').checked),process_threshold:Number($('guardThreshold').value),sustain_seconds:Number($('guardSeconds').value)};
    automationDraft.balance={enabled:$('balanceEnabled').checked,system_threshold:Number($('balanceSystem').value),process_threshold:Number($('balanceProcess').value),recover_threshold:Number($('balanceRecover').value),sustain_seconds:Number($('balanceSeconds').value),recover_seconds:Number($('balanceRecoverySeconds').value)};
    const saved=await api('/api/automation',{config:automationDraft,revision:automationRevision});
    data.automation={...data.automation,...saved};$('automationDialog').close();renderAutomationStatus();toast('Настройки сохранены');
  }catch(e){toast(e.message);}finally{$('automationSave').disabled=false;}
};
$('launchAvoid22').onclick=()=>{
  try{const cores=without22(data.cpus.map(c=>c.id));for(const b of $('launchCores').querySelectorAll('[data-launch]')){b.classList.toggle('active',cores.includes(+b.dataset.launch));b.setAttribute('aria-pressed',cores.includes(+b.dataset.launch));}}
  catch(e){toast(e.message);}
};
document.addEventListener('click',async e=>{
  if(!['priorityApply','processAvoid22','processRule'].includes(e.target.id))return;
  const p=data?.processes.find(p=>p.pid===selected && p.identity===selectedIdentity);if(!p)return;
  try {
    if(e.target.id==='processRule'){openAutomation(p);return;}
    if(e.target.id==='processAvoid22'){draft=new Set(without22(p.allowed||[]));inspect();map();return;}
    const priority=$('processPriority').value;e.target.disabled=true;
    const r=await api(`/api/process/${p.pid}/priority`,{identity:p.identity,priority});p.priority=r.priority;
    toast('Приоритет применён');
  }catch(error){toast(error.message);}finally{e.target.disabled=false;renderAutomationStatus();}
});
