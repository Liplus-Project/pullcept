import { parse, type DefaultTreeAdapterMap } from "parse5";

// parse5 is a non-DOM parser: reading hostile markup cannot start resource
// loads. Rebuild a deliberately static HTML subset, never a live parent DOM.
const TAGS = new Set("a abbr address article aside b bdi bdo blockquote br button caption cite code col colgroup dd del details dfn div dl dt em figcaption figure footer h1 h2 h3 h4 h5 h6 header hgroup hr i ins kbd label legend li main mark nav ol p pre q rp rt ruby s samp section small span strong style sub summary sup table tbody td tfoot th thead time tr u ul var wbr".split(" "));
const ATTRS = new Set("class id style title lang dir role aria-label aria-hidden colspan rowspan scope span start reversed value open datetime".split(" "));
const VOID = new Set(["br", "col", "hr", "wbr"]);
const HTML = "http://www.w3.org/1999/xhtml";
const escape = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
type Node = DefaultTreeAdapterMap["node"];
const attributes = (node: DefaultTreeAdapterMap["element"]) => node.attrs.filter(a => !a.namespace && ATTRS.has(a.name)).map(a => ` ${a.name}="${escape(a.value)}"`).join("");

function staticNode(node: Node): string {
  if (node.nodeName === "#text") return escape((node as DefaultTreeAdapterMap["textNode"]).value);
  if (!("tagName" in node)) return "";
  if (node.namespaceURI !== HTML || !TAGS.has(node.tagName)) return "";
  const tag = node.tagName;
  const attrs = attributes(node);
  if (VOID.has(tag)) return `<${tag}${attrs}>`;
  // CSS remains raw text, but cannot terminate the generated style element.
  const content = tag === "style"
    ? node.childNodes.map(n => n.nodeName === "#text" ? (n as DefaultTreeAdapterMap["textNode"]).value.replace(/</g, "\\3c ") : "").join("")
    : node.childNodes.map(staticNode).join("");
  return `<${tag}${attrs}>${content}</${tag}>`;
}

/** Fixed boundary first; posted doctypes, heads, meta/base and URL attributes never survive. */
export function htmlPreviewDocument(source: string): string {
  const tree = parse(source);
  const html = tree.childNodes.find(n => "tagName" in n && n.tagName === "html") as DefaultTreeAdapterMap["element"];
  const body = html.childNodes.find(n => "tagName" in n && n.tagName === "body") as DefaultTreeAdapterMap["element"];
  // An inline root style before the CSP could start a resource request. Keep
  // other root attributes, and apply its declarations as CSS after the boundary.
  const rootAttrs = attributes({ ...html, attrs: html.attrs.filter(a => a.name !== "style") });
  const rootStyle = html.attrs.find(a => a.name === "style")?.value.replace(/</g, "\\3c ") ?? "";
  const content = html.childNodes.flatMap(n => "childNodes" in n ? n.childNodes : []).map(staticNode).join("");
  return `<!doctype html><html${rootAttrs}><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src 'none'; font-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; base-uri 'none'; form-action 'none'"><meta name="referrer" content="no-referrer"><style>body{margin:20px;font-family:system-ui,sans-serif;overflow-wrap:anywhere}button,a{cursor:default}html{${rootStyle}}</style></head><body${attributes(body)}>${content}</body></html>`;
}

export function previewFrame(doc: string): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.title = "HTML プレビュー";
  frame.setAttribute("sandbox", "");
  frame.referrerPolicy = "no-referrer";
  frame.srcdoc = doc;
  return frame;
}
