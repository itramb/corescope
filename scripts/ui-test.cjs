const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const path=require('node:path');
const html=fs.readFileSync(path.join(__dirname,'../assets/index.html'),'utf8');
const code=html.slice(html.indexOf('      function matchesProcess('),html.indexOf('      function renderCore('));
function harness() {
  const elements={coreSearch:{value:''},groupApplications:{checked:false,parentElement:{}},applicationRuleHint:{},activityThreshold:{value:'0'},activityWindow:{value:'1'},coreLogToggle:{},coreMatches:{},coreHint:{},coreActivityNotes:{},coreProcesses:{innerHTML:'',matches:()=>false,contains:()=>false,querySelector:()=>null}};
  const context={document:{activeElement:null},data:{activity:{status:'running',lag_ms:100},processes:[]},$:(id)=>elements[id],pct:(v)=>v.toFixed(1),esc:(v)=>String(v)};
  vm.createContext(context);vm.runInContext(code,context);
  return {context,elements};
}
test('search combines with inclusive CPU/RAM thresholds and handles inaccessible affinity',()=>{
  const {context:c}=harness();
  const p={name:'Worker.EXE',pid:321,cpu:1,memory:100*1024*1024,affinity:[0]};
  assert.equal(c.matchesProcess(p,' worker ','busy',2),true);
  assert.equal(c.matchesProcess(p,'321','memory',2),true);
  assert.equal(c.matchesProcess(p,'missing','busy',2),false);
  assert.equal(c.matchesProcess({...p,cpu:.99},'','busy',2),false);
  assert.equal(c.matchesProcess({...p,memory:p.memory-1},'','memory',2),false);
  assert.equal(c.matchesProcess(p,'','restricted',2),true);
  for(const affinity of [null,[],[0,1]])assert.equal(c.matchesProcess({...p,affinity},'','restricted',2),false);
});

