import { invoke } from "@tauri-apps/api/core";
import type { Fold, RoomMessage, LoggedPost, PermissionCard, PermissionResolved, MissedPost, TextPiece, Participant, Account } from "../contracts";
import { lineInFence, findCodeFences, highlightedCode } from "../composer";
import { appendMarkdown } from "../markdown";
import type { createHtmlPreview } from "../html-preview";

export interface Dependencies {
  roomEl: HTMLElement;
  roomGliding: boolean;
  viewerEl: HTMLDialogElement;
  viewerImageEl: HTMLImageElement;
  accounts: Account[];
  rosters: Map<string, Participant[]>;
  lastSeenId: string | null;
  drawnIds: Set<string>;
  awaiting: Map<string, Set<string>>;
  shownTopicId: () => string;
  shownRoster: () => Participant[];
  status: (text: string, kind?: "info" | "error") => void;
  localAccount: () => Account | null;
  speakerColor: (name: string, hue: number | null, own: boolean) => string;
  shortTime: (iso: string) => string;
  dayKey: (iso: string) => string;
  dayLabel: (iso: string) => string;
  fullDateTime: (iso: string) => string;
  roomAtBottom: () => boolean;
  glideRoomToFoot: (behavior: "smooth" | "instant") => void;
  syncScrollLatest: () => void;
  scrollRoomToLatest: (behavior: "smooth" | "instant") => void;
  attachmentsRoot: string | null;
  htmlPreview: ReturnType<typeof createHtmlPreview>;
  localName: () => string;
  renderPanel: () => void;
  baseName: (path: string) => string;
}

