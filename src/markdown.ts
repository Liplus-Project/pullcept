// A post's words drawn as Markdown (#431): paragraphs, headings, lists,
// quotes, tables, emphasis, inline code, code blocks and ```mermaid diagrams.
//
// The text is read into tokens by marked and every element is built here, by
// hand, from those tokens. Nothing a post says is ever handed to the page as
// HTML: raw HTML in a post (a block or an inline tag) is drawn as the text it
// is, an image is drawn as its alt text, and a link is a link only when the
// screen's own link rule accepts its address. The one exception is a mermaid
// diagram's SVG, which mermaid draws under its `strict` security level (its
// labels are sanitised by DOMPurify, and no script or click handler is kept).
import { Lexer } from "marked";
import type { Token, Tokens } from "marked";

/** How the screen draws the parts Markdown does not decide itself. */
export interface MarkdownHooks {
  /** Plain words: text with its links (#349). */
  prose(text: string): Node[];
  /** A Markdown link: its label, made a link to `href` when the screen allows it. */
  link(href: string, label: Node[]): Node[];
  /** A fenced or indented code block (#348). */
  /** `info` is the fence's whole opening-line text after the backquotes. */
  code(code: string, lang: string, info?: string): HTMLElement;
}

/** The fence's language: its first word, as `splitCodeFences` reads it. */
function fenceLang(lang: string | undefined): string {
  return (lang ?? "").trim().split(/\s+/)[0] ?? "";
}

/**
 * A post's words as Markdown tokens. GFM (tables, strikethrough, task lists,
 * bare URLs) is on; a single newline stays a newline in the text, drawn as a
 * line break by the body's `white-space`, as posts have always been.
 */
export function lexPost(text: string): Token[] {
  return new Lexer({ gfm: true, breaks: false }).lex(text);
}

/** Draw `text` as Markdown into `parent`. */
export function appendMarkdown(parent: HTMLElement, text: string, hooks: MarkdownHooks): void {
  parent.append(...blocks(lexPost(text), hooks));
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, children: Node[] = []): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.append(...children);
  return node;
}

function blocks(tokens: Token[], hooks: MarkdownHooks): Node[] {
  const out: Node[] = [];
  for (const token of tokens) out.push(...block(token, hooks));
  return out;
}

function block(token: Token, hooks: MarkdownHooks): Node[] {
  switch (token.type) {
    case "space":
    case "def":
      return [];
    case "paragraph":
      return [el("p", inline((token as Tokens.Paragraph).tokens, hooks))];
    case "heading": {
      const heading = token as Tokens.Heading;
      return [el(`h${Math.min(Math.max(heading.depth, 1), 6)}` as "h1", inline(heading.tokens, hooks))];
    }
    case "code": {
      const code = token as Tokens.Code;
      const lang = fenceLang(code.lang);
      const drawn = hooks.code(code.text, lang, (code.lang ?? "").trim());
      if (lang.toLowerCase() === "mermaid") return [mermaidBlock(code.text, drawn)];
      return [drawn];
    }
    case "blockquote":
      return [el("blockquote", blocks((token as Tokens.Blockquote).tokens, hooks))];
    case "list":
      return [list(token as Tokens.List, hooks)];
    case "table":
      return [table(token as Tokens.Table, hooks)];
    case "hr":
      return [el("hr")];
    case "text": {
      // A list item's loose words: inline tokens when marked read them.
      const text = token as Tokens.Text;
      return text.tokens ? inline(text.tokens, hooks) : hooks.prose(text.text);
    }
    case "html":
      // Raw HTML is drawn as the text it is, never as HTML.
      return [el("p", hooks.prose((token as Tokens.HTML).raw.replace(/\n+$/, "")))];
    default:
      return "raw" in token && typeof token.raw === "string" ? [el("p", hooks.prose(token.raw))] : [];
  }
}

function list(token: Tokens.List, hooks: MarkdownHooks): HTMLElement {
  const node = token.ordered ? el("ol") : el("ul");
  if (token.ordered && typeof token.start === "number" && token.start !== 1) node.setAttribute("start", String(token.start));
  for (const item of token.items) {
    const li = el("li");
    if (item.task) {
      li.className = "task";
      const box = el("input");
      box.type = "checkbox";
      box.checked = !!item.checked;
      box.disabled = true;
      li.appendChild(box);
    }
    // marked puts the checkbox as the first inline token of a task item; it is
    // drawn above, so it is dropped here.
    const tokens = item.tokens.filter((t) => t.type !== "checkbox");
    for (const child of tokens) {
      if (child.type === "paragraph" || child.type === "text") {
        const children = child.type === "paragraph" ? inline(child.tokens ?? [], hooks) : block(child, hooks);
        li.append(...(item.loose ? [el("p", children)] : children));
      } else {
        li.append(...block(child, hooks));
      }
    }
    node.appendChild(li);
  }
  return node;
}

