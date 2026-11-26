import type { Member, RowWord } from "../contracts";

export interface Dependencies {
  roomEl: HTMLElement;
  scrollLatestEl: HTMLButtonElement;
  roomBusyEl: HTMLElement;
  roomGliding: boolean;
  speakerColor: (name: string, hue: number | null, own: boolean) => string;
  memberName: (row: Member) => string;
  pulsePhase: () => string;
  rowActivity: (row: Member) => RowWord;
}

export function createRoom(deps: Dependencies) {

  /**
   * Whether the room is at its foot: within 40px of the bottom. One judgment for
   * two things — an arriving line follows the room down only from here, and the
   * button back to the newest line is shown only away from here (#243).
   */
  function roomAtBottom(): boolean {
    return deps.roomGliding || deps.roomEl.scrollHeight - deps.roomEl.scrollTop - deps.roomEl.clientHeight < 40;
  }

  /** Send the room to its foot: gliding, or at once for a topic just opened. */
  function glideRoomToFoot(behavior: "smooth" | "instant"): void {
    const distance = deps.roomEl.scrollHeight - deps.roomEl.scrollTop - deps.roomEl.clientHeight;
    // Already there: no scroll happens, so nothing would ever end the glide.
    if (distance < 2) return;
    // `smooth` here is "whatever `#room` says": the stylesheet glides only where
    // motion is not reduced, and moves at once where it is.
    deps.roomGliding = behavior === "smooth";
    deps.roomEl.scrollTo({ top: deps.roomEl.scrollHeight, behavior: behavior === "smooth" ? "auto" : "instant" });
  }

  /**
   * End the glide once it has landed, or once the person took the scroll over —
   * a wheel or a drag during a glide cancels it, and its `scrollend` comes where
   * they stopped.
   */
  function settleGlide(event: Event): void {
    if (!deps.roomGliding) return;
    const distance = deps.roomEl.scrollHeight - deps.roomEl.scrollTop - deps.roomEl.clientHeight;
    if (event.type === "scrollend" || distance < 2) deps.roomGliding = false;
  }

  /**
   * Show 最新の発言へ while the room is being read back, and hide it at the foot
   * (#243). Run wherever the distance to the foot can change: a scroll, the
   * room's own size, a line drawn, a fold opened, the conversation's text size.
   */
  function syncScrollLatest(): void {
    deps.scrollLatestEl.hidden = roomAtBottom();
  }

  /**
   * To the room's foot, where the conversation continues (#243). Gliding when
   * 最新の発言へ is pressed, at once when a topic is opened (#307).
   */
  function scrollRoomToLatest(behavior: "smooth" | "instant"): void {
    glideRoomToFoot(behavior);
    syncScrollLatest();
  }

  /** What the line under the room says now, so an unchanged one is not redrawn. */
  let roomBusyKey = "";

  /**
   * The line under the room naming who is 考え中… or 出力中 now (#307):
   * 「Claude Lin が考え中…」, and the next one after 、.
   *
   * The same word, from the same reading, as each row's badge (`rowActivity`) —
   * this line adds no signal of its own and no word the badge does not say. It
   * keeps its row whether or not it says anything, so the room above it does not
   * grow and shrink with every burst of output. Redrawn only when what it says
   * changes, so its dots keep moving through the panel's redraws.
   */
  function renderRoomBusy(rows: Member[]): void {
    const busy = rows.flatMap((row) => {
      const said = deps.rowActivity(row);
      if (!said.kind) return [];
      const name = deps.memberName(row);
      return [{ name, word: said.line, colour: deps.speakerColor(name, row.participant!.hue, false) }];
    });
    const key = busy.map((one) => `${one.name}\u0000${one.word}\u0000${one.colour}`).join("\u0001");
    if (key === roomBusyKey) return;
    roomBusyKey = key;
    deps.roomBusyEl.replaceChildren();
    if (!busy.length) return;

    const dots = document.createElement("span");
    dots.className = "busy-dots";
    dots.setAttribute("aria-hidden", "true");
    for (let i = 0; i < 3; i += 1) dots.appendChild(document.createElement("i"));
    dots.style.setProperty("--pulse-phase", deps.pulsePhase());
    deps.roomBusyEl.appendChild(dots);

    const text = document.createElement("span");
    text.className = "busy-text";
    busy.forEach((one, index) => {
      if (index > 0) text.append("、");
      const who = document.createElement("span");
      who.className = "who";
      who.style.setProperty("--speaker", one.colour);
      who.textContent = one.name;
      text.append(who, `が${one.word}`);
    });
    deps.roomBusyEl.appendChild(text);
  }

  return { settleGlide, syncScrollLatest, scrollRoomToLatest, renderRoomBusy, roomAtBottom, glideRoomToFoot };
}
