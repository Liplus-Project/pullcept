// Execute the actual frontend functions, without starting Tauri or a CLI.
import { test } from "node:test";
import assert from "node:assert/strict";
import { tsImport } from "tsx/esm/api";
const { activityNote, limitedByUsage, statsForView } = await tsImport("../../src/seat-status.ts", import.meta.url);
function view(source, limited) {
  return {ended:null,topicId:"topic",outputting:true,silent:false,
    stats:{limited,limited_source:source,five_hour:100,seven_day:100},
    activity:{word:"委任中",line:"child still listed",waiting:false,connected:true}};
}
test("confirmed Claude parent rejection outranks stale delegation and terminal repaint without removing it", () => {
  const seat = view("claude-parent", true);
  assert.equal(activityNote(seat).word, "制限中");
  assert.equal(seat.activity.word, "委任中");
  seat.stats.limited = false;
  assert.equal(activityNote(seat).word, "委任中");
  seat.activity = null;
  assert.equal(activityNote(seat).word, "出力中");
});
test("a Codex seat stopped at its limit outranks leftover work and terminal repaint (#372)", () => {
  const seat = view("codex", true);
  assert.equal(activityNote(seat).word, "制限中");
  seat.activity.word = "許可待ち"; seat.activity.waiting = true;
  assert.equal(activityNote(seat).word, "制限中");
  seat.activity = null;
  assert.equal(activityNote(seat).word, "制限中");
  seat.stats.limited = false;
  assert.equal(activityNote(seat).word, "出力中");
  seat.outputting = false;
  assert.equal(activityNote(seat).word, "");
});

test("reloaded views accept the current PTY snapshot and reject delayed old-launch stats", () => {
  const seat = {ptyId:"new-pty",ended:null};
  assert.equal(statsForView({pty_id:"old-pty"}, seat), false);
  assert.equal(statsForView({pty_id:"new-pty"}, seat), true);
  assert.equal(statsForView({pty_id:null}, seat), true);
  seat.ended = 1;
  assert.equal(statsForView({pty_id:"new-pty"}, seat), false);
});

test("room addressing is explicit and never inferred from history", () => {
  const seat = view(null, false); seat.activity = null;
  assert.equal(activityNote(seat, true).word, "考え中…");
  assert.equal(activityNote(seat, false).word, "出力中");
  seat.outputting = false; seat.silent = true;
  assert.equal(activityNote(seat, true).word, "待機");
  assert.equal(limitedByUsage({five_hour: 100, seven_day: null}), true);
  assert.equal(limitedByUsage({five_hour: 100, limited: false}), false);
});