function diagnosticsHarness(){
  const elements={};const c={$:id=>elements[id]??(elements[id]={}),document:{addEventListener:()=>{}},Map,Date,Number,Set,structuredClone};
  vm.createContext(c);vm.runInContext(fs.readFileSync(path.join(__dirname,'../assets/diagnostics.js'),'utf8'),c);return c;
}
test('process activity reports observed CPUs, deduplicates and distinguishes unavailable ETW',()=>{
  const c=diagnosticsHarness();
  const a={status:'running',cores:[{id:22,rows:[{pid:4,ms:20,usage:2}]},{id:2,rows:[{pid:4,ms:30,usage:3},{pid:5,ms:0,usage:0}]}]};
  assert.deepEqual([...c.processActivity(a).get(4)],[2,22]);assert.equal(c.processActivity(a).has(5),false);
  assert.equal(c.processActivity({status:'permission'}),null);
});
test('sorting uses numeric affinity masks, name ordering, stable PID ties and inaccessible-last',()=>{
  const c=diagnosticsHarness(),p=(pid,name,affinity)=>({pid,name,affinity});
  assert.ok(c.compareProcesses(p(1,'a',[0,2]),p(2,'b',[0,10]),'affinity',true)<0);
  assert.ok(c.compareProcesses(p(1,'a',[0,2]),p(2,'b',[0,10]),'affinity',false)>0);
  assert.ok(c.compareProcesses(p(1,'a',null),p(2,'b',[0]),'affinity',false)>0);
  assert.ok(c.compareProcesses(p(2,'App2',[0]),p(1,'app10',[0]),'name',true)<0);
  assert.ok(c.compareProcesses(p(1,'same',[0]),p(2,'SAME',[0]),'name',false)<0);
});
test('thermal summary never infers throttling or maps ACPI to CPU and rejects stale readings',()=>{
  const c=diagnosticsHarness(),sensor={hardware_type:'Cpu',kind:'Temperature',name:'CPU Package',value:98,sample_time:100};
  let r=c.thermalSummary([sensor,{...sensor,name:'Distance to TjMax',value:110}],105);
  assert.equal(r.packageTemperature,98);assert.equal(r.maximum,98);assert.equal(r.throttled,null);
  assert.equal(c.thermalSummary([{...sensor,sample_time:70}],105).packageTemperature,null);
  assert.equal(c.thermalSummary([{...sensor,hardware_type:null,parent:'ACPI thermal zone'}],105).maximum,null);
  assert.equal(c.thermalSummary([{...sensor,kind:'Throttling',value:0}],105).throttled,false);
  assert.equal(c.thermalSummary([{...sensor,kind:'Throttling',value:1}],105).throttled,true);
  assert.equal(c.liveTemperature({...sensor,name:'Distance to TjMax',value:120},105),false);
  assert.equal(c.liveTemperature({...sensor,name:'Thermal Sensor Critical Limit',value:105},105),false);
  assert.equal(c.liveTemperature({...sensor,name:'Core Max',value:98},105),true);
  assert.equal(c.liveTemperature({...sensor,name:'GPU Hot Spot',value:85},105),true);
});
test('sensor grouping keeps device identities distinct and reference temperatures out of maxima',()=>{
  const c=diagnosticsHarness(),reading=(parent,hardware_type,name,kind,value,identifier)=>({parent,hardware_type,name,kind,value,identifier,sample_time:100});
  const list=[
    reading('/intelcpu/0','Cpu','CPU Package','Temperature',70,'cpu/temp/0'),
    reading('/intelcpu/0','Cpu','Distance to TjMax','Temperature',120,'cpu/temp/1'),
    reading('/intelcpu/0','Cpu','Core Max','Temperature',80,'cpu/temp/2'),
    reading('/intelcpu/0','Cpu','CPU Core #1','Load',30,'cpu/load/0'),
    reading('/gpu-nvidia/0','GpuNvidia','GPU Hot Spot','Temperature',85,'gpu/0/temp'),
    reading('/gpu-nvidia/1','GpuNvidia','GPU Core','Temperature',60,'gpu/1/temp'),
    reading('ACPI thermal zone',null,'THRM_0','Temperature',95,'windows/acpi/0'),
    reading('Storage 0',null,'NVMe disk','Temperature',42,'windows/storage/0'),
    reading('/memory/dimm/1','Memory','DIMM #1','Temperature',50,'memory/1/temp'),
    reading('/memory/dimm/1','Memory','Thermal Sensor Critical Limit','Temperature',105,'memory/1/critical'),
  ];
  const before=JSON.stringify(list),groups=c.groupSensors(list,105),cpu=groups.find(g=>g.category==='cpu');
  assert.equal(cpu.temperature,80);assert.equal(cpu.types.find(t=>t.kind==='Reference').sensors.length,1);
  assert.equal(groups.filter(g=>g.category==='gpu').length,2);
  assert.equal(groups.find(g=>g.category==='memory').temperature,50);
  assert.equal(groups.find(g=>g.category==='acpi').temperature,95);
  assert.equal(groups.find(g=>g.category==='storage').displayLabel,'Накопитель · NVMe disk');
  assert.equal(JSON.stringify(list),before);
  const reordered=c.groupSensors([...list].reverse(),105);
  assert.deepEqual([...groups].map(g=>g.key),[...reordered].map(g=>g.key));
  assert.equal(c.groupSensors([{...list[0],sample_time:70}],105)[0].temperature,null);
});
test('sensor disclosure state preserves explicit closures and temporarily absent groups',()=>{
  const c=diagnosticsHarness(),state=new Map([['cpu|/intelcpu/0',true],['gpu|/gpu-nvidia/0',false]]);
  const updated=c.rememberSensorDisclosure(state,[['cpu|/intelcpu/0',false],['cpu|/intelcpu/0|Temperature',false]]);
  assert.equal(c.sensorDisclosureOpen(updated,'cpu|/intelcpu/0'),false);
  assert.equal(c.sensorDisclosureOpen(updated,'cpu|/intelcpu/0|Temperature',true),false);
  assert.equal(updated.has('gpu|/gpu-nvidia/0'),true);
  assert.equal(c.sensorDisclosureOpen(updated,'new|Temperature',true),true);
  assert.equal(state.get('cpu|/intelcpu/0'),true);
});
test('sensor cache reuses unchanged samples but refreshes for staleness, locale and errors',()=>{
  const c=diagnosticsHarness(),list=[{identifier:'cpu/temp',parent:'/intelcpu/0',hardware_type:'Cpu',name:'CPU Package',kind:'Temperature',value:70,sample_time:100,history:[[100,70]]}],settings={extended_sensors:true},telemetry={extended:{diagnostics:{cpu_temperature_status:'available'}}};
  const signature=c.sensorPanelSignature(list,telemetry,settings,'ru',105);
  assert.equal(c.sensorPanelSignature(list,telemetry,settings,'ru',106),signature);
  assert.notEqual(c.sensorPanelSignature(list,telemetry,settings,'ru',121),signature);
  assert.notEqual(c.sensorPanelSignature(list,telemetry,settings,'en',105),signature);
  assert.notEqual(c.sensorPanelSignature([{...list[0],value:71}],telemetry,settings,'ru',105),signature);
  assert.notEqual(c.sensorPanelSignature(list,{...telemetry,error:'provider failed'},settings,'ru',105),signature);
  assert.notEqual(c.sensorPanelSignature(list,telemetry,{extended_sensors:false},'ru',105),signature);
});
test('CPU exclusion border reflects enabled protection and the toggle preserves other exclusions',()=>{
  const c=diagnosticsHarness(),config={cpu_guard:{enabled:false,excluded_cores:[22],process_threshold:3,sustain_seconds:3},balance:{enabled:false},rules:[]};
  const core={id:22,usage:50,mhz:3000};
  assert.equal(c.cpuExcluded(config,22),false);
  assert.doesNotMatch(c.coreButton(core,config,null),/class="[^"]*\bprotected\b/);
  const enabled=c.cpu22Config(config,true);assert.equal(c.cpuExcluded(enabled,22),true);
  assert.match(c.coreButton(core,enabled,null),/class="[^"]*\bprotected\b/);
  assert.equal(config.cpu_guard.enabled,false);
  const disabled=c.cpu22Config(enabled,false);assert.equal(disabled.cpu_guard.enabled,false);assert.equal(c.cpuExcluded(disabled,22),false);
  const other=c.cpu22Config({...enabled,cpu_guard:{...enabled.cpu_guard,excluded_cores:[3,22]}},false);
  assert.equal(other.cpu_guard.enabled,true);assert.deepEqual([...other.cpu_guard.excluded_cores],[3]);
  assert.equal(other.cpu_guard.process_threshold,3);
});
test('CPU temperature status distinguishes missing driver, permissions, and disabled collection',()=>{
  const c=diagnosticsHarness();c.data={settings:{extended_sensors:true},telemetry:{extended:{diagnostics:{cpu_temperature_status:'driver_missing'}}}};
  assert.match(c.cpuTemperatureNotice(),/PawnIO/);assert.match(c.cpuTemperatureNotice(),/не установлен/);
  c.data.telemetry.extended.diagnostics.cpu_temperature_status='access_required';assert.match(c.cpuTemperatureNotice(),/от администратора/);assert.doesNotMatch(c.cpuTemperatureNotice(),/не установлен/);
  c.data.settings.extended_sensors=false;assert.match(c.cpuTemperatureNotice(),/включите расширенные датчики/);
});
test('application grouping combines instances and separates shared helpers belonging to different apps',()=>{
  const c=diagnosticsHarness();
  const processes=[{pid:1,name:'ChatGPT.exe',path:'C:\\Apps\\ChatGPT.exe',identity:'a'},{pid:2,parent:1,name:'ChatGPT.exe',path:'c:\\apps\\chatgpt.exe',identity:'b'},{pid:3,parent:1,name:'msedgewebview2.exe',path:'C:\\WebView\\msedgewebview2.exe',identity:'c'},{pid:4,name:'other.exe',path:'C:\\Other\\other.exe',identity:'d'},{pid:5,parent:4,name:'msedgewebview2.exe',path:'C:\\WebView\\msedgewebview2.exe',identity:'e'}];
  const groups=c.applicationGroups(processes);assert.equal(groups.length,2);
  assert.deepEqual([...groups].map(g=>g.members.length).sort(),[2,3]);
  assert.equal(groups.find(g=>g.members.some(p=>p.pid===3)).anchor.pid,1);
  assert.equal(groups.find(g=>g.members.some(p=>p.pid===5)).anchor.pid,4);
  const detached=c.applicationGroups([{pid:7,name:'helper.exe',path:'C:\\Shared\\helper.exe',identity:'z',application_rule:'C:\\Apps\\ChatGPT.exe'}]);
  assert.equal(detached[0].target,'C:\\Apps\\ChatGPT.exe');
  const reused=c.applicationGroups([{pid:1,name:'other.exe',path:'C:\\Other\\other.exe',identity:'200'},{pid:2,parent:1,name:'msedgewebview2.exe',path:'C:\\Shared\\msedgewebview2.exe',identity:'100'}]);
  assert.equal(reused.length,2);assert.equal(reused.find(g=>g.members[0].pid===2).target,null);
});
test('20-second activity preserves names of exited processes and rejects reused identities',()=>{
  const {context:c,elements:e}=harness();c.data.processes=[{pid:9,name:'new.exe',identity:'new'}];
  c.renderActivity({id:22},{window_ms:20000,idle:90,unknown:0,rows:[{pid:9,name:'old.exe',identity:'old',usage:2,ms:400}]});
  assert.match(e.coreProcesses.innerHTML,/old.exe/);assert.doesNotMatch(e.coreProcesses.innerHTML,/data-pid="9"/);assert.match(e.coreHint.textContent,/20 секунд/);
  e.coreSearch.value='old.exe';c.renderActivity({id:22},{window_ms:20000,idle:90,unknown:0,rows:[{pid:9,name:'old.exe',identity:'old',usage:2,ms:400}]});assert.equal(e.coreMatches.textContent,'1 / 1');
});
test('all embedded modules parse as delivered and reference existing static or generated elements',()=>{
  const automation=fs.readFileSync(path.join(__dirname,'../assets/automation.js'),'utf8'),diagnostics=fs.readFileSync(path.join(__dirname,'../assets/diagnostics.js'),'utf8');
  const built=html.replace('/*__AUTOMATION__*/',automation).replace('/*__DIAGNOSTICS__*/',diagnostics);
  for(const m of built.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g))new vm.Script(m[1]);
  const ids=new Set([...built.matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
  for(const m of built.matchAll(/\$\(['"]([^'"]+)['"]\)/g))assert.ok(ids.has(m[1]),'Missing '+m[1]);
});
test('sensor histories retain the units of their actual readings',()=>{
  const c={};vm.createContext(c);
  vm.runInContext(html.slice(html.indexOf('      function sparkline('),html.indexOf('      function render()')),c);
  for(const [kind,unit] of [['Temperature','°C'],['Load','%'],['Clock','MHz'],['Power','W']]){
    const rendered=c.sparkline({kind,history:[[1,20],[2,30]]});assert.ok(rendered.includes('20.0 '+unit));
    if(kind!=='Temperature')assert.ok(!rendered.includes('°'));
  }
});
test('core threshold, search and idle are distinct; filters do not alter the source snapshot',()=>{
  const {context:c,elements:e}=harness();
  c.data.processes=[{pid:12,name:'worker',identity:'a',affinity:[0,1],allowed:[0,1]}];
  const sample={idle:94,unknown:0,rows:[{pid:12,usage:5,ms:50,history:[5]},{pid:99,usage:1,ms:10,history:[1]}]};
  e.activityThreshold.value='5';c.renderActivity({id:0},sample);
  assert.equal(e.coreMatches.textContent,'1 / 2');
  assert.match(e.coreProcesses.innerHTML,/worker/);assert.doesNotMatch(e.coreProcesses.innerHTML,/PID 99/);
  e.coreSearch.value='99';c.renderActivity({id:0},sample);
  assert.equal(e.coreMatches.textContent,'0 / 2');assert.match(e.coreProcesses.innerHTML,/Процессы не найдены/);
  assert.equal(sample.rows.length,2);
  e.activityThreshold.value='0';c.renderActivity({id:0},sample);
  assert.match(e.coreProcesses.innerHTML,/PID 99/);
});
test('focused activity row continues updating and keyboard focus is restored',()=>{
  const {context:c,elements:e}=harness();let restored=false;
  c.document.activeElement={dataset:{pid:'12'},matches:()=>false};
  e.coreProcesses.contains=()=>true;e.coreProcesses.querySelector=()=>({focus:()=>{restored=true}});
  c.data.processes=[{pid:12,name:'worker',identity:'a',affinity:[0],allowed:[0]}];
  c.renderActivity({id:0},{idle:90,unknown:0,rows:[{pid:12,usage:10,ms:100,history:[10]}]});
  assert.match(e.coreProcesses.innerHTML,/10.0%/);assert.equal(restored,true);
});
test('all referenced static UI IDs exist and inline JavaScript parses',()=>{
  const ids=new Set([...html.matchAll(/\bid="([^"]+)"/g)].map(m=>m[1]));
  for(const m of html.matchAll(/\$\("([^"]+)"\)/g))assert.ok(ids.has(m[1]),'Missing element '+m[1]);
  for(const m of html.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g))new vm.Script(m[1]);
});
test('Allowed and Denied remain independent of the global filter and retain process identity',()=>{
  const {context:c,elements:e}=harness();
  for(const id of ['coreTools','coreTitle','coreUsage','coreLatency','allowedTab','deniedTab','activityTab'])e[id]={classList:{toggle:()=>{}}};
  c.data.ready=true;c.data.cpus=[{id:0,usage:10}];c.filterCore=0;c.coreBusy=false;c.coreMode='allowed';c.bytes=String;
  c.data.processes=[{pid:12,name:'allowed',identity:'a',cpu:0,memory:0,affinity:[0],allowed:[0,1]},{pid:13,name:'denied',identity:'b',cpu:0,memory:0,affinity:[1],allowed:[0,1]}];
  vm.runInContext(html.slice(html.indexOf('      function renderCore('),html.indexOf('      function table(')),c);
  c.renderCore();assert.equal(e.coreMatches.textContent,'1 / 1');assert.match(e.coreProcesses.innerHTML,/data-identity="a"/);
  assert.equal(e.activityThreshold.hidden,true);
  c.coreMode='denied';c.renderCore();assert.match(e.coreProcesses.innerHTML,/data-identity="b"/);
  e.coreSearch.value='missing';c.renderCore();assert.equal(e.coreMatches.textContent,'0 / 1');
});
