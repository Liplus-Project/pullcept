import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Topic, TopicRef, Participant, IconName } from "../contracts";

export interface Dependencies {
  icon: (name: IconName) => SVGSVGElement;
  roomTitleEl: HTMLElement;
  roomTitleNameEl: HTMLElement;
  roomTitleCountEl: HTMLElement;
  topics: Topic[];
  currentTopic: TopicRef | null;
  shownRoster: () => Participant[];
  status: (text: string, kind?: "info" | "error") => void;
  openTopicMenu: (topic: Topic, at: { x: number; y: number; }, rename: () => void) => void;
  renameField: (topic: Topic, done: () => void) => HTMLInputElement;
  topicName: (topic: Topic) => string;
}

export function createTitlebar(deps: Dependencies) {

  /**
   * The room's heading on the title bar (#225): the topic on the glass, and how
   * many are in its room.
   *
   * The name is the one its row carries (`topicName`), so the two never say it
   * differently. A topic the index does not carry yet has no row and no title —
   * the moment between a launch or 新規 and the first post (#115) — and is called
   * what it is, 新しいトピック.
   *
   * The count is the room's roster for that topic: who is connected to it now.
   * An account that is not running is not in it, and neither is a local MCP
   * server, which takes no seat (#193) — the panel lists both, and this counts
   * only who is here.
   */
  function renderRoomTitle(): void {
    const current = deps.currentTopic;
    const topic = current ? deps.topics.find((one) => one.topic_id === current.topic_id) : undefined;
    const name = current === null ? "" : topic ? deps.topicName(topic) : "新しいトピック";
    deps.roomTitleNameEl.textContent = name;
    deps.roomTitleNameEl.title = topic ? `${name}（クリックで名前を変える）` : name;
    deps.roomTitleNameEl.classList.toggle("unnamed", topic !== undefined && !topic.title);
    deps.roomTitleCountEl.textContent = current === null ? "" : `${deps.shownRoster().length} 人`;
  }

  /**
   * Rename the open topic from the title bar's heading (#469).
   *
   * A topic the index does not carry yet — 新しいトピック — has no row to rename,
   * so the heading offers nothing then. The field stands in the name's place and
   * the count stays beside it.
   */
  function beginTitleRename(): void {
    const current = deps.currentTopic;
    const topic = current ? deps.topics.find((one) => one.topic_id === current.topic_id) : undefined;
    if (!topic || deps.roomTitleEl.querySelector("input.rename")) return;
    const field = deps.renameField(topic, () => {
      field.remove();
      deps.roomTitleNameEl.hidden = false;
      renderRoomTitle();
    });
    deps.roomTitleNameEl.hidden = true;
    deps.roomTitleNameEl.after(field);
    // As wide as what is written in it (#479), measured rather than left to
    // `field-sizing`, which fell about two characters short on the device and
    // scrolled the start of the name out of the field (#484). The extra is the
    // field's own padding and border, and room for the caret.
    const measure = document.createElement("canvas").getContext("2d");
    const fit = (): void => {
      if (!measure) return;
      const style = getComputedStyle(field);
      measure.font = style.font;
      const chrome =
        parseFloat(style.paddingLeft) + parseFloat(style.paddingRight) +
        parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth);
      field.style.width = `${Math.ceil(measure.measureText(field.value).width + chrome) + 8}px`;
    };
    field.addEventListener("input", fit);
    fit();
    field.focus();
    field.select();
  }

  /**
   * The window's three buttons on the title bar (#252), which stand where the
   * standard frame's stood until the frame was taken off.
   *
   * 閉じる is `close`, not `destroy`. `close` raises the same `CloseRequested`
   * the frame's ✕ raised (tauri-runtime-wry 2.11.4, `on_close_requested`;
   * `@tauri-apps/api/window` says so of `close`), so it lands in
   * `onQuitRequested` above and the question about running sessions is asked
   * exactly as before. `destroy` would take the window without asking.
   *
   * 最大化 is one button with two names. Its name and drawing are read back from
   * the window whenever the window's size changes, rather than flipped on the
   * click: a window is also maximized by a double click on the bar, by Win+↑ and
   * by dragging it to the top of the screen, and none of those passes through
   * this button.
   *
   * A call the window refuses is said on the status line. Nothing else would show
   * it: the click is the only witness, and it already happened.
   */
  async function wireWindowControls(): Promise<void> {
    const appWindow = getCurrentWindow();
    const minimizeEl = document.getElementById("window-minimize") as HTMLButtonElement;
    const maximizeEl = document.getElementById("window-maximize") as HTMLButtonElement;
    const closeEl = document.getElementById("window-close") as HTMLButtonElement;

    const refused = (err: unknown) => deps.status(`窓を操作できませんでした: ${err}`, "error");
    minimizeEl.addEventListener("click", () => void appWindow.minimize().catch(refused));
    maximizeEl.addEventListener("click", () => void appWindow.toggleMaximize().catch(refused));
    closeEl.addEventListener("click", () => void appWindow.close().catch(refused));

    // The topic's name renames on a click (#469, #479): a press released where
    // it was pressed opens the field, and a press that moves grabs the window
    // like the bar around it. The name carries `data-tauri-drag-region="false"`,
    // which takes it out of Tauri's drag script — that script starts the drag on
    // the press itself, before it can be told from a click — so the drag is
    // started here once the pointer has moved a few pixels with the button held.
    deps.roomTitleNameEl.addEventListener("mousedown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      const fromX = event.clientX;
      const fromY = event.clientY;
      const off = (): void => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };
      const onMove = (move: MouseEvent): void => {
        if (Math.abs(move.clientX - fromX) + Math.abs(move.clientY - fromY) < 4) return;
        off();
        void appWindow.startDragging().catch(refused);
      };
      const onUp = (): void => {
        off();
        beginTitleRename();
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    });
    deps.roomTitleNameEl.addEventListener("contextmenu", (event) => {
      const current = deps.currentTopic;
      const topic = current ? deps.topics.find((one) => one.topic_id === current.topic_id) : undefined;
      if (!topic) return;
      event.preventDefault();
      deps.openTopicMenu(topic, { x: event.clientX, y: event.clientY }, () => beginTitleRename());
    });

    const showMaximized = async () => {
      const maximized = await appWindow.isMaximized();
      const label = maximized ? "元に戻す" : "最大化";
      maximizeEl.setAttribute("aria-label", label);
      maximizeEl.title = label;
      maximizeEl.replaceChildren(deps.icon(maximized ? "restore" : "maximize"));
    };
    // Caught here rather than left to the caller: this runs early in start-up,
    // and a throw would stop everything after it for the sake of one tooltip.
    // The three buttons above are already wired by now and work regardless.
    try {
      await appWindow.onResized(() => void showMaximized().catch(refused));
      await showMaximized();
    } catch (err) {
      refused(err);
    }
  }

  return { wireWindowControls, renderRoomTitle };
}
