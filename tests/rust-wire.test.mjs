import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import test from "node:test";
import { randomUUID } from "node:crypto";
import { prepareSessionState } from "../lib/native-state.mjs";
import { fileURLToPath, pathToFileURL } from "node:url";
const binary = process.env.ZCODE_ACP_BIN || fileURLToPath(new URL(`../target/release/zcode-acp${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
const fixture = fileURLToPath(new URL("./support/fake-native-server.mjs", import.meta.url));
const expectedVersion = fs.readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
assert.ok(expectedVersion, "Cargo package version is required");
function start(t, mode = "normal", extra = {}, permission = "accept-edits") {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "zcode-rust-wire-"));
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^ZCODE_|TOKEN|SECRET|API_KEY|CREDENTIAL/.test(k)));
  Object.assign(env, { ZCODE_NODE: process.execPath, ZCODE_NATIVE_ENTRY: fixture, ZCODE_DATA_BASE_DIR: root, ZCODE_ACP_STATE_DIR: path.join(root,"state"), FAKE_NATIVE_MODE: mode, ...extra });
  const child = spawn(binary, [`--bb-permission=${permission}`], { env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
  const exit = once(child,"close"); const pending = new Map(), updates = [], permissions = []; let counter = 0, stderr = "";
  child.stdin.on("error",()=>{}); child.stderr.on("data",c=>{stderr+=c;});
  readline.createInterface({input:child.stdout}).on("line",line=>{
    const m=JSON.parse(line); assert.equal(m.jsonrpc,"2.0");
    if(m.method==="session/update") {updates.push(m.params.update);return;}
    if(m.method==="session/request_permission") {permissions.push(m.params);child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:m.id,result:{outcome:{outcome:"selected",optionId:extra.CHOICE||"once"}}})+"\n");return;}
    const p=pending.get(m.id);if(p){clearTimeout(p.timer);pending.delete(m.id);m.error?p.reject(Object.assign(new Error(m.error.message),{code:m.error.code})):p.resolve(m.result);}
  });
  child.once("close",()=>{for(const p of pending.values()){clearTimeout(p.timer);p.reject(new Error("ACP exited: "+stderr));}pending.clear();});
  const api={root,env,updates,permissions,
    request(method,params={}) { const id=++counter; return new Promise((resolve,reject)=>{const timer=setTimeout(()=>{pending.delete(id);reject(new Error("ACP timeout: "+method));},15_000);pending.set(id,{resolve,reject,timer});child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");}); },
    notify(method,params={}){child.stdin.write(JSON.stringify({jsonrpc:"2.0",method,params})+"\n");},
    async close(){child.stdin.end();const timer=setTimeout(()=>child.kill(),5000);await exit;clearTimeout(timer);},
  };
  t.after(async()=>{await api.close();assert.equal(stderr,"");assert.equal(path.dirname(root),path.resolve(os.tmpdir()));fs.rmSync(root,{recursive:true,force:true});});return api;
}
const open=a=>a.request("session/new",{cwd:process.cwd(),mcpServers:[]});
const prompt=(a,id)=>a.request("session/prompt",{sessionId:id,prompt:[{type:"text",text:"offline fixture"}]});

function embeddedBundle(t) {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),"zcode embedded bundle-"));
  const fixtureBundle={root,agent:null};
  t.after(async()=>{
    if(fixtureBundle.agent) await fixtureBundle.agent.close();
    assert.equal(path.dirname(root),path.resolve(os.tmpdir()));
    fs.rmSync(root,{recursive:true,force:true});
  });
  const contents=process.platform==="darwin"?path.join(root,"Z Code.app","Contents"):path.join(root,"Z Code");
  const resources=path.join(contents,process.platform==="darwin"?"Resources":"resources");
  const glm=path.join(resources,"glm");fs.mkdirSync(glm,{recursive:true});
  const executable=process.platform==="darwin"?path.join(contents,"MacOS","ZCode Test"):path.join(contents,"ZCode.exe");
  fs.mkdirSync(path.dirname(executable),{recursive:true});
  // A mock bundle uses a copied test Node binary. Real Electron is checked by
  // the separate installed-ZCode smoke; CI checks discovery and child flags.
  fs.copyFileSync(process.execPath,executable);fs.chmodSync(executable,0o755);
  if(process.platform==="darwin") fs.writeFileSync(path.join(contents,"Info.plist"),"<plist><dict><key>CFBundleExecutable</key><string>ZCode Test</string></dict></plist>");
  const entry=path.join(glm,"zcode.cjs");
  fs.writeFileSync(path.join(glm,".node-bundle-meta.json"),JSON.stringify({runtime:"electron-node",entry:"zcode.cjs",platform:`${process.platform}-${process.arch}`}));
  fs.writeFileSync(entry,`if ((process.env.ELECTRON_RUN_AS_NODE || '') !== process.env.EXPECT_RUN_AS_NODE) throw new Error('Unexpected child Electron flag');\nimport(${JSON.stringify(pathToFileURL(fixture).href)});\n`);
  const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!/^ZCODE_|TOKEN|SECRET|API_KEY|CREDENTIAL/.test(k)));
  Object.assign(env,{ZCODE_NODE:"",ZCODE_NATIVE_ENTRY:entry,ZCODE_DATA_BASE_DIR:root,ZCODE_ACP_STATE_DIR:path.join(root,"state"),EXPECT_RUN_AS_NODE:"1",ELECTRON_RUN_AS_NODE:"inherited-value"});
  env.PATH=process.platform==="win32"?[path.join(process.env.SystemRoot||"C:\\Windows","System32"),path.join(process.env.SystemRoot||"C:\\Windows","System32","WindowsPowerShell","v1.0")].join(path.delimiter):"/usr/bin:/bin";
  return Object.assign(fixtureBundle,{entry,executable,env});
}
const embeddedSupported=["win32","darwin"].includes(process.platform);
test("bundled runtime creates a session and completes an offline turn with no external Node on PATH",{skip:!embeddedSupported},async t=>{
  const f=embeddedBundle(t);const a=start(t,"normal",f.env);f.agent=a;
  const s=await open(a);assert.equal((await prompt(a,s.sessionId)).stopReason,"end_turn");
  assert.equal(a.updates.filter(u=>u.sessionUpdate==="agent_message_chunk").map(u=>u.content.text).join(""),"Привет 🌍");
});
test("doctor reports the automatically selected bundled runtime",{skip:!embeddedSupported},t=>{
  const parentFlag=process.env.ELECTRON_RUN_AS_NODE;
  const f=embeddedBundle(t);
  const r=JSON.parse(execFileSync(binary,["--doctor"],{env:f.env,windowsHide:true,encoding:"utf8",timeout:15_000}));
  assert.equal(r.ok,true);assert.deepEqual(r.nativeRuntime,{kind:"zcode-electron",executable:f.executable,source:"bundle"});
  assert.equal(r.modelRequestsSent,0);assert.equal(process.env.ELECTRON_RUN_AS_NODE,parentFlag);
});
test("explicit Node overrides the bundle and strips inherited Electron mode",{skip:!embeddedSupported},t=>{
  const f=embeddedBundle(t);const env={...f.env,ZCODE_NODE:process.execPath,EXPECT_RUN_AS_NODE:""};
  const r=JSON.parse(execFileSync(binary,["--doctor"],{env,windowsHide:true,encoding:"utf8",timeout:15_000}));
  assert.equal(r.ok,true);assert.equal(r.nativeRuntime.kind,"node");assert.equal(r.nativeRuntime.source,"override");
  assert.throws(()=>execFileSync(binary,["--doctor","--node",path.join(f.root,"missing-node")],{env,windowsHide:true,stdio:"pipe",timeout:15_000}),e=>JSON.parse(e.stdout).error.includes("override"));
});
test("an explicit path to the selected ZCode executable enables its Electron runtime",{skip:!embeddedSupported},t=>{
  const f=embeddedBundle(t);
  const r=JSON.parse(execFileSync(binary,["--doctor","--node",f.executable],{env:f.env,windowsHide:true,encoding:"utf8",timeout:15_000}));
  assert.equal(r.ok,true);assert.equal(r.nativeRuntime.kind,"zcode-electron");assert.equal(r.nativeRuntime.source,"override");
});
test("a loose CLI falls back to Node from PATH even when an unrelated executable is nearby",{skip:!embeddedSupported},t=>{
  const f=embeddedBundle(t);
  fs.unlinkSync(path.join(path.dirname(f.entry),".node-bundle-meta.json"));
  const env={...f.env,EXPECT_RUN_AS_NODE:"",PATH:path.dirname(process.execPath)+path.delimiter+f.env.PATH};
  const r=JSON.parse(execFileSync(binary,["--doctor"],{env,windowsHide:true,encoding:"utf8",timeout:15_000}));
  assert.equal(r.ok,true);assert.deepEqual(r.nativeRuntime,{kind:"node",executable:"node",source:"path"});
});
test("Rust CLI has a versioned JSON contract and authoritative native entry flag",()=>{
  const run=(args,env={})=>JSON.parse(execFileSync(binary,args,{env:{...process.env,...env},encoding:"utf8",windowsHide:true}));
  assert.equal(run(["--version"]).runtime,"rust");assert.equal(run(["--version"]).version,expectedVersion);assert.equal(run(["runtime-info","--json"]).schemaVersion,1);
  assert.equal(run(["inventory","--native-entry",fixture],{ZCODE_NATIVE_ENTRY:path.join(os.tmpdir(),"missing")}).installed,true);
  assert.equal(run(["inventory","--native-entry",path.join(os.tmpdir(),"missing")]).installed,false);
  assert.throws(()=>execFileSync(binary,["--native-entry"],{stdio:"pipe",windowsHide:true}));
});
for(const permission of ["accept-edits","full"]) test(`Rust ACP streams text, reasoning and tools with ${permission}`,async t=>{
  const a=start(t,"normal",{},permission);assert.equal((await a.request("initialize")).agentInfo.version,expectedVersion);const s=await open(a);
  assert.equal(s.modes.currentModeId,permission==="full"?"yolo":"edit");assert.equal(s.modes.availableModes.some(m=>m.id==="yolo"),permission==="full");
  assert.deepEqual(await prompt(a,s.sessionId),{stopReason:"end_turn"});assert.equal(a.updates.filter(u=>u.sessionUpdate==="agent_message_chunk").map(u=>u.content.text).join(""),"Привет 🌍");assert.ok(a.updates.some(u=>u.sessionUpdate==="agent_thought_chunk"));assert.equal(a.updates.filter(u=>u.sessionUpdate.startsWith("tool_call")).length,2);
  await a.request("session/close",{sessionId:s.sessionId});
});
for(const choice of ["once","always","deny"])test(`Permission ${choice} cannot become a permanent grant`,async t=>{const a=start(t,"permission",{CHOICE:choice});const s=await open(a);await prompt(a,s.sessionId);assert.deepEqual(a.permissions[0].options.map(o=>o.kind),["allow_once","reject_once"]);assert.equal(a.updates.at(-1).content.text,choice==="once"?"allow":"deny");});
test("Configuration keeps model identity and forbids permission escalation",async t=>{const a=start(t);const s=await open(a);const id=s.models.currentModelId;let r=await a.request("session/set_config_option",{sessionId:s.sessionId,configId:"effort",value:"max"});assert.equal(r.configOptions.find(o=>o.id==="effort").currentValue,"max");assert.equal(r.configOptions.find(o=>o.id==="model").currentValue,id);await assert.rejects(a.request("session/set_mode",{sessionId:s.sessionId,modeId:"yolo"}),e=>e.code===-32602);await assert.rejects(a.request("session/set_model",{sessionId:s.sessionId,modelId:"other"}),e=>e.code===-32602);});
test("Cancellation finishes an active prompt and notification cancellation works",async t=>{const a=start(t,"wait");const s=await open(a);const p=prompt(a,s.sessionId);await new Promise(r=>setTimeout(r,100));await assert.rejects(prompt(a,s.sessionId),/progress/);await a.request("session/cancel",{sessionId:s.sessionId});assert.equal((await p).stopReason,"cancelled");const next=prompt(a,s.sessionId);await new Promise(r=>setTimeout(r,100));a.notify("session/cancel",{sessionId:s.sessionId});assert.equal((await next).stopReason,"cancelled");});
test("Cold restore only recreates a known-empty session",async t=>{const a=start(t);const s=await open(a);await a.request("session/close",{sessionId:s.sessionId});const r=await a.request("session/load",{sessionId:s.sessionId,cwd:process.cwd(),mcpServers:[]});assert.equal(r.models.currentModelId,s.models.currentModelId);await prompt(a,s.sessionId);await a.request("session/close",{sessionId:s.sessionId});await assert.rejects(a.request("session/load",{sessionId:s.sessionId,cwd:process.cwd(),mcpServers:[]}),/nothing was resent/);});
test("Desktop sessions and another workspace cannot be resumed",async t=>{const a=start(t);await assert.rejects(a.request("session/load",{sessionId:"desktop-task",cwd:process.cwd()}),e=>e.code===-32602);const s=await open(a);await a.request("session/close",{sessionId:s.sessionId});await assert.rejects(a.request("session/load",{sessionId:s.sessionId,cwd:os.tmpdir()}),/workspace/);});
test("Rust resumes a known-empty binding written by the previous JS adapter", async t => {
  const a = start(t);
  const id = `bb-native-${randomUUID()}`;
  const state = prepareSessionState(id, process.cwd(), false, a.env);
  state.markReady();
  const result = await a.request("session/load", {sessionId:id,cwd:process.cwd(),mcpServers:[]});
  assert.ok(result.models.availableModels.length);
});
test("Concurrent cancellation requests are idempotent", async t => {
  const a = start(t,"wait"); const s = await open(a); const p = prompt(a,s.sessionId);
  await new Promise(r=>setTimeout(r,100));
  assert.deepEqual(await Promise.all([a.request("session/cancel",{sessionId:s.sessionId}),a.request("session/cancel",{sessionId:s.sessionId})]),[{},{}]);
  assert.equal((await p).stopReason,"cancelled");
});
test("Native error contents are redacted",async t=>{const a=start(t,"error");const s=await open(a);await assert.rejects(prompt(a,s.sessionId),e=>e.code===-32000&&!e.message.includes("do-not-leak-fixture-secret"));});
for(const mode of ["malformed","exit"])test(`Native ${mode} fails promptly`,async t=>{const a=start(t,mode);await assert.rejects(open(a),/closed/);});
test("Quota commands do not need Node, start sessions or mix plan sources",()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),"zcode-rust-quotas-"));try{
    const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!/^ZCODE_/.test(k)));Object.assign(env,{ZCODE_NATIVE_ENTRY:fixture,ZCODE_STORAGE_DIR:root,ZCODE_NODE:"does-not-exist"});
    for(const plan of ["coding","start"]){const data=JSON.parse(execFileSync(binary,["usage","--plan",plan,"--json"],{env,windowsHide:true,encoding:"utf8"}));assert.equal(data.status,"unauthenticated");assert.equal(data.planLabel,plan==="start"?"Start Plan":"Coding Plan");}
    assert.equal(JSON.parse(execFileSync(binary,["account-scope"],{env,encoding:"utf8",windowsHide:true})).scope,"signed-out");
  }finally{assert.equal(path.dirname(root),path.resolve(os.tmpdir()));fs.rmSync(root,{recursive:true,force:true});}
});
