// Patch @xterm/addon-webgl so block elements are painted on whole-pixel edges.
//
// Runs as the `postinstall` script, so every `npm install` / `npm ci` applies it
// to the freshly installed package. See docs/2-screen.md, 診断面 (#278).
//
// The addon draws a block element made of several rectangles (`▛`, `▜`, ...)
// with one `fillRect` per rectangle, at multiples of cell width / 8. When the
// cell is an odd number of device pixels wide, the edge two rectangles share
// falls in the middle of a pixel; each side paints it half transparent and the
// seam shows as a thin line. The replacement rounds every edge to a whole pixel
// first, so neighbouring rectangles meet exactly.
//
// The anchors are the exact minified text of the pinned version. If the package
// changes, the anchor is not found and this script fails the install rather than
// letting the seam come back silently.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const EXPECTED_VERSION = "0.19.0";
const MARKER = "/*pullcept#278*/";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkgDir = join(root, "node_modules", "@xterm", "addon-webgl");

// One entry per bundle the package ships. `from` is the draw call inside
// drawBlockElementChar; `to` is the same call with each edge rounded.
const targets = [
  {
    file: "lib/addon-webgl.mjs",
    from: "i.fillRect(t+a.x*l,n+a.y*u,a.w*l,a.h*u)",
    to:
      MARKER +
      "{let pcX=Math.round(t+a.x*l),pcY=Math.round(n+a.y*u);" +
      "i.fillRect(pcX,pcY,Math.round(t+(a.x+a.w)*l)-pcX,Math.round(n+(a.y+a.h)*u)-pcY)}",
  },
  {
    file: "lib/addon-webgl.js",
    from: "e.fillRect(i+a.x*l,s+a.y*h,a.w*l,a.h*h)",
    to:
      MARKER +
      "{let pcX=Math.round(i+a.x*l),pcY=Math.round(s+a.y*h);" +
      "e.fillRect(pcX,pcY,Math.round(i+(a.x+a.w)*l)-pcX,Math.round(s+(a.y+a.h)*h)-pcY)}",
  },
];

function fail(message) {
  console.error(`patch-xterm-webgl: ${message}`);
  process.exit(1);
}

let version;
try {
  version = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8")).version;
} catch {
  fail(`@xterm/addon-webgl is not installed at ${pkgDir}`);
}
if (version !== EXPECTED_VERSION) {
  fail(
    `@xterm/addon-webgl is ${version}, the patch targets ${EXPECTED_VERSION}. ` +
      "Re-check the block element drawing in the new version and update this script.",
  );
}

for (const { file, from, to } of targets) {
  const path = join(pkgDir, file);
  const text = readFileSync(path, "utf8");
  if (text.includes(MARKER)) {
    console.log(`patch-xterm-webgl: ${file} already patched`);
    continue;
  }
  const count = text.split(from).length - 1;
  if (count !== 1) {
    fail(`expected the block element draw call once in ${file}, found ${count}`);
  }
  writeFileSync(path, text.replace(from, to));
  console.log(`patch-xterm-webgl: patched ${file}`);
}
