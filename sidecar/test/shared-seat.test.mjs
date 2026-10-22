import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync,writeFileSync,rmSync,existsSync} from "node:fs";
import {tmpdir} from "node:os";
import {join,resolve} from "node:path";
import {spawnSync} from "node:child_process";
test("seat bootstrap stays reusable during its lease and fails closed after revocation", t=>{
 const root=mkdtempSync(join(tmpdir(),"pullcept-bootstrap-"));t.after(()=>rmSync(root,{recursive:true,force:true}));
 const file=join(root,"seat.json"),runner=join(root,"runner.mjs");
 writeFileSync(file,JSON.stringify({PULLCEPT_AGENT_NAME:"fixture",PULLCEPT_ROOM_TOKEN:"fixture-secret"}),{mode:0o600});
 writeFileSync(runner,'process.stdout.write(process.env.PULLCEPT_AGENT_NAME === "fixture" && process.env.PULLCEPT_ROOM_TOKEN === "fixture-secret" ? "matched" : "wrong");');
 const run=()=>spawnSync(process.execPath,[resolve("sidecar/src/shared-seat.mjs"),file,runner,"unused"],{encoding:"utf8",windowsHide:true});
 for(let i=0;i<2;i++){const r=run();assert.equal(r.status,0);assert.equal(r.stdout,"matched");assert.ok(existsSync(file));assert.ok(existsSync(join(root,"seat.ready")));}
 rmSync(file);const r=run();assert.equal(r.status,1);assert.equal(r.stdout,"");assert.match(r.stderr,/no longer available/);assert.ok(!r.stderr.includes(root));assert.ok(!r.stderr.includes("fixture-secret"));
});
