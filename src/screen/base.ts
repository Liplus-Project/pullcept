import type { IconName, IconShape } from "../contracts";

/**
 * The hues a participant can declare.
 *
 * Hue only. Lightness and chroma use the theme's shared display tokens, so
 * declared colours share one weight on the page. The vivid pair (#397) is
 * independent of accent; the hue-only contract stays (#43). Offering a full
 * colour picker would ask for two values and silently discard them.
 *
 * A short list rather than a continuous dial: what this has to buy is that any
 * two participants can be told apart, and sixteen positions spread round the
 * wheel buy it without asking anyone to judge degrees.
 */
export const HUES: { label: string; hue: number }[] = [
  { label: "紅", hue: 7.5 },
  { label: "赤", hue: 25 },
  { label: "朱", hue: 40 },
  { label: "橙", hue: 55 },
  { label: "山吹", hue: 75 },
  { label: "黄", hue: 95 },
  { label: "黄緑", hue: 120 },
  { label: "緑", hue: 145 },
  { label: "ミント", hue: 170 },
  { label: "青緑", hue: 195 },
  { label: "水色", hue: 222.5 },
  { label: "青", hue: 250 },
  { label: "藍", hue: 275 },
  { label: "紫", hue: 300 },
  { label: "藤", hue: 325 },
  { label: "桃", hue: 350 },
];

/**
 * Hue of `--accent`, and the arc the other participants are drawn from.
 *
 * `#3a6ea5` measured in oklch. The accent is this screen's own colour, so the
 * derived hues start a gap past it and stop a gap short of it: a participant
 * whose name happened to land on the accent would look like oneself.
 */
const ACCENT_HUE = 251.5;
const RESERVED_ARC = 25;
const DERIVED_ARC = 360 - RESERVED_ARC * 2;

