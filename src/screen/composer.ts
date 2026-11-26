import type { Participant, Attachment, PostOutcome, MissedPost, IconName } from "../contracts";
import { invoke } from "@tauri-apps/api/core";
import type { createComposer } from "../composer";

export interface Dependencies {
  icon: (name: IconName) => SVGSVGElement;
  composerEl: HTMLElement;
  composerBoxEl: HTMLElement;
  attachmentsEl: HTMLUListElement;
  mentionListEl: HTMLUListElement;
  composer: ReturnType<typeof createComposer>;
  lastSeenId: string | null;
  shownTopicId: () => string;
  shownRoster: () => Participant[];
  status: (text: string, kind?: "info" | "error") => void;
  speakerColor: (name: string, hue: number | null, own: boolean) => string;
  drawMissed: (missed: MissedPost[]) => number;
  ATTACHMENT_HEAD: string;
  imageType: (name: string) => string | undefined;
  openViewer: (url: string, name: string) => void;
  localName: () => string;
}

export function createComposerScreen(deps: Dependencies) {

  /*
   * The list `@` opens in the composer (#204) is an aid to typing and nothing
   * more. What addresses a post is the `@名前` in its text: the room reads those
   * against the names it holds, moves each one that names someone into `to`, and
   * takes it out of the text (#206). A name typed out by hand addresses as a
   * picked one does.
   */

  /** Who the open list offers, and which of them the keys are on. */
  let mentionCandidates: Participant[] = [];
  let mentionActive = 0;
  /** Where the `@` the list is completing sits in the text; -1 when it is shut. */
  let mentionAt = -1;
  /** An `@` whose list was shut with Esc. Typing on after it does not open the
   *  list again; a different `@` does. */
  let mentionDismissedAt = -1;

  /**
   * Who can be addressed: everyone in the room on the glass but this screen's own
   * person, one entry per name.
   *
   * Sessions and people alike — the roster does not separate them. The room's
   * roster is the whole source, so an account with no session in it is absent by
   * construction: naming a participant who cannot be reached is a post addressed
   * to nobody, which is the reason addressees are picked from a roster at all
   * (#43). Names, deduplicated: `to` carries a display name, so two participants
   * answering to one name are one addressee, and offering both would be a choice
   * between two things that address the same pair.
   */
  function addressable(): Participant[] {
    const seen = new Set<string>();
    return deps.shownRoster().filter((one) => {
      if (one.own || seen.has(one.name)) return false;
      seen.add(one.name);
      return true;
    });
  }

  /**
   * The `@` the caret is completing, and what has been typed after it — or null
   * when the caret is not completing one.
   *
   * An `@` counts at the start of the text or after whitespace, so one in the
   * middle of a word (`a@b`) does not open the list. What follows may hold spaces,
   * because names do, but not a line break. The full-width `＠` counts too: it is
   * what an IME in kana mode types for the same key.
   */
  function mentionQuery(): { at: number; query: string } | null {
    // Read on the caret's line (#354): the box's text is lines and code blocks,
    // and an `@` is looked for in the one the caret is in, up to the caret.
    const { from, to, lineStart, before } = deps.composer.selection();
    if (from !== to) return null;
    const at = Math.max(before.lastIndexOf("@"), before.lastIndexOf("＠"));
    if (at < 0) return null;
    if (at > 0 && !/\s/.test(before[at - 1])) return null;
    const query = before.slice(at + 1);
    if (query.includes("\n")) return null;
    return { at: lineStart + at, query };
  }

  /**
   * Open, narrow or shut the list for where the caret is now.
   *
   * Narrowed to the names that start with what was typed after the `@`, case
   * aside. Nothing left is the list shut: after a pick the name and its space are
   * past the `@`, and no name starts with those, so typing on past a pick shuts it
   * without anything having to say so.
   */
  function refreshMentions(): void {
    const found = mentionQuery();
    if (!found || found.at !== mentionDismissedAt) mentionDismissedAt = -1;
    const query = found?.query.toLowerCase() ?? "";
    const candidates =
      found && found.at !== mentionDismissedAt
        ? addressable().filter((one) => one.name.toLowerCase().startsWith(query))
        : [];
    if (!found || !candidates.length) {
      closeMentions();
      return;
    }
    // The keys stay on the same participant while the list narrows around them.
    const current = mentionCandidates[mentionActive]?.name;
    const kept = candidates.findIndex((one) => one.name === current);
    mentionCandidates = candidates;
    mentionActive = kept >= 0 ? kept : 0;
    mentionAt = found.at;
    renderMentions();
  }

  /**
   * 宛先 on the composer's row (#222): type an `@` at the caret, as the key would.
   *
   * Only the typing. The list it opens, and what a name picked from it does, are
   * the `@` key's (#204, #206), so the button cannot address a post any way the
   * key does not. A space goes in first when the caret is against a word, because
   * an `@` in the middle of one does not open the list.
   */
  function typeMention(): void {
    const { from, to, before } = deps.composer.selection();
    const at = before === "" || /\s$/.test(before) ? "@" : " @";
    deps.composer.replace(from, to, at);
    refreshMentions();
  }

  function closeMentions(): void {
    mentionCandidates = [];
    mentionActive = 0;
    mentionAt = -1;
    deps.mentionListEl.hidden = true;
    deps.mentionListEl.replaceChildren();
  }

  function renderMentions(): void {
    deps.mentionListEl.replaceChildren();
    mentionCandidates.forEach((one, at) => {
      const item = document.createElement("li");
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(at === mentionActive));
      // In the colour the room draws them in, so the name picked is recognisably
      // the participant on the roster.
      item.style.setProperty("--speaker", deps.speakerColor(one.name, one.hue, false));
      const duplicates = deps.shownRoster().filter((seat) => seat.name === one.name).length;
      item.textContent = duplicates > 1 ? `${one.name}（同名の全参加者宛て）` : one.name;
      // mousedown rather than click: by the time a click lands the box has
      // lost focus, and the caret the pick goes in at with it.
      item.addEventListener("mousedown", (event) => {
        event.preventDefault();
        pickMention(at);
      });
      deps.mentionListEl.appendChild(item);
    });
    deps.mentionListEl.hidden = false;
    deps.mentionListEl.children[mentionActive]?.scrollIntoView({ block: "nearest" });
  }

  /**
   * Put a candidate into the text as `@名前`.
   *
   * What was typed after the `@` is replaced by the whole name and a space, so the
   * next word does not run into it.
   */
  function pickMention(at: number): void {
    const one = mentionCandidates[at];
    if (!one || mentionAt < 0) return;
    deps.composer.replace(mentionAt, deps.composer.selection().from, `@${one.name} `);
    closeMentions();
  }

  /**
   * The keys of an open list. Answers whether the key was the list's, in which
   * case it must not also send, or move the caret.
   *
   * Enter and Tab pick, the arrows move, Esc shuts. Shift+Enter is still a line
   * break. Nothing while an IME is composing: those keys belong to the conversion.
   */
  function mentionKey(event: KeyboardEvent): boolean {
    if (deps.mentionListEl.hidden || event.isComposing) return false;
    const count = mentionCandidates.length;
    switch (event.key) {
      case "ArrowDown":
        mentionActive = (mentionActive + 1) % count;
        renderMentions();
        break;
      case "ArrowUp":
        mentionActive = (mentionActive - 1 + count) % count;
        renderMentions();
        break;
      case "Enter":
      case "Tab":
        if (event.shiftKey) return false;
        pickMention(mentionActive);
        break;
      case "Escape":
        mentionDismissedAt = mentionAt;
        closeMentions();
        break;
      default:
        return false;
    }
    event.preventDefault();
    return true;
  }

  let attachments: Attachment[] = [];

  /** Let go of the pictures the chips that are gone were showing. */
  function dropPreviews(gone: Attachment[]): void {
    for (const one of gone) {
      if (one.preview) URL.revokeObjectURL(one.preview);
      one.preview = null;
    }
  }

  /** The last part of a path, either separator: what a dropped file is called. */
  function baseName(path: string): string {
    return path.split(/[\\/]/).pop() || path;
  }

  function attach(found: Attachment[]): void {
    if (!found.length) return;
    attachments = [...attachments, ...found];
    renderAttachments();
  }

  function attachFiles(files: Iterable<File>): void {
    attach(
      [...files].map((file) => ({
        // A pasted image arrives as `image.png`; a nameless one is still a file.
        name: file.name || "image.png",
        source: { file },
        saved: null,
        preview: deps.imageType(file.name || "image.png") ? URL.createObjectURL(file) : null,
      })),
    );
  }

  function attachPaths(paths: string[]): void {
    attach(
      paths.map((path) => ({ name: baseName(path), source: { path }, saved: null, preview: null })),
    );
  }

  function renderAttachments(): void {
    deps.attachmentsEl.replaceChildren();
    attachments.forEach((one, at) => {
      const chip = document.createElement("li");
      let name: HTMLElement;
      if (one.preview) {
        // A picture in place of the name (#318), opened larger by a press. The
        // name is still its label and its tooltip.
        const url = one.preview;
        chip.classList.add("image");
        const thumb = document.createElement("button");
        thumb.type = "button";
        thumb.className = "thumb";
        thumb.setAttribute("aria-label", `${one.name} を大きく見る`);
        const image = document.createElement("img");
        image.alt = "";
        image.src = url;
        thumb.appendChild(image);
        // The box keeps its focus through the press, so closing the larger
        // picture hands the focus back to it.
        thumb.addEventListener("mousedown", (event) => event.preventDefault());
        thumb.addEventListener("click", () => deps.openViewer(url, one.name));
        name = thumb;
      } else {
        name = document.createElement("span");
        name.className = "name";
        name.textContent = one.name;
      }
      name.title = one.name;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.setAttribute("aria-label", `${one.name} を外す`);
      remove.title = "外す";
      remove.appendChild(deps.icon("close"));
      // The box keeps its focus and caret through the press, as it does
      // for 宛先.
      remove.addEventListener("mousedown", (event) => event.preventDefault());
      remove.addEventListener("click", () => {
        dropPreviews([one]);
        attachments = attachments.filter((_, other) => other !== at);
        renderAttachments();
      });
      chip.append(name, remove);
      deps.attachmentsEl.appendChild(chip);
    });
    deps.attachmentsEl.hidden = attachments.length === 0;
  }

  /**
   * A name as a header can carry it: a JSON string with everything outside
   * printable ASCII escaped. The app reads it back with a JSON parse
   * (`room_attach_bytes`).
   */
  function asciiJson(text: string): string {
    return JSON.stringify(text).replace(
      /[\u007f-\uffff]/g,
      (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, "0")}`,
    );
  }

  /** Save one attachment under `topicId`, or answer where it already was. */
  async function saveAttachment(one: Attachment, topicId: string): Promise<string> {
    if (one.saved?.topicId === topicId) return one.saved.path;
    const path =
      "path" in one.source
        ? await invoke<string>("room_attach_path", { topicId, path: one.source.path })
        : await invoke<string>(
            "room_attach_bytes",
            new Uint8Array(await one.source.file.arrayBuffer()),
            { headers: { "Pullcept-Topic": topicId, "Pullcept-Name": asciiJson(one.name) } },
          );
    one.saved = { topicId, path };
    return path;
  }

  /**
   * The text a post with attachments carries: what was written, a blank line,
   * and the paths under `添付:`, one to a line. A post of nothing but files is
   * the block alone.
   */
  function withAttachments(text: string, paths: string[]): string {
    if (!paths.length) return text;
    const block = [deps.ATTACHMENT_HEAD, ...paths].join("\n");
    return text ? `${text}\n\n${block}` : block;
  }

  /**
   * Whether a point the drop reported is over the composer.
   *
   * The drop reports physical pixels from the webview's corner; the page lays
   * out in CSS pixels, and the two differ by the device pixel ratio.
   */
  function overComposer(position: { x: number; y: number }): boolean {
    const scale = window.devicePixelRatio || 1;
    const x = position.x / scale;
    const y = position.y / scale;
    const box = deps.composerEl.getBoundingClientRect();
    return x >= box.left && x <= box.right && y >= box.top && y <= box.bottom;
  }

  /** Show that letting go here attaches. */
  function markDrop(over: boolean): void {
    if (over) deps.composerBoxEl.dataset.drop = "over";
    else delete deps.composerBoxEl.dataset.drop;
  }

  async function send(): Promise<void> {
    const text = deps.composer.text().trim();
    const pending = attachments;
    if (!text && !pending.length) return;

    const speaker = deps.localName();
    // The topic on the glass as this was typed, which is the conversation the
    // watermark below belongs to. Read now, not after the round trip: the post is
    // said where it was written (#141).
    const topicId = deps.shownTopicId();
    // No addressee is sent beside the text: the `@名前` in it are the addressees,
    // and the room reads them out (#206). The app still delivers to everyone;
    // addressees are judgment material for the participants, not a delivery
    // filter.
    // Read before the await: what the screen had drawn when this was sent is the
    // watermark, and an arrival during the round trip must not be folded into it.
    const lastSeen = deps.lastSeenId;
    deps.composer.setText("");
    closeMentions();
    // Off the composer at once with the text, so a second Enter while the files
    // are being saved does not send them twice. Put back with it below on
    // anything but a delivery.
    attachments = [];
    renderAttachments();
    const putBack = (): void => {
      deps.composer.setText(text);
      attachments = [...pending, ...attachments];
      renderAttachments();
    };
    // Saved before the post, so every path in it names a file that is there by
    // the time a session reads it. One at a time, in the order the chips stand,
    // which is the order the paths are written in.
    const paths: string[] = [];
    try {
      for (const one of pending) paths.push(await saveAttachment(one, topicId));
    } catch (err) {
      putBack();
      deps.status(`添付を保存できませんでした: ${err}`, "error");
      return;
    }
    const content = withAttachments(text, paths);
    try {
      const outcome = await invoke<PostOutcome>("room_post", {
        topicId,
        speaker,
        content,
        lastSeen,
      });
      if (!outcome.delivered) {
        // Refused, not failed. The refusal carries the posts themselves, so they
        // go on the glass here and the person reads them and decides again — #47
        // unchanged, except that what it says to read is now there to read. The
        // reading is left where it belongs; only the means of doing it is added.
        // The text as written and the chips as they stood: the paths are the
        // chips, already saved, and go into the text again when it is sent.
        putBack();
        // Drawn only into the conversation they belong to. A topic opened during
        // the round trip is another conversation, and it has drawn its own log.
        const drew = topicId === deps.shownTopicId() ? deps.drawMissed(outcome.missed) : 0;
        const speakers = [...new Set(outcome.missed.map((one) => one.speaker))];
        // What was drawn, and nothing about how much arrived. Beyond the floor's
        // 512 the room cannot enumerate what it dropped and this screen cannot
        // count it, so a total here would be a number that looks complete and is
        // not.
        deps.status(
          drew
            ? `送っていません。書いている間に届いた発言 ${drew} 件を下へ描きました（${speakers.join("、")}）。読んでから送るか決めてください。`
            : `送っていません。書いている間に届いた発言（${speakers.join("、")}）は画面に出ています。読んでから送るか決めてください。`,
          "error",
        );
        return;
      }
      // Delivered: the chips are gone for good, and their pictures with them.
      dropPreviews(pending);
      deps.status("");
    } catch (err) {
      // Put the text back rather than losing what was typed.
      putBack();
      deps.status(`発言を送れませんでした: ${err}`, "error");
    }
  }

  return { send, mentionKey, refreshMentions, closeMentions, attachFiles, baseName, typeMention, markDrop, overComposer, attachPaths, asciiJson };
}