function table(token: Tokens.Table, hooks: MarkdownHooks): HTMLElement {
  const cell = (tag: "th" | "td", c: Tokens.TableCell, align: Tokens.Table["align"][number]) => {
    const node = el(tag, inline(c.tokens, hooks));
    if (align) node.style.textAlign = align;
    return node;
  };
  const head = el("tr", token.header.map((c, i) => cell("th", c, token.align[i])));
  const rows = token.rows.map((row) => el("tr", row.map((c, i) => cell("td", c, token.align[i]))));
  const node = el("table", [el("thead", [head]), ...(rows.length ? [el("tbody", rows)] : [])]);
  // A wide table scrolls inside its frame rather than widening the bubble.
  const frame = el("div", [node]);
  frame.className = "md-table";
  return frame;
}

/**
 * Inline tokens. Neighbouring text and escapes are joined before they are
 * read for links, so a path is not cut where Markdown saw an escape; `\\` is
 * kept as written, since a UNC path begins with it (#349).
 */
function inline(tokens: Token[], hooks: MarkdownHooks): Node[] {
  const out: Node[] = [];
  let words = "";
  const flush = () => {
    if (words) out.push(...hooks.prose(words));
    words = "";
  };
  for (const token of tokens) {
    if (token.type === "text" && !(token as Tokens.Text).tokens?.length) {
      words += (token as Tokens.Text).text;
      continue;
    }
    if (token.type === "escape") {
      words += token.raw === "\\\\" ? token.raw : (token as Tokens.Escape).text;
      continue;
    }
    if (token.type === "html") {
      words += token.raw;
      continue;
    }
    flush();
    out.push(...inlineNode(token, hooks));
  }
  flush();
  return out;
}

function inlineNode(token: Token, hooks: MarkdownHooks): Node[] {
  switch (token.type) {
    case "text":
      return inline((token as Tokens.Text).tokens ?? [], hooks);
    case "strong":
      return [el("strong", inline((token as Tokens.Strong).tokens, hooks))];
    case "em":
      return [el("em", inline((token as Tokens.Em).tokens, hooks))];
    case "del":
      return [el("del", inline((token as Tokens.Del).tokens, hooks))];
    case "codespan": {
      const code = el("code");
      code.className = "inline-code";
      code.textContent = (token as Tokens.Codespan).text;
      return [code];
    }
    case "br":
      return [document.createTextNode("\n")];
    case "link": {
      const link = token as Tokens.Link;
      return hooks.link(link.href, inline(link.tokens, hooks));
    }
    case "image":
      // No picture is fetched from a post: the alt text stands in for it.
      return hooks.prose((token as Tokens.Image).text);
    default:
      return "raw" in token && typeof token.raw === "string" ? hooks.prose(token.raw) : [];
  }
}

let mermaidLoad: Promise<typeof import("mermaid").default> | undefined;
let mermaidSeq = 0;

/** mermaid, loaded on the first diagram and set up once. */
function loadMermaid(): Promise<typeof import("mermaid").default> {
  mermaidLoad ??= import("mermaid").then(({ default: mermaid }) => {
    const dark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
    mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: dark ? "dark" : "default" });
    return mermaid;
  });
  return mermaidLoad;
}

/**
 * A ```mermaid block: drawn as its code first, replaced by the diagram once
 * mermaid has drawn it. A diagram mermaid cannot read or draw stays the code.
 */
function mermaidBlock(source: string, code: HTMLElement): HTMLElement {
  const holder = el("div", [code]);
  holder.className = "mermaid-holder";
  void (async () => {
    const id = `pullcept-mermaid-${++mermaidSeq}`;
    try {
      const mermaid = await loadMermaid();
      if (!(await mermaid.parse(source, { suppressErrors: true }))) return;
      const { svg } = await mermaid.render(id, source);
      const diagram = el("div");
      diagram.className = "mermaid-block";
      diagram.innerHTML = svg;
      holder.replaceChildren(diagram);
    } catch {
      // Left as the code.
    } finally {
      document.getElementById(`d${id}`)?.remove();
    }
  })();
  return holder;
}
