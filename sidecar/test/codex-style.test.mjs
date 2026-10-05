import {test} from "node:test";
import assert from "node:assert/strict";
import {mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, realpathSync} from "node:fs";
import {join} from "node:path";
import {tmpdir} from "node:os";
import {fileModeRoot, resolveOutputStyle} from "../src/codex-style.mjs";

// A protocol peer fixture, not a copy of the installed style parser.
const peer = `import sys,json,hashlib,pathlib,os
args=sys.argv[1:]
root=pathlib.Path(args[args.index('--root')+1]).resolve()
settings=json.loads((root/'peer.json').read_text())
if settings.get('failure'):
 print(json.dumps({'protocol_version':1,'status':'blocked','reason':'invalid_style_or_project'}))
 sys.stderr.write('SECRET_FIXTURE')
 sys.exit(1)
name=args[args.index('--style')+1] if '--style' in args else settings.get('name','character_instance')
if os.environ.get('LI_PLUS_OUTPUT_STYLE'): name='INHERITED_BAD'
mode=settings.get('mode','file')
if '--style' in args: mode='file'
if mode=='disabled': name=None
helper=pathlib.Path(__file__).resolve()
reply={'protocol_version':1,'mode':mode,'name':name,'root':str(root),'sha256':'a'*64 if mode=='file' else None,
 'byte_count':100000 if mode=='file' else 0,'handler':{'path':str(helper),'sha256':hashlib.sha256(helper.read_bytes()).hexdigest(),
 'additional_context_limit':0,'matchers':['startup','resume','clear','compact']}}
reply.update(settings.get('reply',{}))
print(json.dumps(reply))
`;
function fixture() {
  const scratch = mkdtempSync(join(tmpdir(), "pullcept styles "));
  const root = realpathSync.native(scratch), nested = join(root, "nested");
  mkdirSync(nested); mkdirSync(join(root, ".codex/hooks"), {recursive:true});
  mkdirSync(join(root, ".codex/output-styles"));
  const helper = join(root, ".codex/hooks/codex-output-style.py"), path = join(root, ".codex/hooks.json");
  writeFileSync(helper, peer); writeFileSync(path, "{}"); writeFileSync(join(root, "peer.json"), "{}");
  const discovery = {projects:[{folder:join(root, ".codex"), disabled:null}], styleHooks:[{
    source:"project", path, trust:"trusted", enabled:true, async:false, additionalContextLimit:0,
    matcher:"startup|resume|clear|compact",
    command:`${process.platform === "win32" ? "python" : "python3"} "${root}/.codex/hooks/codex-output-style.py" hook --root "${root}"`,
  }]};
  return {root, nested, helper, discovery, settings(value) { writeFileSync(join(root, "peer.json"), JSON.stringify(value)); },
    close() { rmSync(root, {recursive:true, force:true}); }};
}

test("file-mode roots use only active native layers, nested priority and default directory", async () => {
  const f = fixture();
  try {
    const folder = join(f.root, ".codex");
    assert.equal(await fileModeRoot([{folder, disabled:"untrusted", outputStyle:"A"}], f.nested), null);
    assert.equal(await fileModeRoot([{folder, disabled:null}], f.nested), f.root);
    mkdirSync(join(f.nested, ".codex/output-styles"), {recursive:true});
    assert.equal(await fileModeRoot([{folder:join(f.nested, ".codex"), disabled:null}, {folder, disabled:null, outputStyle:false}], f.nested), f.nested);
    await assert.rejects(fileModeRoot([{folder:join(f.root, ".codex"), disabled:null, outputStyle:"A"}], tmpdir()));
    rmSync(join(f.root, ".codex/output-styles"), {recursive:true});
    assert.equal(await fileModeRoot([{folder, disabled:null}], f.root), null);
    assert.equal(await fileModeRoot([{folder, disabled:null, outputStyle:false}], f.root), f.root);
  } finally { f.close(); }
});

test("default, false, account replacement and independent environments return metadata only", async () => {
  const f = fixture();
  try {
    const env = {...process.env, LI_PLUS_OUTPUT_STYLE:"INHERITED_SECRET"};
    assert.equal((await resolveOutputStyle(f.discovery, f.nested, "", env)).name, "character_instance");
    const [luna, lala] = await Promise.all(["character_codex_luna", "character_Codex_Lala"].map(name => resolveOutputStyle(f.discovery, f.nested, name, env)));
    assert.equal(luna.name, "character_codex_luna"); assert.equal(lala.name, "character_Codex_Lala");
    assert.equal(luna.byte_count, 100000); assert.equal(luna.protocol_version, 1);
    assert.equal(Object.hasOwn(luna, "body"), false); assert.equal(Object.hasOwn(luna, "instructions"), false);
    assert.equal(env.LI_PLUS_OUTPUT_STYLE, "INHERITED_SECRET");
    f.settings({mode:"disabled"});
    assert.equal((await resolveOutputStyle(f.discovery, f.root, "", env)).mode, "disabled");
    assert.equal((await resolveOutputStyle(f.discovery, f.root, "character_codex_luna", env)).mode, "file");
    f.discovery.projects[0].disabled = "untrusted";
    assert.deepEqual(await resolveOutputStyle(f.discovery, f.root, "character_Lin", env), {mode:"legacy", name:"character_Lin"});
  } finally { f.close(); }
});

test("file mode rejects missing, untrusted, modified, duplicate and partial handlers before helper execution", async () => {
  const f = fixture();
  try {
    const original = {...f.discovery.styleHooks[0]};
    for (const change of [{trust:"untrusted"}, {trust:"modified"}, {async:true}, {additionalContextLimit:2500},
      {matcher:"startup"}, {source:"user"}, {command:original.command + " SECRET_FIXTURE"}, {enabled:false}]) {
      f.discovery.styleHooks = [{...original, ...change}];
      await assert.rejects(resolveOutputStyle(f.discovery, f.root, ""), error => !error.message.includes("SECRET_FIXTURE"));
    }
    f.discovery.styleHooks = [original, original];
    await assert.rejects(resolveOutputStyle(f.discovery, f.root, ""));
    f.discovery.styleHooks = [];
    await assert.rejects(resolveOutputStyle(f.discovery, f.root, ""));
    f.discovery.styleHooks = [original]; rmSync(f.helper);
    await assert.rejects(resolveOutputStyle(f.discovery, f.root, ""));
  } finally { f.close(); }
});

test("loader failures and malformed protocol fail closed without exposing output", async () => {
  const f = fixture();
  try {
    for (const settings of [{failure:true}, {reply:{protocol_version:2}}, {reply:{mode:"legacy"}},
      {reply:{root:tmpdir()}}, {reply:{byte_count:131073}}, {reply:{name:"wrong"}},
      {reply:{sha256:null}}, {reply:{handler:null}}]) {
      f.settings(settings);
      await assert.rejects(resolveOutputStyle(f.discovery, f.root, "character_codex_luna"), error => !error.message.includes("SECRET_FIXTURE"));
    }
    writeFileSync(f.helper, "print('SECRET_FIXTURE')\n");
    await assert.rejects(resolveOutputStyle(f.discovery, f.root, ""), error => !error.message.includes("SECRET_FIXTURE"));
    const idHelper = readFileSync(new URL("../src/codex-session.mjs", import.meta.url), "utf8");
    assert.equal(idHelper.includes("LI_PLUS_OUTPUT_STYLE"), false);
  } finally { f.close(); }
});