export function createMessages(deps: Dependencies) {

  /**
   * One line of the room, whenever it was said.
   *
   * The same element for a post arriving now and a post read back out of the
   * topic's log, because they are the same conversation: picking a topic puts its
   * posts in the room rather than beside it (#115, decision 2). The stamp is the
   * clock alone for both: the day is the divider above the line (`placeDay`,
   * #225).
   *
   * Who said it is a circle carrying the speaker's initial, on a tint of their
   * own colour (#225). It replaced the colour bar down the line's left edge; the
   * colour is the same one, so this circle and the one beside the name in the
   * panel still match — the panel draws it with the same function (#253).
   *
   * The screen person's own line is a bubble at the right edge (#185) and carries
   * neither the circle nor the name: where it stands and its tint already say
   * whose it is. Its clock is at its foot, and a run of them shows only the last
   * one's (src/styles/messages.css, `.message.mine`).
   */
  function roomLine(line: {
    speaker: string;
    full_name?: string | null;
    /** The account it was said as, or null when none was declared — what the
     *  circle's image is looked up by (#236). */
    account: string | null;
    /** Said by the app itself: the circle carries the app's icon (#340). */
    app: boolean;
    colour: string;
    /** Empty for a line said to the room. */
    to: string[];
    ts: string;
    stamp: string;
    content: string;
    past: boolean;
    /** Drawn at the right edge, as the screen person's own words (#185). */
    mine: boolean;
  }): HTMLElement {
    const article = document.createElement("article");
    article.className = "message";
    if (line.past) article.classList.add("past");
    if (line.mine) article.classList.add("mine");
    // The screen person's lines take their colour from the room, not from the
    // moment they were drawn, so a colour chosen in the account reaches every one
    // of them at once — the live and the read-back alike (#189, `paintMine`).
    article.style.setProperty("--speaker", line.mine ? MINE_SPEAKER : line.colour);

    const head = document.createElement("div");
    head.className = "meta";

    if (!line.mine) {
      article.appendChild(avatar(line.speaker, line.account, line.app));
      const speaker = document.createElement("span");
      speaker.className = "speaker";
      speaker.textContent = line.speaker;
      head.appendChild(speaker);
    }

    // Each addressee as `@name`, in that addressee's colour (#452). The form is
    // the one an addressee is typed in, so the heading reads as what was said to
    // whom; it is drawn from `to`, and nothing in it is read back as an address.
    if (line.to.length) {
      const to = document.createElement("span");
      to.className = "to";
      for (const name of line.to) {
        const one = document.createElement("span");
        one.className = "addressee";
        one.style.setProperty("--addressee", addresseeColour(name));
        one.textContent = `@${name}`;
        to.appendChild(one);
      }
      head.appendChild(to);
    }

    const time = document.createElement("time");
    time.className = "ts";
    time.dateTime = line.ts;
    time.textContent = line.stamp;
    time.title = deps.fullDateTime(line.ts);

    // What was attached is drawn rather than written out (#318): the paths under
    // `添付:` become pictures and chips below the words, and the words are what
    // is left. Only the screen reads it so; the post itself is unchanged.
    const { text, paths } = splitAttachments(line.content);
    const said: HTMLElement[] = [];
    if (text || !paths.length) {
      const body = document.createElement("div");
      body.className = "body";
      // ``` fences are drawn as code blocks (#348); the words around them as
      // before.
      appendPostWords(body, text);
      said.push(body);
    }
    if (paths.length) said.push(postAttachments(paths));

    if (line.mine) {
      // The clock goes to the bubble's foot, so a run of them can keep only the
      // last one's. A head is drawn only when it has an addressee to carry.
      if (head.childElementCount) article.appendChild(head);
      article.append(...said, time);
    } else {
      head.appendChild(time);
      article.append(head, ...said);
    }
    return article;
  }

  /**
   * The circle that says who a line is from (#225): the name's first character on
   * a tint of the speaker's colour, taken from `--speaker` on the element it
   * stands in. Hidden from a screen reader, which reads the name beside it.
   *
   * The account's image instead of the character, when the line names an account
   * that carries one (#236). The account rather than the name, because two
   * accounts may share a name and an image is one account's: a speaker with no
   * account, and an account with no image, keep the character. The circle
   * remembers which account it is for, so an image that arrives or changes after
   * the line was drawn reaches it (`setAvatarImage`).
   *
   * The app's icon instead, when the line is the app's own (#340). Decided by
   * the mark the room put on the post, never by the name: a participant may
   * answer to `Pullcept`, and their line keeps its character. The app speaks as
   * no account, so nothing repaints this circle afterwards.
   */
  function avatar(name: string, accountId: string | null = null, app = false): HTMLElement {
    const mark = document.createElement("span");
    mark.className = "avatar";
    mark.setAttribute("aria-hidden", "true");
    mark.dataset.initial = initialOf(name);
    if (app) {
      mark.classList.add("app");
      drawAvatar(mark, APP_ICON_URL);
      return mark;
    }
    if (accountId) mark.dataset.account = accountId;
    paintAvatar(mark);
    return mark;
  }

  /**
   * The app's icon, as the app's own lines carry it (#340): the same drawing the
   * title bar shows and every size in src-tauri/icons/ is made from (#246), so
   * one picture is the app wherever it appears. Resolved here so the build
   * bundles it the way it bundles the title bar's.
   */
  const APP_ICON_URL = new URL("../../src-tauri/app-icon.svg", import.meta.url).href;

  /** The character a circle carries for `name`: its first, upper-cased. */
  function initialOf(name: string): string {
    return (Array.from(name.trim())[0] ?? "?").toUpperCase();
  }

  /** Draw a circle from the account it names: the image if there is one. */
  function paintAvatar(mark: HTMLElement): void {
    const account = mark.dataset.account;
    drawAvatar(mark, account ? avatarImages.get(account) : undefined);
  }

  /**
   * Put `url`'s image in a circle, or its character when there is no image.
   *
   * An image that will not load is dropped for the character without a word
   * (#236): the file may have been removed or broken behind the app's back, and
   * the character is what the circle said before there was an image at all.
   */
  function drawAvatar(mark: HTMLElement, url: string | undefined): void {
    const initial = () => {
      mark.classList.remove("image");
      mark.textContent = mark.dataset.initial ?? "?";
    };
    if (!url) {
      initial();
      return;
    }
    const image = document.createElement("img");
    image.alt = "";
    image.addEventListener("error", initial, { once: true });
    image.src = url;
    mark.classList.add("image");
    mark.replaceChildren(image);
  }

  /**
   * The images accounts carry, as object URLs by account id (#236).
   *
   * Read from the app once — when the config is read, and when an image is
   * picked — rather than with every line drawn: a room read back is many lines
   * from few speakers. An account is here only while its image is: one without an
   * image, one whose file would not read, and one deleted draw the character.
   */
  const avatarImages = new Map<string, string>();

  /**
   * Set or drop one account's image, and redraw every circle on the screen that
   * is that account's — lines drawn before the image arrived included.
   */
  function setAvatarImage(accountId: string, image: Blob | null): void {
    const old = avatarImages.get(accountId);
    if (old) URL.revokeObjectURL(old);
    if (image) avatarImages.set(accountId, URL.createObjectURL(image));
    else avatarImages.delete(accountId);
    for (const mark of document.querySelectorAll<HTMLElement>(".avatar[data-account]")) {
      if (mark.dataset.account === accountId) paintAvatar(mark);
    }
  }

  /**
   * Read one account's image from the app (`account_avatar`). A file that is not
   * there or will not read leaves the account on its character, silently
   * (#236): the flag on the account is not corrected for it, and the next 選ぶ
   * writes the file again.
   */
  async function loadAvatarImage(accountId: string): Promise<void> {
    let bytes: ArrayBuffer;
    try {
      bytes = await invoke<ArrayBuffer>("account_avatar", { accountId });
    } catch {
      setAvatarImage(accountId, null);
      return;
    }
    setAvatarImage(accountId, new Blob([bytes], { type: "image/png" }));
  }

  /** Read the image of every account that carries one (#236). */
  function loadAvatarImages(): void {
    for (const account of deps.accounts) {
      if (account.avatar) void loadAvatarImage(account.id);
    }
  }

  /**
   * Put a day divider in the room ahead of a line, when the line is the first of
   * its day on the glass (#225). The day was on every read-back line's stamp
   * until then; it is said once, between the days, and the lines keep the clock.
   *
   * The day the room last drew is `roomDay`, cleared with the room in
   * `drawTopic`. A divider says 今日 by the clock it was drawn at, so when a new
   * one is drawn every divider on the glass is relabelled — a room left open
   * past midnight does not end up with two of them saying 今日.
   */
  let roomDay = "";

  function placeDay(ts: string): void {
    const key = deps.dayKey(ts);
    if (key === "" || key === roomDay) return;
    roomDay = key;
    const divider = document.createElement("div");
    divider.className = "day";
    divider.setAttribute("role", "separator");
    divider.dataset.ts = ts;
    divider.appendChild(document.createElement("span"));
    deps.roomEl.appendChild(divider);
    for (const one of deps.roomEl.querySelectorAll<HTMLElement>(":scope > .day")) {
      one.firstElementChild!.textContent = deps.dayLabel(one.dataset.ts ?? "");
    }
  }

  /**
   * The name notices were posted under before they were said as an account
   * (#169 → #193). Read, never written: a line in an older topic's log carries this
   * name and no account, and it is folded as it was when it was said.
   */
  const LEGACY_NOTICE_SPEAKER = "webhook";

  function foldOf(speaker: string, account: string | null | undefined): Fold | null {
    if (account) {
      const owner = deps.accounts.find((one) => one.id === account);
      if (owner?.kind !== "mcp") return null;
      return {
        key: owner.id,
        account: owner.id,
        label: owner.name,
        colour: deps.speakerColor(owner.name, owner.hue, false),
      };
    }
    if (speaker === LEGACY_NOTICE_SPEAKER) {
      return {
        key: `legacy:${speaker}`,
        account: null,
        label: speaker,
        colour: deps.speakerColor(speaker, null, false),
      };
    }
    return null;
  }

  /**
   * Whether a line is drawn at the right edge, as the screen person's (#185).
   *
   * Decided by the name: a line read back from the log carries no `own`, and a
   * live line and a read-back one must land on the same side. `own` would answer
   * the live half better — the room decides it on the connection — but it has no
   * read-back half, and a rule that switched between the two would move a line
   * from one side to the other when its topic is picked again. So this is where a
   * line sits, not who said it: someone joining under this screen's name is drawn
   * on this side too, and after a rename the lines said under the old name return
   * to the left once redrawn. Colour keeps its own ladder (`speakerColor`); only
   * the side is decided here. A folded line is never on this side (#185).
   */
  function isMine(speaker: string, fold: Fold | null): boolean {
    return fold === null && speaker === deps.localName();
  }

  /**
   * The colour of the screen person's lines, as one value on the room (#189).
   *
   * A line's colour was fixed when it was drawn, so a colour chosen in the
   * account reached only what was said after it, and a read-back line never
   * reached it at all: the log carries no hue, and the name-derived one is not
   * the one chosen. The screen person's lines are the one set whose colour this
   * screen knows without the room telling it — it is the account's — so they
   * point at this value instead, and setting it repaints all of them.
   *
   * The same ladder as every other line (`speakerColor`), read as oneself: the
   * account's hue if one is chosen, else the accent. Which lines are the screen
   * person's is `isMine`'s answer, by name, so this colour goes where the right
   * edge does — a line said under this screen's name by someone else included,
   * the same cost the side already carries.
   */
  const MINE_SPEAKER = "var(--mine-speaker, var(--accent))";

  /**
   * The colour an addressee's name is drawn in (#452): the colour that name has
   * where it speaks. This screen's own name takes the screen person's colour; a
   * participant of the topic on the glass takes their declared hue; an account
   * that is not in the room takes its own; anything else falls to the name-derived
   * colour, the same last rung `speakerColor` has.
   */
  function addresseeColour(name: string): string {
    if (name === deps.localName()) return MINE_SPEAKER;
    const present = deps.shownRoster().find((one) => one.name === name);
    if (present) return deps.speakerColor(name, present.hue, false);
    const account = deps.accounts.find((one) => one.name === name);
    return deps.speakerColor(name, account?.hue ?? null, false);
  }

  function paintMine(): void {
    deps.roomEl.style.setProperty(
      "--mine-speaker",
      deps.speakerColor(deps.localName(), deps.localAccount()?.hue ?? null, true),
    );
  }

  /**
   * Put one line in the room, folding notices away (#169 / #193).
   *
   * A notice is drawn folded, and a run of them — everything one server says until
   * someone else speaks — is one fold, headed with how many it holds and opened by
   * a click. The fold is the screen's alone: each notice is still its own post in
   * the room, in the log and typed into every session's terminal, and nothing but
   * this drawing groups them. Which lines fold is `foldOf`'s answer.
   */
  function placeLine(line: HTMLElement, fold: Fold | null): void {
    if (!fold) {
      deps.roomEl.appendChild(line);
      return;
    }
    const last = deps.roomEl.lastElementChild;
    let box =
      last instanceof HTMLDetailsElement &&
      last.classList.contains("notice-fold") &&
      last.dataset.fold === fold.key
        ? last
        : null;
    if (!box) {
      box = document.createElement("details");
      box.className = "notice-fold";
      box.dataset.fold = fold.key;
      box.style.setProperty("--speaker", fold.colour);
      // Headed by the server's circle, as its lines are (#225).
      const summary = document.createElement("summary");
      const label = document.createElement("span");
      label.className = "label";
      summary.append(avatar(fold.label, fold.account), label);
      box.appendChild(summary);
      deps.roomEl.appendChild(box);
    }
    box.appendChild(line);
    const count = box.querySelectorAll(":scope > .message").length;
    box.querySelector("summary > .label")!.textContent = `${fold.label} ${count} 件`;
  }

  function appendMessage(message: RoomMessage): void {
    // The room is scrolled to the bottom only when it already was, so reading
    // back through the log is not yanked away by an arriving message.
    const atBottom = deps.roomAtBottom();

    const fold = foldOf(message.speaker, message.account);
    placeDay(message.ts);
    const line = roomLine({
      speaker: message.speaker,
      full_name: message.full_name,
      account: message.account,
      app: message.from_app,
      // `own` rather than a name test: the room decides self on the connection
      // a post arrived on, which a rename cannot blur (#40). A folded line takes
      // its account's colour, so it is the colour it has when read back (#193).
      colour: fold?.colour ?? deps.speakerColor(message.speaker, message.hue, message.own),
      to: message.to,
      ts: message.ts,
      stamp: deps.shortTime(message.ts),
      content: message.content,
      past: false,
      mine: isMine(message.speaker, fold),
    });
    // A line that has just been said fades in (#307). Only here: a topic opened
    // draws what was already said, and that is not arriving.
    line.classList.add("arriving");
    placeLine(line, fold);
    // On the glass, so it is what this screen can declare having seen. Own posts
    // included: the room does not hold a speaker's own posts against them, and
    // carrying the newest id either way keeps this one value rather than two.
    deps.lastSeenId = message.message_id;
    deps.drawnIds.add(message.message_id);

    // Gliding down, and only from the foot (#307): a person reading further up
    // is left where they are, as before (#243).
    if (atBottom) deps.glideRoomToFoot("smooth");
    deps.syncScrollLatest();
  }

  /**
   * Put a topic's posts in the room, in place of whatever was there.
   *
   * Picking a topic is picking where the conversation is, so what is drawn is the
   * topic itself and not a strip beside it. #48 held the opposite — the room
   * started empty and the past was read in a column of its own — and #115
   * overwrites that line: the column is a list of topics now, and the room is
   * where a topic is read.
   *
   * The watermark is the last line drawn. A topic's room keeps its floor while the
   * screen is elsewhere (#141) — nothing empties it on the way in any more — and
   * every post that floor holds was written to the log inside the acquisition
   * that admitted it, so the newest line read back is the newest post on the
   * floor whenever the floor holds any. When it holds none, or the line is from
   * an earlier run, the room reads the watermark as naming nothing, which costs
   * one refusal and draws nothing twice (`drawMissed`).
   *
   * The ids are kept, because they are on the glass: a refusal that hands back
   * the whole retained floor must not draw a line twice (#108).
   *
   * The colour is derived from the name and `own` is false for every line. The
   * log carries neither declaration (see `LoggedPost`). The screen person's own
   * lines are the exception: they take this screen's colour from `paintMine`,
   * which is the account's and needs nothing from the log (#189).
   */
  function drawTopic(posts: LoggedPost[]): void {
    deps.roomEl.replaceChildren();
    forgetAttachmentUrls();
    roomDay = "";
    deps.lastSeenId = posts[posts.length - 1]?.message_id ?? null;
    deps.drawnIds.clear();

    for (const post of posts) {
      const fold = foldOf(post.speaker, post.account);
      placeDay(post.ts);
      placeLine(
        roomLine({
          speaker: post.speaker,
          full_name: post.full_name,
          account: post.account ?? null,
          // Read back as the app's when the log says it was (#340).
          app: post.from_app ?? false,
          colour: fold?.colour ?? deps.speakerColor(post.speaker, null, false),
          to: post.to ?? [],
          ts: post.ts,
          // The clock alone: a topic spans days, and the day is the divider
          // above (`placeDay`, #225).
          stamp: deps.shortTime(post.ts),
          content: post.content,
          past: true,
          mine: isMine(post.speaker, fold),
        }),
        fold,
      );
      deps.drawnIds.add(post.message_id);
    }

    // The permission prompts still waiting in this topic (#336). They are not
    // posts and are in no log, so they are put back after the log's lines.
    permissionEls.clear();
    for (const card of permissionCards.values()) {
      if (card.topic_id === deps.shownTopicId()) deps.roomEl.appendChild(permissionLine(card));
    }

    // Opened at the end, which is where the conversation continues. At once: a
    // topic opened is a place arrived at, not a line arriving (#307).
    deps.roomGliding = false;
    deps.scrollRoomToLatest("instant");
  }

  /** The prompts still waiting on the room, by id, in every topic. */
  const permissionCards = new Map<string, PermissionCard>();
  /** The cards drawn in the room on the glass, waiting or ended, by id. */
  const permissionEls = new Map<string, HTMLElement>();

  /** What an ended card says, by how it ended. */
  const PERMISSION_OUTCOMES: Record<PermissionResolved["outcome"], string> = {
    deny: "部屋から「拒否」を返しました",
    allow: "部屋から「一度だけ許可」を返しました",
    always: "部屋から「常に許可」を返しました",
    elsewhere: "端末で答えたか、待ちが終わりました",
    closed: "セッションが答えを待つのをやめました",
    timeout: "部屋での受付を終えました。端末から答えてください",
  };

  /** The buttons, in the order Claude Desktop's card puts them. */
  const PERMISSION_BUTTONS: { decision: "deny" | "always" | "allow"; label: string }[] = [
    { decision: "deny", label: "拒否" },
    { decision: "always", label: "常に許可" },
    { decision: "allow", label: "一度だけ許可" },
  ];

  /**
   * One permission card in the room (#336), modelled on Claude Desktop's: who
   * asks, for which tool — an MCP tool as its server and its own name — what the
   * input says, and 拒否 / 常に許可 / 一度だけ許可. 常に許可 is drawn only when the
   * request carries a rule to add, and the rule is shown beside it.
   */
  function permissionLine(card: PermissionCard): HTMLElement {
    const account = deps.accounts.find((one) => one.id === card.account_id);
    const name = account?.name.trim() || card.account_id;
    const article = document.createElement("article");
    article.className = "message permission";
    article.dataset.permission = card.id;
    article.style.setProperty("--speaker", deps.speakerColor(name, account?.hue ?? null, false));
    article.appendChild(avatar(name, card.account_id));

    const head = document.createElement("div");
    head.className = "meta";
    const speaker = document.createElement("span");
    speaker.className = "speaker";
    speaker.textContent = name;
    const kind = document.createElement("span");
    kind.className = "permission-kind";
    kind.textContent = card.agent_type ? `許可の確認・サブエージェント（${card.agent_type}）` : "許可の確認";
    const time = document.createElement("time");
    time.className = "ts";
    time.dateTime = card.at;
    time.textContent = deps.shortTime(card.at);
    time.title = deps.fullDateTime(card.at);
    head.append(speaker, kind, time);

    const body = document.createElement("div");
    body.className = "permission-body";
    const title = document.createElement("div");
    title.className = "permission-tool";
    title.title = card.tool_name;
    if (card.server) {
      const server = document.createElement("span");
      server.className = "permission-server";
      server.textContent = card.server;
      title.append(server, " / ");
    }
    const tool = document.createElement("strong");
    tool.textContent = card.tool;
    title.append(tool, " を使ってよいですか？");
    body.appendChild(title);

    if (card.fields.length) {
      const list = document.createElement("dl");
      list.className = "permission-input";
      for (const field of card.fields) {
        const term = document.createElement("dt");
        term.textContent = field.name;
        const value = document.createElement("dd");
        value.textContent = field.cut ? `${field.value}…（以下略）` : field.value;
        list.append(term, value);
      }
      body.appendChild(list);
    }
    if (card.more_fields > 0) {
      const more = document.createElement("div");
      more.className = "permission-note";
      more.textContent = `ほか ${card.more_fields} 項目は省きました`;
      body.appendChild(more);
    }
    if (card.can_always && card.always_rules.length) {
      const rules = document.createElement("div");
      rules.className = "permission-note";
      rules.textContent = `常に許可で足す規則：${card.always_rules.join("、")}`;
      body.appendChild(rules);
    }

    const actions = document.createElement("div");
    actions.className = "permission-actions";
    for (const button of PERMISSION_BUTTONS) {
      if (button.decision === "always" && !card.can_always) continue;
      const press = document.createElement("button");
      press.type = "button";
      press.className = `permission-${button.decision}`;
      press.textContent = button.label;
      press.addEventListener("click", (event) => void answerPermission(card, button.decision, article, event));
      actions.appendChild(press);
    }
    const state = document.createElement("div");
    state.className = "permission-state";
    state.textContent = "端末からも答えられます";
    body.append(actions, state);

    article.append(head, body);
    permissionEls.set(card.id, article);
    return article;
  }

  /** Turn a card into its ended form: no buttons, and how it ended. */
  function closePermissionLine(article: HTMLElement, text: string): void {
    article.classList.add("ended");
    article.querySelector(".permission-actions")?.remove();
    const state = article.querySelector(".permission-state");
    if (state) state.textContent = text;
  }

  /**
   * Send the person's press to the held hook. Only this screen can: the command
   * is the webview's, and the room reads no post as an answer (#336). What is
   * said back is what the room sent, not what the session did — the terminal
   * may have answered first.
   *
   * How the click reached the button goes with it, for the app's log only
   * (#346): the pointer's kind (empty for Enter or Space on a focused button),
   * the click count (0 for a key) and whether the browser made the event.
   */
  async function answerPermission(
    card: PermissionCard,
    decision: "deny" | "always" | "allow",
    article: HTMLElement,
    event: MouseEvent,
  ): Promise<void> {
    const buttons = article.querySelectorAll<HTMLButtonElement>(".permission-actions button");
    for (const button of buttons) button.disabled = true;
    const press = {
      pointer_type: "pointerType" in event ? String((event as PointerEvent).pointerType ?? "") : "",
      detail: event.detail,
      trusted: event.isTrusted,
    };
    let delivered: boolean;
    try {
      delivered = await invoke<boolean>("permission_answer", { id: card.id, decision, press });
    } catch (err) {
      for (const button of buttons) button.disabled = false;
      deps.status(`許可の答えを送れませんでした: ${err}`, "error");
      return;
    }
    if (!delivered) {
      permissionCards.delete(card.id);
      closePermissionLine(article, "もう答えられません。端末で答えたか、待ちが終わりました");
    }
  }

  /** A prompt arrived to be held for the room: draw it if its topic is shown. */
  function showPermission(card: PermissionCard): void {
    if (permissionCards.has(card.id)) return;
    permissionCards.set(card.id, card);
    if (card.topic_id !== deps.shownTopicId()) return;
    const atBottom = deps.roomAtBottom();
    const line = permissionLine(card);
    line.classList.add("arriving");
    deps.roomEl.appendChild(line);
    if (atBottom) deps.glideRoomToFoot("smooth");
    deps.syncScrollLatest();
  }

  /** A held prompt ended, however it did: its card stops being pressable. */
  function endPermission(resolved: PermissionResolved): void {
    permissionCards.delete(resolved.id);
    const article = permissionEls.get(resolved.id);
    if (!article) return;
    closePermissionLine(article, PERMISSION_OUTCOMES[resolved.outcome] ?? "受付を終えました");
  }

  /**
   * Put the posts a refusal handed back on the glass, and answer how many were
   * new. Oldest first, in the order the room put them in.
   *
   * The refusal is the only place these posts are reachable from. The room has no
   * read-out for the floor — `room_participants` / `room_port` / `room_join` /
   * `room_post` is the whole surface — and the watermark advances inside
   * `appendMessage`, that is, only for a line actually drawn. So a screen that
   * missed the delivering event had no second path to those posts: the watermark
   * stayed behind, the next post was refused for the same reason, and nothing the
   * person could do moved it. Only another participant speaking broke it, which
   * is not something the person could cause while refused (#108).
   *
   * Drawn as ordinary lines, with no mark saying they were caught up on. They are
   * posts of the room like any other, and "arrived while you were typing" is not
   * an attribute of a post.
   *
   * The watermark is then the newest of them, drawn here or drawn earlier: the
   * screen has seen every one either way. Setting it is what the already-drawn
   * case needs — nothing is appended there, so nothing else would move it, and
   * leaving it standing is the refusal loop this exists to break. Against the
   * watermark that was declared it always advances, since every missed post is by
   * definition ahead of that one. A post admitted after the refusal was computed
   * and drawn before this runs is the one thing it can fall behind, and that
   * costs the round trip the floor already prices (#47).
   */
  function drawMissed(missed: MissedPost[]): number {
    let drew = 0;
    for (const one of missed) {
      if (deps.drawnIds.has(one.message_id)) continue;
      appendMessage({
        // Drawn into the topic on the glass, which is the one the refused post was
        // written in (`send` checks that before calling this).
        topic_id: deps.shownTopicId(),
        message_id: one.message_id,
        speaker: one.speaker,
        full_name: one.full_name,
        hue: one.hue,
        account: one.account,
        from_app: one.from_app ?? false,
        content: one.content,
        to: one.to,
        ts: one.ts,
        // Never this screen's own. The floor holds a speaker's own posts back
        // from their own refusal: they were not delivered to their author, so
        // there was nothing there to have missed.
        own: false,
      });
      drew += 1;
    }

    const newest = missed[missed.length - 1];
    if (newest) deps.lastSeenId = newest.message_id;
    return drew;
  }

  /**
   * Read one post for who the room is now waiting on.
   *
   * Speaking clears first, then being addressed marks — in that order, so a
   * participant answering one question and being asked another in the same instant
   * ends up marked. Whoever spoke is no longer owed an answer whether or not the
   * post was the one they were asked for: they are audibly not stuck.
   *
   * A name nobody currently answers to is not marked. The room delivers a post
   * addressed to no one present just the same, and marking it would leave the word
   * armed for whoever takes that name next — a row saying 考え中 about a question
   * asked before it was even running.
   */
  function trackAddress(message: RoomMessage): void {
    // In the topic it was said in. A name asked in one topic is not being waited
    // on in another, where someone else — or the same account's other session —
    // answers to it (#141).
    let waiting = deps.awaiting.get(message.topic_id);
    if (!waiting) {
      waiting = new Set<string>();
      deps.awaiting.set(message.topic_id, waiting);
    }
    let moved = waiting.delete(message.speaker);
    const present = deps.rosters.get(message.topic_id) ?? [];
    // Each name on it, the same way: a post to several is a question to each of
    // them, and every one that has not answered is being waited on (#204).
    for (const to of message.to) {
      if (!waiting.has(to) && present.some((one) => one.name === to && !one.own)) {
        waiting.add(to);
        moved = true;
      }
    }
    if (moved && message.topic_id === deps.shownTopicId()) deps.renderPanel();
  }

  /**
   * Drop everyone the room is waiting on who is no longer in it.
   *
   * The roster is the authority on who is present, so this runs when it arrives. A
   * session that exits with a question outstanding leaves the room, and this is
   * what takes its mark with it.
   */
  function pruneAwaiting(topicId: string): void {
    const waiting = deps.awaiting.get(topicId);
    if (!waiting) return;
    const present = new Set(
      (deps.rosters.get(topicId) ?? []).filter((one) => !one.own).map((one) => one.name),
    );
    for (const name of waiting) {
      if (!present.has(name)) waiting.delete(name);
    }
  }

  /** The line a post's attachments are listed under (`withAttachments`). */
  const ATTACHMENT_HEAD = "添付:";

  /**
   * A post's words and the attachments it lists, read back out of its text
   * (#318): the `添付:` block `withAttachments` put at its end.
   *
   * Read as attachments only when every line of the block is a path inside this
   * room's attachments folder. A block naming anything else — an AI writing out
   * a path of its own, a post quoting one — is left as the text it is, so a path
   * the screen will not draw is not hidden either. This decides how the text is
   * drawn and nothing more: what may be loaded is decided again by the app, on
   * the resolved path (`room_attachment`).
   */
  function splitAttachments(content: string): { text: string; paths: string[] } {
    const asText = { text: content, paths: [] };
    const root = deps.attachmentsRoot;
    if (!root) return asText;
    const marker = `\n\n${ATTACHMENT_HEAD}\n`;
    const at = content.lastIndexOf(marker);
    let text: string;
    let block: string;
    if (at >= 0) {
      text = content.slice(0, at);
      block = content.slice(at + marker.length);
    } else if (content.startsWith(`${ATTACHMENT_HEAD}\n`)) {
      text = "";
      block = content.slice(ATTACHMENT_HEAD.length + 1);
    } else {
      return asText;
    }
    // What was written inside a ``` fence stays code (#348): a `添付:` block that
    // an unclosed fence runs over is part of the code, not attachments.
    const head = at >= 0 ? text.split("\n").length + 1 : 0;
    if (lineInFence(findCodeFences(content.split("\n")), head)) return asText;
    const paths = block.split("\n");
    const inside = (path: string) =>
      path.length > root.length + 1 &&
      path.startsWith(root) &&
      (path[root.length] === "\\" || path[root.length] === "/");
    if (!paths.every(inside)) return asText;
    return { text, paths };
  }

  /**
   * A post's words into its body (#431): read as Markdown — tables, lists,
   * emphasis, code blocks (#348) and mermaid diagrams — with every element built
   * by the screen. Nothing a post says is read as HTML.
   */
  function appendPostWords(body: HTMLElement, text: string): void {
    appendMarkdown(body, text, {
      prose: (words) => linkifyText(words),
      link: markdownLink,
      code: codeBlock,
    });

    // Windows MCP counts visible UI Automation word boxes before visiting the
    // editor and send button. Keep each static text run in its original reading
    // order and Markdown element, with a clipped copy for assistive technology.
    // Leave links in place while clipping their static text too. Keep button and
    // form control labels in their own elements.
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, (node) => {
      if (node instanceof Element) {
        return node.matches('button, input, select, textarea, summary, [tabindex], [contenteditable], [aria-hidden="true"]')
          ? NodeFilter.FILTER_REJECT
          : NodeFilter.FILTER_SKIP;
      }
      return NodeFilter.FILTER_ACCEPT;
    });
    const textRuns: Text[] = [];
    for (let node = walker.nextNode(); node; node = walker.nextNode()) textRuns.push(node as Text);
    for (const textRun of textRuns) {
      const visual = document.createElement("span");
      visual.setAttribute("aria-hidden", "true");
      const spoken = document.createElement("span");
      spoken.className = "post-readable-text";
      spoken.textContent = textRun.data;
      textRun.replaceWith(visual, spoken);
      visual.appendChild(textRun);
    }
  }

  /**
   * A Markdown link (#431): its label, opened as a URL when the address is one
   * the link rule of #349 accepts whole (http / https). Any other address leaves
   * the label as plain words.
   */
  function markdownLink(href: string, label: Node[]): Node[] {
    const pieces = linkPieces(href);
    const piece = pieces.length === 1 && pieces[0].kind === "url" && pieces[0].text === href ? pieces[0] : undefined;
    if (!piece) return label;
    // A bare URL is already a link from the label's own words.
    if (label.length === 1 && label[0] instanceof HTMLAnchorElement) return label;
    const link = document.createElement("a");
    link.className = "link";
    link.setAttribute("role", "link");
    link.title = href;
    link.append(...label);
    link.addEventListener("click", () => {
      if (!(window.getSelection()?.isCollapsed ?? true)) return;
      openLink(piece);
    });
    return [link];
  }

  function codeBlock(code: string, lang: string, info = ""): HTMLElement {
    // An html fence is one file-name line in the room; its code is read in the preview (#436).
    if (lang.toLowerCase() === "html") return deps.htmlPreview.fileBlock(code, info);
    const block = document.createElement("div");
    block.className = "code-block";
    if (lang) {
      const label = document.createElement("div");
      label.className = "code-lang";
      label.textContent = lang;
      block.appendChild(label);
    }
    const pre = document.createElement("pre");
    const inner = document.createElement("code");
    // Coloured as the input box colours it (#354): the fence's language, or a guess.
    inner.append(...highlightedCode(code, lang));
    pre.appendChild(inner);
    block.appendChild(pre);
    return block;
  }

  /**
   * Where a link may begin, and how far it runs before its end is trimmed
   * (#349): an `http://` / `https://` URL, a drive path (`C:\…`, `D:/…`) or a
   * UNC path (`\\server\share\…`). A relative path is not a link, and no other
   * scheme is.
   *
   * A link is one run of text: it stops at whitespace, at what a path cannot
   * hold, at a backquote (a path written as code), and at the Japanese
   * punctuation that closes a phrase — 、。「」『』【】 — which a name almost
   * never carries and a sentence written straight after a path always does.
   * Other full-width characters are part of a path. Not preceded by a letter or
   * a digit, so a drive path is not found inside a word, nor inside a URL or a
   * longer path.
   */
  const LINK_PATTERN =
    /(?<![\w])https?:\/\/[^\s<>"`、。「」『』【】]+|(?<![\w/\\])[a-z]:[\\/][^\s<>"|*?:`、。「」『』【】]*|(?<![\w/\\])\\\\[^\s\\/<>"|*?:`、。「」『』【】]+\\[^\s<>"|*?:`、。「」『』【】]+/gi;

  /** What a link does not end on: the punctuation after it in a sentence. */
  const LINK_TRAILING = new Set([...".,;:!?'\"*>。、，．：；！？〉》」』】"]);

  /** A closing bracket, by the opening one it is kept with. */
  const LINK_CLOSERS: Record<string, string> = { ")": "(", "）": "（", "]": "[", "］": "［", "}": "{", "｝": "｛" };

  /**
   * A link with the punctuation after it taken off (#349). A closing bracket is
   * taken off only when it closes nothing in the link, so `…/Foo_(bar)` keeps its
   * own and `（C:\資料）` gives back the sentence's.
   */
  function trimLinkEnd(candidate: string): string {
    let end = candidate.length;
    while (end > 0) {
      const last = candidate[end - 1];
      if (LINK_TRAILING.has(last)) {
        end--;
        continue;
      }
      const open = LINK_CLOSERS[last];
      if (!open) break;
      const kept = candidate.slice(0, end);
      if (kept.split(open).length >= kept.split(last).length) break;
      end--;
    }
    return candidate.slice(0, end);
  }

  /**
   * The words of a post as text and links, in order (#349): the pure half of
   * `linkifyText`. Joined back, the pieces are `text` exactly.
   */
  function linkPieces(text: string): TextPiece[] {
    const pieces: TextPiece[] = [];
    const say = (kind: TextPiece["kind"], part: string) => {
      if (!part) return;
      const last = pieces[pieces.length - 1];
      if (kind === "text" && last?.kind === "text") last.text += part;
      else pieces.push({ kind, text: part });
    };
    let from = 0;
    for (const match of text.matchAll(LINK_PATTERN)) {
      const at = match.index ?? 0;
      const link = trimLinkEnd(match[0]);
      const url = /^https?:\/\//i.test(link);
      // Trimmed down to no host, or to a server with no share, it is no link;
      // nor is `\\.\`, which names a device and not a server.
      const whole = url ? /^https?:\/\/[^/\\]/i.test(link) : !link.startsWith("\\\\") || /^\\\\(?!\.\\)[^\\]+\\[^\\]/.test(link);
      if (!whole) continue;
      say("text", text.slice(from, at));
      say(url ? "url" : "path", link);
      from = at + link.length;
    }
    say("text", text.slice(from));
    return pieces;
  }

  /**
   * A post's words as the nodes the screen draws them with (#349): text as text,
   * and every URL and Windows path as a link a press opens — a URL in the
   * default browser, a folder in Explorer, a file in the app its type opens
   * with. What is opened and how is decided again by the app
   * (`open_post_url` / `open_post_path`), since a post is anyone's writing.
   *
   * Takes any run of plain text and gives it back unchanged but for the links,
   * newlines and spaces included, so a part of a post drawn some other way can
   * pass the rest of its words through here.
   *
   * A press is a click and nothing else. A drag that selects text across a link
   * opens nothing.
   */
  function linkifyText(text: string): Node[] {
    return linkPieces(text).map((piece) => {
      if (piece.kind === "text") return document.createTextNode(piece.text);
      const link = document.createElement("a");
      link.className = "link";
      link.setAttribute("role", "link");
      link.textContent = piece.text;
      link.addEventListener("click", () => {
        if (!(window.getSelection()?.isCollapsed ?? true)) return;
        openLink(piece);
      });
      return link;
    });
  }

  /** Open what a link names, saying on the status line when it cannot (#349). */
  async function openLink(piece: TextPiece): Promise<void> {
    try {
      if (piece.kind === "url") await invoke("open_post_url", { url: piece.text });
      else await invoke("open_post_path", { path: piece.text });
    } catch (err) {
      deps.status(String(err), "error");
    }
  }

  /** What a file is called after its last dot, lower-cased; empty without one. */
  function extensionOf(name: string): string {
    const at = name.lastIndexOf(".");
    return at > 0 ? name.slice(at + 1).toLowerCase() : "";
  }

  /** The pictures drawn as pictures (#318), by extension, with their types. */
  const IMAGE_TYPES: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
  };

  /** The type a file is drawn as a picture under, or undefined for any other. */
  function imageType(name: string): string | undefined {
    return IMAGE_TYPES[extensionOf(name)];
  }

  /**
   * The pictures of the attachments on the glass, as object URLs by path (#318).
   * Read once per path rather than once per line: a post drawn again draws the
   * same picture. A path that would not read is kept as null, and its line shows
   * the file's chip. Emptied when a topic is drawn afresh (`drawTopic`), which
   * takes every line that was showing one.
   */
  const attachmentUrls = new Map<string, Promise<string | null>>();

  function attachmentUrl(path: string, type: string): Promise<string | null> {
    let url = attachmentUrls.get(path);
    if (!url) {
      url = invoke<ArrayBuffer>("room_attachment", { path }).then(
        (bytes) => URL.createObjectURL(new Blob([bytes], { type })),
        () => null,
      );
      attachmentUrls.set(path, url);
    }
    return url;
  }

  function forgetAttachmentUrls(): void {
    for (const url of attachmentUrls.values()) {
      void url.then((one) => {
        if (one) URL.revokeObjectURL(one);
      });
    }
    attachmentUrls.clear();
  }

  /** A post's attachments, in the order its block lists them (#318). */
  function postAttachments(paths: string[]): HTMLElement {
    const list = document.createElement("div");
    list.className = "attachments";
    for (const path of paths) {
      const name = deps.baseName(path);
      const type = imageType(name);
      list.appendChild(type ? postImage(path, name, type) : fileChip(name));
    }
    return list;
  }

  /** A file that is not drawn as a picture: its name, and its type beside it. */
  function fileChip(name: string): HTMLElement {
    const chip = document.createElement("span");
    chip.className = "file";
    chip.title = name;
    const label = document.createElement("span");
    label.className = "name";
    label.textContent = name;
    const kind = document.createElement("span");
    kind.className = "kind";
    const ext = extensionOf(name);
    kind.textContent = ext ? ext.toUpperCase() : "ファイル";
    chip.append(label, kind);
    return chip;
  }

  /**
   * An attached picture, small, opened larger by a press (#318). The tile has
   * its full size before the picture arrives, so a line does not grow under the
   * reader when it does. A picture that will not read or will not decode gives
   * way to the file's chip, without a word.
   */
  function postImage(path: string, name: string, type: string): HTMLElement {
    const thumb = document.createElement("button");
    thumb.type = "button";
    thumb.className = "thumb";
    thumb.title = name;
    thumb.setAttribute("aria-label", `${name} を大きく見る`);
    const image = document.createElement("img");
    image.alt = "";
    thumb.appendChild(image);
    const giveWay = () => thumb.replaceWith(fileChip(name));
    void attachmentUrl(path, type).then((url) => {
      if (!url) {
        giveWay();
        return;
      }
      image.addEventListener("error", giveWay, { once: true });
      image.src = url;
    });
    thumb.addEventListener("click", () => {
      if (image.src) openViewer(image.src, name);
    });
    return thumb;
  }

  /** Open a picture larger, over the dimmed window (#318). */
  function openViewer(url: string, name: string): void {
    deps.viewerImageEl.src = url;
    deps.viewerImageEl.alt = name;
    deps.viewerEl.setAttribute("aria-label", name);
    if (!deps.viewerEl.open) deps.viewerEl.showModal();
  }

  return { appendMessage, trackAddress, showPermission, endPermission, loadAvatarImages, drawTopic, initialOf, avatarImages, drawAvatar, setAvatarImage, avatar, paintMine, pruneAwaiting, drawMissed, ATTACHMENT_HEAD, imageType, openViewer };
}