const ICONS: Record<IconName, IconShape[]> = {
  // A panel seen edge-on, the divider on the side it folds (#118). The right
  // one is this drawing turned over in src/styles/titlebar.css.
  panel: [
    ["rect", { x: "1.7", y: "2.7", width: "12.6", height: "10.6", rx: "2.4" }],
    ["line", { x1: "6.2", y1: "2.7", x2: "6.2", y2: "13.3" }],
  ],
  // A window with a prompt in it: the pane 端末 opens.
  terminal: [
    ["rect", { x: "1.7", y: "2.7", width: "12.6", height: "10.6", rx: "2.4" }],
    ["polyline", { points: "4.6 6.3 6.6 8 4.6 9.7" }],
    ["line", { x1: "8.2", y1: "10.2", x2: "11.4", y2: "10.2" }],
  ],
  // A gear: the app's own settings (#234, the shape of the 2026-10-02 mockup).
  // Tabler Icons `settings` (MIT, NOTICE.txt), redrawn from its 24-unit grid
  // onto this one. #221 drew sliders here to keep it apart from the row's ⚙,
  // which #224 has since moved into the row's menu.
  settings: [
    [
      "path",
      {
        d:
          "M6.883 2.878c.284 -1.171 1.949 -1.171 2.233 0a1.149 1.149 0 0 0 1.715 .711" +
          "c1.029 -.627 2.207 .551 1.58 1.58a1.149 1.149 0 0 0 .71 1.715" +
          "c1.171 .284 1.171 1.949 0 2.233a1.149 1.149 0 0 0 -.711 1.715" +
          "c.627 1.029 -.551 2.207 -1.58 1.58a1.149 1.149 0 0 0 -1.715 .71" +
          "c-.284 1.171 -1.949 1.171 -2.233 0a1.149 1.149 0 0 0 -1.715 -.711" +
          "c-1.029 .627 -2.207 -.551 -1.58 -1.58a1.149 1.149 0 0 0 -.71 -1.715" +
          "c-1.171 -.284 -1.171 -1.949 0 -2.233a1.149 1.149 0 0 0 .711 -1.715" +
          "c-.627 -1.029 .551 -2.207 1.58 -1.58c.667 .405 1.531 .047 1.715 -.71",
      },
    ],
    ["path", { d: "M6 8a2 2 0 1 0 4 0a2 2 0 0 0 -4 0" }],
  ],
  plus: [
    ["line", { x1: "8", y1: "3", x2: "8", y2: "13" }],
    ["line", { x1: "3", y1: "8", x2: "13", y2: "8" }],
  ],
  copy: [
    ["rect", { x: "5.5", y: "5.5", width: "8", height: "8", rx: "1.6" }],
    ["path", { d: "M10.5 5.5V4.1A1.6 1.6 0 0 0 8.9 2.5H4.1A1.6 1.6 0 0 0 2.5 4.1v4.8a1.6 1.6 0 0 0 1.6 1.6h1.4" }],
  ],
  // Folding the pane, closing an ended tab and deleting a topic. Which of
  // those it is, is the colour and the label: the one that cannot be taken
  // back carries `--danger`. Ending a session has its own drawing, `stop`.
  close: [
    ["line", { x1: "4", y1: "4", x2: "12", y2: "12" }],
    ["line", { x1: "12", y1: "4", x2: "4", y2: "12" }],
  ],
  // 送信 (#234): a paper plane, Tabler Icons `send` (MIT, NOTICE.txt) on this grid.
  send: [
    ["path", { d: "M6.667 9.333l7.333 -7.333" }],
    [
      "path",
      { d: "M14 2l-4.333 12a.367 .367 0 0 1 -.667 0l-2.333 -4.667l-4.667 -2.333a.367 .367 0 0 1 0 -.667l12 -4.333" },
    ],
  ],
  // 宛先: the `@` the button types (#222).
  at: [
    ["circle", { cx: "8", cy: "8", r: "2.6" }],
    ["path", { d: "M10.6 5.4v3.3a2 2 0 0 0 4 0V8a6.6 6.6 0 1 0-2.6 5.25" }],
  ],
  // 添付 (#223): a paper clip, slanted (#234). Tabler Icons `paperclip` (MIT,
  // NOTICE.txt) on this grid.
  attach: [
    [
      "path",
      { d: "M10 4.667l-4.333 4.333a1 1 0 0 0 2 2l4.333 -4.333a2 2 0 0 0 -4 -4l-4.333 4.333a3 3 0 0 0 6 6l4.333 -4.333" },
    ],
  ],
  start: [["path", { d: "M5 3.2v9.6L12.6 8Z" }]],
  // 終了 (#239): a square, the other end of the triangle above, on the row in
  // the column 開始 stands in and in the row's menu. Tabler Icons `player-stop`
  // (MIT, NOTICE.txt) on this grid.
  stop: [
    [
      "path",
      {
        d:
          "M3.333 4.667a1.333 1.333 0 0 1 1.333 -1.333h6.667a1.333 1.333 0 0 1 1.333 1.333" +
          "v6.667a1.333 1.333 0 0 1 -1.333 1.333h-6.667a1.333 1.333 0 0 1 -1.333 -1.333Z",
      },
    ],
  ],
  // A gear: 編集 in the row's menu, the window an account's settings are made in.
  edit: [
    [
      "path",
      {
        d:
          "M7.08 1.46L8.92 1.46L9.17 3.14L10.61 3.74L11.97 2.73L13.27 4.03L12.26 5.39" +
          "L12.86 6.83L14.54 7.08L14.54 8.92L12.86 9.17L12.26 10.61L13.27 11.97" +
          "L11.97 13.27L10.61 12.26L9.17 12.86L8.92 14.54L7.08 14.54L6.83 12.86" +
          "L5.39 12.26L4.03 13.27L2.73 11.97L3.74 10.61L3.14 9.17L1.46 8.92" +
          "L1.46 7.08L3.14 6.83L3.74 5.39L2.73 4.03L4.03 2.73L5.39 3.74L6.83 3.14Z",
      },
    ],
    ["circle", { cx: "8", cy: "8", r: "2.1" }],
  ],
  // Three dots: the row's menu, the way into what right-click opens (#224).
  // Drawn as stroked rings small enough to read as dots, so it is stroked like
  // every other drawing here and takes the button's colour the same way.
  more: [
    ["circle", { cx: "3.5", cy: "8", r: "0.55" }],
    ["circle", { cx: "8", cy: "8", r: "0.55" }],
    ["circle", { cx: "12.5", cy: "8", r: "0.55" }],
  ],
  // 最新の発言へ (#243): a chevron pointing down, to the room's foot. Tabler
  // Icons `chevron-down` (MIT, NOTICE.txt) on this grid.
  "chevron-down": [["path", { d: "M4 6l4 4l4 -4" }]],
  // The window's own three (#252), in the shapes the standard frame drew them:
  // a bar, a square, and two squares with the one in front at the lower left.
  // 閉じる is `close` above, the same ✕ as every other one on the screen.
  // `restore` is not `copy` turned over by accident: the frame puts its front
  // square at the lower left, `copy` puts it at the lower right.
  minimize: [["line", { x1: "3.5", y1: "8", x2: "12.5", y2: "8" }]],
  maximize: [["rect", { x: "3.5", y: "3.5", width: "9", height: "9", rx: "1.2" }]],
  restore: [
    ["rect", { x: "3", y: "5.5", width: "7.5", height: "7.5", rx: "1.2" }],
    ["path", { d: "M5.5 5.5V4.2A1.2 1.2 0 0 1 6.7 3h5.1A1.2 1.2 0 0 1 13 4.2v5.1a1.2 1.2 0 0 1 -1.2 1.2H10.5" }],
  ],
};

