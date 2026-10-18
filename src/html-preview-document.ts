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
const attributes = (node: Element, handlers = false) => node.attrs.filter(a => !a.namespace && (ATTRS.has(a.name) || (handlers && HANDLER.test(a.name)))).map(a => ` ${a.name}="${escape(a.value)}"`).join("");
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

function staticNode(node: Node, scripts: boolean): string {
  if (node.nodeName === "#text") return escape((node as DefaultTreeAdapterMap["textNode"]).value);
  if (!("tagName" in node)) return "";
  if (node.namespaceURI !== HTML) return "";
  const tag = node.tagName;
  if (tag === "script") return scripts ? inlineScript(node) : "";
  if (!TAGS.has(tag)) return "";
  const attrs = attributes(node, scripts);
  if (VOID.has(tag)) return `<${tag}${attrs}>`;
  // CSS remains raw text, but cannot terminate the generated style element.
  const content = tag === "style"
    ? text(node).replace(/</g, "\\3c ")
    : node.childNodes.map(n => staticNode(n, scripts)).join("");
  return `<${tag}${attrs}>${content}</${tag}>`;
}

/**
 * Fixed boundary first; posted doctypes, heads, meta/base and URL attributes never survive.
 * `scripts` is only for the separate window's frame (#443): inline `<script>` and on*
 * attributes survive and the frame CSP opens `script-src 'unsafe-inline'`. The side
 * pane's document never carries either.
 */
export function htmlPreviewDocument(source: string, scripts = false): string {
  const tree = parse(source);
  const html = tree.childNodes.find(n => "tagName" in n && n.tagName === "html") as DefaultTreeAdapterMap["element"];
  const body = html.childNodes.find(n => "tagName" in n && n.tagName === "body") as DefaultTreeAdapterMap["element"];
  // An inline root style before the CSP could start a resource request. Keep
  // other root attributes, and apply its declarations as CSS after the boundary.
  // Root handlers would sit before the CSP; root attributes stay presentational.
  const rootAttrs = attributes({ ...html, attrs: html.attrs.filter(a => a.name !== "style") });
  const rootStyle = html.attrs.find(a => a.name === "style")?.value.replace(/</g, "\\3c ") ?? "";
  const content = html.childNodes.flatMap(n => "childNodes" in n ? n.childNodes : []).map(n => staticNode(n, scripts)).join("");
  return `<!doctype html><html${rootAttrs}><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src ${scripts ? "'unsafe-inline'" : "'none'"}; style-src 'unsafe-inline'; img-src 'none'; font-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'"><meta name="referrer" content="no-referrer"><style>body{margin:20px;font-family:system-ui,sans-serif;overflow-wrap:anywhere}button,a{cursor:default}html{${rootStyle}}</style></head><body${attributes(body, scripts)}>${content}</body></html>`;
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
 * Opaque-origin srcdoc frame. `scripts` adds only `allow-scripts`, for the
 * separate window (#443); the side pane's frame keeps an empty sandbox.
 */
export function previewFrame(doc: string, scripts = false): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.title = "HTML プレビュー";
  frame.setAttribute("sandbox", scripts ? "allow-scripts" : "");
  frame.referrerPolicy = "no-referrer";
  frame.srcdoc = doc;
  return frame;
}
