// Drag to resize the two side panels (#530): トピック on the left, アカウント on
// the right. Each gets a thin grip on its inner edge; dragging it sets the
// panel's width, kept between MIN and MAX. The width is remembered in this
// window's localStorage — a convenience only: when it cannot be read the panel
// keeps its stylesheet width (16.5rem), and a double-click on the grip goes
// back to that.

const MIN_REM = 12;
const MAX_REM = 32;

function rem(): number {
  return parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
}

function read(key: string): number | null {
  try {
    const value = Number(localStorage.getItem(key));
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

function write(key: string, value: number | null): void {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, String(value));
  } catch {
    // Not remembered; the width still applies for this run.
  }
}

/**
 * Put a grip on `panel`'s inner edge. `side` is where the panel sits: a left
 * panel grows when its grip moves right, a right panel when it moves left.
 */
export function resizableSidebar(panel: HTMLElement, side: "left" | "right", label: string): void {
  const key = `pullcept.sidebar.${panel.id}`;
  const clamp = (px: number) => Math.min(MAX_REM * rem(), Math.max(MIN_REM * rem(), px));
  const apply = (remWidth: number | null) => {
    panel.style.width = remWidth === null ? "" : `${remWidth}rem`;
  };
  apply(read(key));

  const grip = document.createElement("div");
  grip.className = `sidebar-grip sidebar-grip-${side}`;
  grip.setAttribute("role", "separator");
  grip.setAttribute("aria-orientation", "vertical");
  grip.setAttribute("aria-label", `${label}の幅`);
  grip.title = `${label}の幅（ドラッグで変更、ダブルクリックで元に戻す）`;
  if (side === "left") panel.after(grip);
  else panel.before(grip);

  grip.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const startWidth = panel.getBoundingClientRect().width;
    document.body.classList.add("sidebar-resizing");
    const move = (e: PointerEvent) => {
      const delta = side === "left" ? e.clientX - startX : startX - e.clientX;
      apply(clamp(startWidth + delta) / rem());
    };
    const end = () => {
      grip.removeEventListener("pointermove", move);
      grip.removeEventListener("pointerup", end);
      grip.removeEventListener("pointercancel", end);
      document.body.classList.remove("sidebar-resizing");
      write(key, panel.getBoundingClientRect().width / rem());
    };
    grip.addEventListener("pointermove", move);
    grip.addEventListener("pointerup", end);
    grip.addEventListener("pointercancel", end);
  });
  grip.addEventListener("dblclick", () => {
    apply(null);
    write(key, null);
  });
}