const SVG_NS = "http://www.w3.org/2000/svg";

/** One drawing, ready to go into a button. Hidden from the reader: the button's label names it. */
export function icon(name: IconName): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("class", "icon");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  for (const [tag, attrs] of ICONS[name]) {
    const shape = document.createElementNS(SVG_NS, tag);
    for (const [key, value] of Object.entries(attrs)) shape.setAttribute(key, value);
    svg.appendChild(shape);
  }
  return svg;
}

/** Put its drawing into every button index.html names one for. */
export function fillIcons(): void {
  for (const button of document.querySelectorAll<HTMLElement>("[data-icon]")) {
    const name = button.dataset.icon as IconName;
    if (name in ICONS) button.replaceChildren(icon(name));
  }
}

/**
 * Fill a hue picker, with "not declared" first.
 *
 * Both pickers are filled from the one list, because a person and a session
 * declare a colour from the same set. Not declaring is an option rather than an
 * omission: a participant who chose no colour still joins, and the room derives
 * one for them from their name.
 */
export function fillHues(select: HTMLSelectElement, saved: string | null): void {
  const none = document.createElement("option");
  none.value = "";
  none.textContent = "既定（名前から）";
  select.appendChild(none);

  for (const { label, hue } of HUES) {
    const option = document.createElement("option");
    option.value = String(hue);
    option.textContent = label;
    select.appendChild(option);
  }
  select.value = saved !== null && HUES.some(({ hue }) => String(hue) === saved) ? saved : "";
}

/** The hue a picker currently declares, or null for "not declared". */
export function declaredHue(select: HTMLSelectElement): number | null {
  return select.value === "" ? null : Number(select.value);
}

/**
 * The hue a participant's name lands on.
 *
 * From the name, so the same participant is the same colour every time they
 * speak and in the roster beside their name. Not from arrival order: a
 * participant who reconnects would come back a different colour, and the
 * colour would then say when they joined rather than who they are.
 */
function hueFor(name: string): number {
  // FNV-1a. Any stable spread would do; this one is four lines.
  let hash = 2166136261;
  for (let i = 0; i < name.length; i += 1) {
    hash ^= name.charCodeAt(i);
    hash = Math.imul(hash, 16777619);
  }
  return (ACCENT_HUE + RESERVED_ARC + (Math.abs(hash) % DERIVED_ARC)) % 360;
}

/**
 * The colour a participant is drawn in.
 *
 * Declared and derived colours use the theme's shared display lightness and
 * chroma; only hue turns. Undeclared oneself keeps accent. One ladder:
 *
 *   declared > oneself's accent > derived from the name
 *
 * A declaration outranks both. Derivation is stable but not choosable, and the
 * three names actually in use landed inside a 75° band — at the accent's low
 * chroma that is not a difference anyone can read, so colour stopped doing the
 * one job it was added for (#40).
 *
 * That the accent is below the declaration is the deliberate half. Oneself
 * keeps it while nothing is declared, so nothing changes for a participant who
 * declares nothing; declaring a colour takes it, because a declared colour that
 * showed to everyone except the person who declared it is not the colour they
 * declared. What then says which participant is oneself is the roster's
 * 「（あなた）」 and the name on the line — colour was the faster of the three
 * carriers, never the only one, and it is the one that is now chosen rather
 * than assigned.
 */
export function speakerColor(name: string, hue: number | null, own: boolean): string {
  if (hue !== null) return `oklch(var(--speaker-l) var(--speaker-c) ${hue.toFixed(1)})`;
  if (own) return "var(--accent)";
  return `oklch(var(--speaker-l) var(--speaker-c) ${hueFor(name).toFixed(1)})`;
}

export function shortTime(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  return at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** The local calendar day a stamp falls on, as a key two stamps compare by. */
export function dayKey(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  return `${at.getFullYear()}-${at.getMonth() + 1}-${at.getDate()}`;
}

/**
 * A day, as short as it can be said and still place it (#225): 今日, else the
 * month and the day, and the year only when it is not this one.
 *
 * One wording for the two places a day is shown — the room's day dividers and
 * the topic list — so a topic and the lines in it name their day alike. The
 * clock is not part of it: a line carries its own, and a topic's is on its
 * tooltip.
 */
export function dayLabel(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const now = new Date();
  if (dayKey(iso) === dayKey(now.toISOString())) return "今日";
  const md = `${at.getMonth() + 1}月${at.getDate()}日`;
  return at.getFullYear() === now.getFullYear() ? md : `${at.getFullYear()}年${md}`;
}

/** The day and the clock in full, for a tooltip under a short label. */
export function fullDateTime(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const day = at.toLocaleDateString([], { year: "numeric", month: "2-digit", day: "2-digit" });
  return `${day} ${at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`;
}
