import { parse, type DefaultTreeAdapterMap } from "parse5";

// parse5 is a non-DOM parser: reading hostile markup cannot start resource
// loads. Rebuild a deliberately limited HTML subset, never a live parent DOM.
const TAGS = new Set("a abbr address article aside b bdi bdo blockquote br button caption cite code col colgroup dd del details dfn div dl dt em figcaption figure footer h1 h2 h3 h4 h5 h6 header hgroup hr i ins kbd label legend li main mark nav ol p pre q rp rt ruby s samp section small span strong style sub summary sup table tbody td tfoot th thead time tr u ul var wbr".split(" "));
const ATTRS = new Set("class id style title lang dir role aria-label aria-hidden colspan rowspan scope span start reversed value open datetime".split(" "));
const VOID = new Set(["br", "col", "hr", "wbr"]);
const HTML = "http://www.w3.org/1999/xhtml";
const escape = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
type Node = DefaultTreeAdapterMap["node"];
type Element = DefaultTreeAdapterMap["element"];
const HANDLER = /^on[a-z]+$/;
const attributes = (node: Element, handlers = true) => node.attrs.filter(a => !a.namespace && (ATTRS.has(a.name) || (handlers && HANDLER.test(a.name)))).map(a => ` ${a.name}="${escape(a.value)}"`).join("");
const text = (node: Element) => node.childNodes.map(n => n.nodeName === "#text" ? (n as DefaultTreeAdapterMap["textNode"]).value : "").join("");
// Classic or module inline JavaScript only. External src, import maps,
// speculation rules and other script types never survive.
const SCRIPT_TYPES = new Set(["", "module", "text/javascript", "application/javascript"]);

function inlineScript(node: Element): string {
  if (node.attrs.some(a => a.name === "src")) return "";
  const type = (node.attrs.find(a => a.name === "type")?.value ?? "").trim().toLowerCase();
  if (!SCRIPT_TYPES.has(type)) return "";
  // The rebuilt element must end where parse5 ended it: no comment or nested
  // script opener may move the closing tag of the generated script.
  const body = text(node).replace(/<(?=!--|\/?script)/gi, "\\x3C");
  return `<script${type === "module" ? ' type="module"' : ""}>${body}</script>`;
}

function rebuild(node: Node): string {
  if (node.nodeName === "#text") return escape((node as DefaultTreeAdapterMap["textNode"]).value);
  if (!("tagName" in node)) return "";
  if (node.namespaceURI !== HTML) return "";
  const tag = node.tagName;
  if (tag === "script") return inlineScript(node);
  if (!TAGS.has(tag)) return "";
  const attrs = attributes(node);
  if (VOID.has(tag)) return `<${tag}${attrs}>`;
  // CSS remains raw text, but cannot terminate the generated style element.
  const content = tag === "style"
    ? text(node).replace(/</g, "\\3c ")
    : node.childNodes.map(rebuild).join("");
  return `<${tag}${attrs}>${content}</${tag}>`;
}

/**
 * Fixed boundary first; posted doctypes, heads, meta/base and URL attributes never survive.
 * Inline `<script>` and on* attributes survive and the frame CSP opens only
 * `script-src 'unsafe-inline'` (#443). Both hosts that show it, the separate window and
 * the side pane's child view (#466), hold no capability.
 */
export function htmlPreviewDocument(source: string): string {
  const tree = parse(source);
  const html = tree.childNodes.find(n => "tagName" in n && n.tagName === "html") as DefaultTreeAdapterMap["element"];
  const body = html.childNodes.find(n => "tagName" in n && n.tagName === "body") as DefaultTreeAdapterMap["element"];
  // An inline root style before the CSP could start a resource request. Keep
  // other root attributes, and apply its declarations as CSS after the boundary.
  // Root handlers would sit before the CSP; root attributes stay presentational.
  const rootAttrs = attributes({ ...html, attrs: html.attrs.filter(a => a.name !== "style") }, false);
  const rootStyle = html.attrs.find(a => a.name === "style")?.value.replace(/</g, "\\3c ") ?? "";
  const content = html.childNodes.flatMap(n => "childNodes" in n ? n.childNodes : []).map(rebuild).join("");
  return `<!doctype html><html${rootAttrs}><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'none'; font-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'"><meta name="referrer" content="no-referrer"><style>body{margin:20px;font-family:system-ui,sans-serif;overflow-wrap:anywhere}button,a{cursor:default}html{${rootStyle}}</style></head><body${attributes(body)}>${content}</body></html>`;
}

/** The document's own `<title>` text, whitespace collapsed; "" when absent. */
export function htmlTitle(source: string): string {
  const find = (node: Node): string | null => {
    if ("tagName" in node && node.tagName === "title" && node.namespaceURI === HTML)
      return node.childNodes.map(n => n.nodeName === "#text" ? (n as DefaultTreeAdapterMap["textNode"]).value : "").join("");
    if ("childNodes" in node) for (const child of node.childNodes) { const hit = find(child); if (hit !== null) return hit; }
    return null;
  };
  return (find(parse(source) as unknown as Node) ?? "").replace(/\s+/g, " ").trim();
}

/**
 * Opaque-origin srcdoc frame whose sandbox adds only `allow-scripts` (#443):
 * no same-origin, top navigation, popups, forms or modals.
 */
export function previewFrame(doc: string): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.title = "HTML プレビュー";
  frame.setAttribute("sandbox", "allow-scripts");
  frame.referrerPolicy = "no-referrer";
  frame.srcdoc = doc;
  return frame;
}
