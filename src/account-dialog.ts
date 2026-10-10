import type { invoke } from "@tauri-apps/api/core";
import type { listen } from "@tauri-apps/api/event";
import type { AccountKind, Account, EnvVar, LaunchFieldReport, SessionView, AvatarEdit, DialogSection, CharacterOpened, CharacterPlace, McpRunState, McpLogLine, McpServerView, McpPanelView } from "./contracts";

export interface Dependencies {
  invoke: typeof invoke;
  listen: typeof listen;
  accounts: Account[];
  shownTopicId: () => string;
  joinArgs: (args: string[]) => string;
  homeDir: string;
  localAccountId: string;
  speakerColor: (name: string, hue: number | null, own: boolean) => string;
  declaredHue: (select: HTMLSelectElement) => number | null;
  initialOf: (name: string) => string;
  avatarImages: Map<string, string>;
  drawAvatar: (mark: HTMLElement, url: string | undefined) => void;
  asciiJson: (text: string) => string;
  setAvatarImage: (accountId: string, image: Blob | null) => void;
  saveConfig: () => Promise<boolean>;
  renderPanel: () => void;
  status: (text: string, kind?: "info" | "error") => void;
  seatedAnywhere: (accountId: string) => boolean;
  renderSessionFacts: () => void;
  join: () => Promise<void>;
  allViews: () => SessionView[];
  discardView: (view: SessionView | undefined) => void;
  fillHues: (select: HTMLSelectElement, saved: string | null) => void;
}

export function createAccountDialog(deps: Dependencies) {

  /**
   * A default name for a new account that no existing account already answers to.
   *
   * A constant default would put every new account on one name, which is the
   * defect #40 removed — two participants answering alike, neither addressable.
   * The identity is the id and would survive that, but being able to name one of
   * them is the point of a name, so the default counts up past whatever is taken.
   * It is a starting point in an editable field, not a value anyone is stuck with.
   */
  function unusedAccountName(): string {
    const taken = new Set(deps.accounts.map((account) => account.name.trim()));
    for (let n = deps.accounts.length + 1; ; n += 1) {
      const candidate = `アカウント ${n}`;
      if (!taken.has(candidate)) return candidate;
    }
  }

  // ── the account dialog ───────────────────────────────────────────────────────
  //
  // One form for making, editing and deleting an account. It holds a draft and
  // writes nothing until 決定; 取消 leaves nothing behind, for a new account as
  // much as for an edit. The fields used to save on `change`, which meant ＋
  // created an account the instant it was pressed and every keystroke on the way
  // to a name was a state that had existed — there was no deciding and no undoing
  // (#59).

  /** The account being edited, or null while the form is making a new one. */
  let editing: Account | null = null;

  /** The draft the form is filling in. Never the account itself. */
  let draft: Account | null = null;

  /** True once 削除 has been armed. The shape 終了 held until #71; see #72. */
  let deleteArmed = false;

  /**
   * The environment field exactly as it was drawn for the draft (masks, one line
   * per variable), or null while it has not been drawn — still loading, or the
   * drawing failed (#163).
   *
   * What 決定 compares the field against. Unchanged, the draft's sealed values
   * stand as they are and nothing is sent to be sealed. Null, the field is not
   * read at all: an empty box that never received the stored lines is not the
   * person clearing them, and reading it as that would delete every variable.
   */
  let dialogEnvDrawn: string | null = null;

  let dialogAvatar: AvatarEdit = { kind: "keep" };

  /** Say why the form cannot be decided yet, or clear that. */
  function dialogError(text: string): void {
    dialogErrorEl.textContent = text;
  }

  /** The section on screen. Every form opens on 基本. */
  let dialogSection: DialogSection = "basic";

  /**
   * The sections a kind has, in the order the side menu lists them.
   *
   * The same judgment `showDialogKind` makes field by field, one level up: a
   * person is not launched, so a character, a launch line and an environment
   * would be sections of fields that never do anything; a server is started from
   * its entry in the file, which is its own section (#193).
   */
  function dialogSectionsFor(kind: AccountKind): DialogSection[] {
    if (kind === "mcp") return ["basic", "server"];
    if (launchesKind(kind)) return ["basic", "character", "launch", "env"];
    return ["basic"];
  }

  /** Put one section on screen and mark its entry in the side menu. */
  function showDialogSection(section: DialogSection): void {
    dialogSection = section;
    for (const pane of dialogSectionPaneEls) pane.hidden = pane.dataset.section !== section;
    for (const tab of dialogSectionTabEls) {
      const on = tab.dataset.sectionTab === section;
      tab.setAttribute("aria-selected", String(on));
      // One stop in the tab order for the whole menu; the arrows move within it.
      tab.tabIndex = on ? 0 : -1;
    }
  }

  /**
   * Bring the section holding `field` on screen, and focus the field when asked.
   *
   * For a refusal that names a field: the reason is said below every section,
   * but the field it is about may be in one that is not shown, and a field that
   * is not shown cannot take focus.
   */
  function revealDialogField(field: HTMLElement, focus = true): void {
    const section = field.closest<HTMLElement>("[data-section]")?.dataset.section;
    if (section) showDialogSection(section as DialogSection);
    if (focus) field.focus();
  }

  /** Move along the side menu by arrow key, over the sections this kind has. */
  function stepDialogSection(event: KeyboardEvent): void {
    const step =
      event.key === "ArrowDown" || event.key === "ArrowRight"
        ? 1
        : event.key === "ArrowUp" || event.key === "ArrowLeft"
          ? -1
          : 0;
    if (step === 0 && event.key !== "Home" && event.key !== "End") return;
    const shown = dialogSectionTabEls.filter((tab) => !tab.hidden);
    if (shown.length === 0) return;
    event.preventDefault();
    const at = shown.findIndex((tab) => tab.dataset.sectionTab === dialogSection);
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? shown.length - 1
          : (at + step + shown.length) % shown.length;
    showDialogSection(shown[next].dataset.sectionTab as DialogSection);
    shown[next].focus();
  }

  /** Put 削除 back to resting. */
  function disarmDelete(): void {
    deleteArmed = false;
    dialogDeleteEl.textContent = "削除";
    dialogDeleteEl.classList.remove("armed");
  }

  /**
   * Show only the fields that mean something for the kind being declared.
   *
   * A person has no command under them, so a working directory and launch options
   * would be two fields that never do anything.
   *
   * The resume line is the same judgment one level in (#156, 決定6). A kind that
   * names a CLI holds the way back into one of its sessions, so the field would
   * be a second answer to a question already answered — and a second answer is
   * one that can disagree. The kind that names none has only the field.
   */
  function showDialogKind(): void {
    const kind = dialogKindEl.value as AccountKind;
    // The groups of fields are sections of their own (#304), so the kind decides
    // which sections the side menu offers. A server launches nothing either; what
    // it is started with is its entry in the file, its own section (#193). A
    // section the kind no longer offers is left for 基本, where the kind is.
    const sections = dialogSectionsFor(kind);
    for (const tab of dialogSectionTabEls) {
      tab.hidden = !sections.includes(tab.dataset.sectionTab as DialogSection);
    }
    showDialogSection(sections.includes(dialogSection) ? dialogSection : "basic");
    dialogResumeFieldEl.hidden = kind !== "cli";
    const codex = kind === "codex_cli";
    dialogCharacterEl.placeholder = codex ? "例: character_codex_luna（ファイル名・拡張子なし）" : "例: character_Lay（output style の name）";
    document.getElementById("dialog-codex-note")!.hidden = !codex;
    dialogCodexAppServerFieldEl.hidden = !codex;
    if (launchesKind(kind)) refreshDialogLine();
    // The kind picks the folder the character file is in, or that there is none (#100).
    void refreshCharacterFile();
    // Chosen on a form making an account: the server is written and started at
    // 決定, so what there is to fill in now is what it is started with (#200).
    if (kind === "mcp" && editing === null) drawNewMcp();
  }

  /** `launches`, for a kind the form holds rather than an account. */
  function launchesKind(kind: AccountKind): boolean {
    return kind === "claude_code" || kind === "codex_cli" || kind === "cli";
  }

  /**
   * Show the command this account's launch would actually run.
   *
   * The app adds the room's own settings to whatever is typed and selects the
   * character named above, so the line written here is not the line that
   * launches; showing the result is cheaper than explaining either. The character
   * is why this reads the fields rather than the draft: it is the one place the
   * `--settings` it becomes can be seen before 決定, and a preview built from the
   * draft would only show it on the next open. The entry names this account's own
   * server, which follows the account id — so the preview holds still while the
   * name in the field above it is edited. Holding still is the point: the
   * identity being launched is the account, and renaming it does not make it
   * something else (#53).
   *
   * The working directory goes with them: another account's room registration
   * sitting in it is named on the line as one this session does not start (#103).
   * That is the case worth seeing before 決定 — pointing an account at a shared
   * directory is what puts `--settings` on a line that had none.
   */
  let dialogPreviewGeneration = 0;

  let codexPreviewTimer: ReturnType<typeof setTimeout> | null = null;

  let codexPreviewRunning = false;

  let codexPreviewQueued = false;

  async function refreshDialogPreview(): Promise<void> {
    const generation = ++dialogPreviewGeneration;
    if (codexPreviewTimer !== null) clearTimeout(codexPreviewTimer);
    if (dialogKindEl.value !== "codex_cli") {
      codexPreviewQueued = false;
      await renderDialogPreview(generation);
      return;
    }
    codexPreviewTimer = setTimeout(() => {
      codexPreviewTimer = null;
      codexPreviewQueued = true;
      void drainCodexPreview();
    }, 250);
  }

  async function drainCodexPreview(): Promise<void> {
    if (codexPreviewRunning) return;
    codexPreviewRunning = true;
    try {
      while (codexPreviewQueued) {
        codexPreviewQueued = false;
        await renderDialogPreview(dialogPreviewGeneration);
      }
    } finally { codexPreviewRunning = false; }
  }

  async function renderDialogPreview(generation: number): Promise<void> {
    if (!draft) return;
    const id = draft.id;
    try {
      const parsed = await deps.invoke<string[]>("parse_launch_options", {
        text: dialogOptionsEl.value,
      });
      const merged = await deps.invoke<{ args: string[]; character_mode: string | null; character_name: string | null; server_args: string[] | null; room_prompt: string | null; }>("preview_launch_args", {
        args: parsed,
        accountId: id,
        // The field rather than the draft, for the reason the character is read
        // that way: what the kind's conventions put on the line is on the line
        // shown, and the kind is being edited right there (#156).
        kind: dialogKindEl.value as AccountKind,
        // The topic on the glass, which is where ▶ would launch it: the entry is
        // this account's in this topic (#141, decision 4).
        topicId: deps.shownTopicId() || null,
        // The field rather than the draft: the preview answers for what the form
        // holds now, and the draft is only written at 決定.
        character: dialogCharacterEl.value.trim() || null,
        cwd: dialogCwdEl.value.trim() || null,
        command: dialogCommandEl.value.trim() || null,
        envText: dialogEnvDrawn === null ? null : dialogEnvEl.value,
        env: draft.env,
        codexAppServer: dialogCodexAppServerEl.checked,
      });
      // The form may have been closed or reopened during the round trip.
      if (draft?.id !== id || generation !== dialogPreviewGeneration) return;
      const character = merged.character_mode === "file" ? `キャラクター: ${merged.character_name}（project のファイル）\n`
        : merged.character_mode === "disabled" ? "キャラクター: project の既定は無効\n"
          : merged.character_mode === "legacy" ? `キャラクター: ${merged.character_name || "CLI の既定"}（従来の指示）\n` : "";
      const command = dialogCommandEl.value.trim() || "codex";
      // An app-server seat runs two lines: the server, then the terminal attached to it (#299).
      dialogPreviewEl.textContent = merged.server_args
        ? `${character}方式: app-server（キャラクターは developerInstructions、Li+ の output style hook はこの席で停止）
  ` +
        // The room's text Claude seats carry on --append-system-prompt, here after the character (#301).
        (merged.room_prompt ? `部屋のルール（developerInstructions のキャラクターの後）: ${merged.room_prompt}
  ` : "") +
        `app-server: ${command} ${deps.joinArgs(merged.server_args)}
  画面: ${command} ${deps.joinArgs(merged.args)}`
        : `${character}${dialogCommandEl.value.trim()} ${deps.joinArgs(merged.args)}`;
    } catch (error) {
      if (draft?.id !== id || generation !== dialogPreviewGeneration) return;
      dialogPreviewEl.textContent = dialogKindEl.value === "codex_cli" ? String(error) : "";
    }
  }

  /** The characters `cmd.exe` acts on, as the sentences below name them. */
  const CONSOLE_HAZARDS = '& | < > ^ ( ) "';

  /**
   * Say what a launch would do with what the form holds (#154, 決定4).
   *
   * The line that runs is drawn above this, and it is where the result is
   * visible — an account whose character was left off it shows a line with no
   * `outputStyle` in the JSON, and one whose options were left off shows a line
   * without them. That is legible once the person already knows what to look
   * for. This says it: what is missing from that line, and what happens at 起動.
   *
   * It does not refuse anything. The value is one the person wrote and the
   * account saves as written — what they can act on is knowing, before the
   * launch, which of these four things it will do. The launch itself refuses the
   * two it has to (`session::start_session`), and that refusal is the authority;
   * this only gets there first, at the moment it can be fixed rather than at the
   * moment it fails — the shape the two-`--settings` check here already has
   * (#99).
   */
  async function refreshDialogNotice(): Promise<void> {
    const kind = dialogKindEl.value as AccountKind;
    if (!draft || !launchesKind(kind)) {
      dialogNoticeEl.textContent = "";
      return;
    }
    const id = draft.id;
    try {
      const report = await deps.invoke<LaunchFieldReport>("launch_field_report", {
        kind,
        character: dialogCharacterEl.value.trim() || null,
        options: dialogOptionsEl.value,
        // The current form's command, including a custom native executable.
        command: dialogCommandEl.value.trim(),
        // Only where the field is the answer. On a kind that holds its own way
        // back, a line stored here is not the one that runs (#156, 決定6).
        resume: kind === "cli" ? dialogResumeEl.value.trim() || null : null,
        codexAppServer: kind === "codex_cli" && dialogCodexAppServerEl.checked,
      });
      if (draft?.id !== id) return;
      const said: string[] = [];
      if (!report.character) {
        said.push(
          "キャラクター名に、Windows の起動の行へ載せられない文字があります。" +
          "保存はできますが、起動時はキャラクターを指定せず、作業ディレクトリの既定で立ちます" +
          "（載せられるのは ASCII の英数字と空白と / : . _ - です）。",
        );
      }
      if (!report.options) {
        said.push(
          kind === "codex_cli" ? "Codex のオプションを安全に運べません。省略せず、起動を拒否します。プレビューの理由を確認してください。" :
            `起動オプションに、Windows の起動の行へ載せられない文字があります（${CONSOLE_HAZARDS}）。` +
            "保存はできますが、起動時はこの欄を丸ごと載せずに起動します。",
        );
      }
      if (!report.command) {
        said.push(
          `このアカウントの起動コマンドに、起動の行へ載せられない文字があります（${CONSOLE_HAZARDS}）。` +
          "このままでは起動できません。",
        );
      }
      if (!report.resume) {
        said.push(
          `再開コマンドに、起動の行へ載せられない文字があります（${CONSOLE_HAZARDS}）。` +
          "載せずに起動すれば戻る先へ戻らないため、このトピックが持つセッションへは戻れません。",
        );
      }
      dialogNoticeEl.textContent = said.join("\n");
    } catch {
      dialogNoticeEl.textContent = "";
    }
  }

  /**
   * Redraw both halves of what the form says about the line that would run.
   *
   * One call rather than two at each field, so a field wired to one of them
   * cannot be missing the other — which is the same reason the line itself is
   * composed in one place (`mcp_config::launch_args`).
   */
  function refreshDialogLine(): void {
    void refreshDialogPreview();
    void refreshDialogNotice();
  }

  /**
   * Open the form on one account, or on a new one when given none.
   *
   * A new account's id is minted here so the launch preview has something to name
   * a server after. That is all it is until 決定 — nothing is pushed into the
   * account list, so 取消 leaves no account behind and no id in use.
   */
  function openAccountDialog(account: Account | null, field: "name" | "hue" = "name"): void {
    editing = account;
    draft = account
      ? { ...account, args: [...account.args] }
      : {
        // Opaque and minted once. Nothing reads a name out of it — the key in
        // `.mcp.json` derives from it precisely so renaming is free (#53).
        id: crypto.randomUUID(),
        name: unusedAccountName(),
        // The command the kind below names. See src-tauri/src/config.rs.
        command: "claude",
        args: [],
        // A prefill, not a default: the app launches nothing in a directory the
        // person has not seen on screen (#20).
        cwd: deps.homeDir || null,
        hue: null,
        // The one vendor the room is built on, and the kind that knows how to
        // drive it (#156). See src-tauri/src/config.rs.
        kind: "claude_code",
        // Nothing, rather than a guess at a style name: an unnamed character
        // launches on whatever the working directory's own settings say, which
        // is an answer. A guessed name that resolves to no style is not.
        character: null,
        // Nothing, because the kind above holds the way back. This field is
        // the generic kind's, and it is blank there too until someone writes
        // the line the app has none of (#156, 決定6).
        resume_command: null,
        // Nothing added to the environment until someone writes a line (#163).
        env: [],
        // No server until one is written: choosing `mcp` below makes one at
        // 決定, and the entry's name comes back from the app then (#200).
        server: null,
        // The initial until an image is picked (#236).
        avatar: false,
      };

    dialogTitleEl.textContent = account ? "アカウントの編集" : "アカウントの追加";
    dialogNameEl.value = draft.name;
    // `mcp` is offered where an account is being made (#200) and on an `mcp`
    // account's own form, and on no other: an account that exists as another kind
    // does not become a server, and the form of one that is a server cannot
    // change it (#193) — the account answers to an entry in the file, and the
    // kinds either side of it launch.
    const server = draft.kind === "mcp";
    const offered = account === null || server;
    dialogKindMcpEl.hidden = !offered;
    dialogKindMcpEl.disabled = !offered;
    dialogKindEl.disabled = server;
    dialogKindEl.value = draft.kind;
    mcpDrawn = null;
    mcpErrorEl.textContent = "";
    // Emptied for every form, so a new server starts from nothing rather than
    // from the fields of the last account the form was open on.
    mcpCommandEl.value = "";
    mcpArgsEl.value = "";
    mcpEnvEl.value = "";
    dialogHueEl.value = draft.hue === null ? "" : String(draft.hue);
    resetDialogAvatar();
    drawDialogAvatar();
    dialogCwdEl.value = draft.cwd ?? "";
    dialogCharacterEl.value = draft.character ?? "";
    dialogCodexAppServerEl.checked = draft.codex_app_server === true;
    dialogCommandEl.value = draft.command;
    dialogOptionsEl.value = deps.joinArgs(draft.args);
    dialogResumeEl.value = draft.resume_command ?? "";
    void drawDialogEnv(draft);
    dialogDeleteEl.hidden = account === null;
    disarmDelete();
    dialogError("");
    // Cleared before the round trip that refills it, so the account being opened
    // is never read against the last one's notice.
    dialogNoticeEl.textContent = "";
    // The body is read for this account's place by `showDialogKind` below (#100).
    resetCharacterFile();
    dialogSection = "basic";
    showDialogKind();
    dialogEl.showModal();
    // On the colour when the form was opened to change it (色を変える, #224);
    // on the name otherwise, selected so typing replaces it.
    if (field === "hue") dialogHueEl.focus();
    else {
      dialogNameEl.focus();
      dialogNameEl.select();
    }
    if (server) {
      // What was last read, at once, and then read again: the log may have moved
      // while the form was closed. The tail is where a log is read from.
      drawDialogMcp();
      mcpLogEl.scrollTop = mcpLogEl.scrollHeight;
      void refreshMcpServers();
    }
  }

  /** The file the body field was last filled from; null when it shows none. */
  let characterOpened: CharacterOpened | null = null;

  /** The place `characterOpened` (or the refusal on screen) was read for, as one key. */
  let characterOpenedKey: string | null = null;

  /** Bumped by every read, so a read that lands after a later one is dropped. */
  let characterReads = 0;

  let characterTimer: number | undefined;

  /** The 決定 that was asked to be pressed again, as place and body: a second
   *  press of the same is the confirmation (the shape 削除 has). */
  let characterArmed: string | null = null;

  function characterPlace(): CharacterPlace | null {
    const kind = dialogKindEl.value;
    if (kind !== "claude_code" && kind !== "codex_cli") return null;
    return { kind, cwd: dialogCwdEl.value.trim(), name: dialogCharacterEl.value.trim() };
  }

  function characterKey(place: CharacterPlace): string {
    return `${place.kind}\n${place.cwd}\n${place.name}`;
  }

  /** Whether the body holds an edit not yet written. */
  function characterDirty(): boolean {
    return characterOpened !== null && dialogCharacterBodyEl.value !== characterOpened.body;
  }

  /** Forget the file, for a form opening on another account. */
  function resetCharacterFile(): void {
    characterReads++;
    window.clearTimeout(characterTimer);
    characterOpened = null;
    characterOpenedKey = null;
    characterArmed = null;
    dialogCharacterBodyEl.value = "";
    // Until the file arrives, so nothing typed is overwritten by a read landing late.
    dialogCharacterBodyEl.disabled = true;
  }

  /** What is said under the body about the file on screen. */
  function drawCharacterNotice(place: CharacterPlace): void {
    const opened = characterOpened;
    const lines: string[] = [];
    if (opened && !opened.exists) lines.push("このファイルはまだありません。本文を書いて決定すると新しく作ります。");
    if (opened?.name_in_file && opened.name_in_file !== place.name) {
      lines.push(
        place.kind === "claude_code"
          ? `このファイルの frontmatter の name は「${opened.name_in_file}」です。Claude は name で選ぶため、キャラクター欄の「${place.name}」ではこのファイルが選ばれません。`
          : `このファイルの frontmatter の name は「${opened.name_in_file}」です。Codex はファイル名と違う name のファイルで起動を止めます。`,
      );
    }
    if (opened && place.kind === "codex_cli" && !opened.folder_exists) {
      lines.push(
        "この作業ディレクトリには .codex/output-styles がありません。決定で作ると、ここで起動する Codex の席はすべてファイル方式になり、キャラクター欄が空の席は character_instance.md を要します。",
      );
    }
    dialogCharacterNoticeEl.textContent = lines.join("\n");
  }

  /**
   * Read the file the three fields name into the body field (#100), when they
   * name another one than the field holds.
   *
   * An edit not yet written is not dropped for it: the field keeps the edit and
   * says that reading the new place would discard it, and 読み直す does that.
   */
  async function refreshCharacterFile(force = false): Promise<void> {
    window.clearTimeout(characterTimer);
    const place = characterPlace();
    dialogCharacterFileEl.hidden = place === null;
    if (place === null) return;
    const key = characterKey(place);
    if (!force && key === characterOpenedKey) {
      dialogCharacterReloadEl.hidden = true;
      // Back on the file shown, after a warning about leaving it. A refusal or
      // the empty name's hint on screen stays as it is.
      if (characterOpened !== null) drawCharacterNotice(place);
      return;
    }
    if (!force && characterDirty()) {
      dialogCharacterReloadEl.hidden = false;
      dialogCharacterNoticeEl.textContent =
        "本文に保存していない変更があります。種別・作業ディレクトリ・キャラクターが変わったため、新しい場所のファイルを読むと変更は捨てられます。読むには「読み直す」を、変更を残すには元の値に戻してください。";
      return;
    }
    dialogCharacterReloadEl.hidden = true;
    const read = ++characterReads;
    if (!place.name) {
      characterOpened = null;
      characterOpenedKey = key;
      dialogCharacterBodyEl.value = "";
      dialogCharacterBodyEl.disabled = true;
      dialogCharacterPathEl.textContent = `${place.cwd || "<作業ディレクトリ>"}\\${place.kind === "codex_cli" ? ".codex" : ".claude"}\\output-styles\\<キャラクター>.md`;
      dialogCharacterNoticeEl.textContent = "キャラクター欄に名前を書くと、そのファイルをここで開きます。";
      return;
    }
    let opened: CharacterOpened | null = null;
    let refusal = "";
    try {
      opened = await deps.invoke<CharacterOpened>("open_character_file", { kind: place.kind, cwd: place.cwd, name: place.name });
    } catch (err) {
      refusal = String(err);
    }
    if (read !== characterReads) return;
    characterOpened = opened;
    characterOpenedKey = key;
    characterArmed = null;
    dialogCharacterBodyEl.value = opened?.body ?? "";
    dialogCharacterBodyEl.disabled = opened === null;
    dialogCharacterPathEl.textContent = opened?.path ?? "—";
    if (opened) drawCharacterNotice(place);
    else dialogCharacterNoticeEl.textContent = refusal;
  }

  /** `refreshCharacterFile` after typing settles, for the fields read per key. */
  function scheduleCharacterFile(): void {
    window.clearTimeout(characterTimer);
    characterTimer = window.setTimeout(() => void refreshCharacterFile(), 300);
  }

  /**
   * Write the body at 決定, when it was changed (#100).
   *
   * Answers whether the form may go on, and whether anything was written. Before
   * writing, names the other accounts the same file is the character of, and on a
   * Codex directory with no style folder says that making one switches its seats
   * to file mode; either asks for 決定 a second time.
   */
  async function settleCharacterFile(
    settlingId: string,
  ): Promise<{ ok: boolean; written: boolean; }> {
    const place = characterPlace();
    const opened = characterOpened;
    if (place === null || opened === null || !characterDirty()) return { ok: true, written: false };
    if (characterKey(place) !== characterOpenedKey) {
      dialogError("キャラクターのファイルの場所が変わりました。「読み直す」で開き直すか、元の値に戻してください。");
      revealDialogField(dialogCharacterBodyEl, false);
      return { ok: false, written: false };
    }
    const body = dialogCharacterBodyEl.value;
    const asks: string[] = [];
    try {
      const wearers = await deps.invoke<string[]>("character_file_wearers", {
        kind: place.kind,
        cwd: place.cwd,
        name: place.name,
        others: deps.accounts
          .filter((one) => one.id !== settlingId)
          .map((one) => ({ name: one.name, kind: one.kind, cwd: one.cwd, character: one.character })),
      });
      if (wearers.length > 0) {
        asks.push(`このファイルは ${wearers.map((one) => `「${one}」`).join("")} のキャラクターでもあります。保存するとそちらも変わります。`);
      }
    } catch (err) {
      dialogError(String(err));
      revealDialogField(dialogCharacterBodyEl, false);
      return { ok: false, written: false };
    }
    if (place.kind === "codex_cli" && !opened.folder_exists) {
      asks.push(".codex/output-styles を作ると、この作業ディレクトリの Codex の席はファイル方式になります。");
    }
    const token = `${characterKey(place)}\n${body}`;
    if (asks.length > 0 && characterArmed !== token) {
      characterArmed = token;
      dialogError(`${asks.join("")}もう一度「決定」を押すと保存します。`);
      revealDialogField(dialogCharacterBodyEl, false);
      return { ok: false, written: false };
    }
    try {
      const saved = await deps.invoke<{ written: boolean; opened: CharacterOpened; }>("save_character_file", {
        kind: place.kind,
        cwd: place.cwd,
        name: place.name,
        body,
        crlf: opened.crlf,
        bom: opened.bom,
        stamp: opened.stamp,
      });
      characterOpened = saved.opened;
      characterArmed = null;
      return { ok: true, written: saved.written };
    } catch (err) {
      dialogError(String(err));
      revealDialogField(dialogCharacterBodyEl, false);
      return { ok: false, written: false };
    }
  }

  /**
   * Draw the draft's environment into the form: `NAME=<mask>` per line (#163).
   *
   * The masks are made on the app's side, which is the only side that can open a
   * value; this screen is handed the drawing and nothing else. The field is
   * read-only until it arrives, so nothing typed into it is overwritten by a
   * drawing that lands late, and a drawing for a form that has since been opened
   * on another account is dropped.
   */
  async function drawDialogEnv(forDraft: Account): Promise<void> {
    dialogEnvDrawn = null;
    dialogEnvEl.value = "";
    dialogEnvEl.readOnly = true;
    let text: string;
    try {
      text = await deps.invoke<string>("account_env_text", { env: forDraft.env });
    } catch (err) {
      if (draft !== forDraft) return;
      // Left read-only and undrawn: 決定 then keeps the stored variables as they
      // are rather than reading an empty box as their removal.
      dialogError(`環境変数を表示できませんでした: ${err}`);
      return;
    }
    if (draft !== forDraft) return;
    dialogEnvEl.value = text;
    dialogEnvDrawn = text;
    dialogEnvEl.readOnly = false;
    refreshDialogLine();
  }

  /** Drop the form's image edit, and the object URL a picked image holds. */
  function resetDialogAvatar(): void {
    if (dialogAvatar.kind === "set") URL.revokeObjectURL(dialogAvatar.url);
    dialogAvatar = { kind: "keep" };
    dialogAvatarInputEl.value = "";
  }

  /**
   * Draw the form's circle as 決定 would leave it (#236): the picked image, the
   * account's own while it is kept, or the initial of the name in the field on the
   * colour chosen above it. 外す is offered only while there is an image to take
   * off.
   */
  function drawDialogAvatar(): void {
    if (!draft) return;
    const own = draft.id === deps.localAccountId;
    dialogAvatarEl.style.setProperty(
      "--speaker",
      deps.speakerColor(dialogNameEl.value, deps.declaredHue(dialogHueEl), own),
    );
    dialogAvatarEl.dataset.initial = deps.initialOf(dialogNameEl.value);
    const url =
      dialogAvatar.kind === "set"
        ? dialogAvatar.url
        : dialogAvatar.kind === "keep" && draft.avatar
          ? deps.avatarImages.get(draft.id)
          : undefined;
    deps.drawAvatar(dialogAvatarEl, url);
    dialogAvatarClearEl.hidden = url === undefined;
  }

  /** The size, in pixels a side, every stored image is scaled to (#236). */
  const AVATAR_SIZE = 128;

  /**
   * The formats an image may be picked in (#236). What the webview decodes is
   * wider than this; these are the ones the docs name, and an animated GIF gives
   * its first frame.
   */
  const AVATAR_TYPES = ["image/png", "image/jpeg", "image/webp", "image/gif"];

  /**
   * Turn a picked image into what is stored (#236): its centre square, scaled to
   * `AVATAR_SIZE` on a side, as a PNG. Made here rather than in the app because
   * the webview already decodes every format offered; the app is handed one small
   * PNG and only keeps it, so the original's size bounds nothing.
   */
  async function squarePng(file: Blob): Promise<Blob> {
    const bitmap = await createImageBitmap(file);
    try {
      const side = Math.min(bitmap.width, bitmap.height);
      const canvas = document.createElement("canvas");
      canvas.width = AVATAR_SIZE;
      canvas.height = AVATAR_SIZE;
      const context = canvas.getContext("2d");
      if (!context || side === 0) throw new Error("empty image");
      context.imageSmoothingQuality = "high";
      context.drawImage(
        bitmap,
        (bitmap.width - side) / 2,
        (bitmap.height - side) / 2,
        side,
        side,
        0,
        0,
        AVATAR_SIZE,
        AVATAR_SIZE,
      );
      return await new Promise<Blob>((resolve, reject) =>
        canvas.toBlob(
          (png) => (png ? resolve(png) : reject(new Error("not encoded"))),
          "image/png",
        ),
      );
    } finally {
      bitmap.close();
    }
  }

  /**
   * Take the file picked for the image into the form (#236). Nothing is written
   * until 決定. An image that will not decode is said on the form and leaves the
   * field as it was.
   */
  async function pickDialogAvatar(): Promise<void> {
    const file = dialogAvatarInputEl.files?.[0];
    dialogAvatarInputEl.value = "";
    if (!file || !draft) return;
    const forDraft = draft;
    if (!AVATAR_TYPES.includes(file.type)) {
      dialogError("画像は png・jpeg・webp・gif のどれかを選んでください。");
      return;
    }
    let png: Blob;
    try {
      png = await squarePng(file);
    } catch {
      if (draft === forDraft) dialogError(`${file.name} を画像として読めませんでした。`);
      return;
    }
    // The form may have been closed, or opened on another account, meanwhile.
    if (draft !== forDraft) return;
    resetDialogAvatar();
    dialogAvatar = { kind: "set", png, url: URL.createObjectURL(png) };
    dialogError("");
    drawDialogAvatar();
  }

  /**
   * Carry out the form's image edit for the account `accountId`, at 決定 (#236).
   *
   * Answers the account's flag as it should be saved — `had` when the image was
   * left alone — or null when the file could not be written or removed, which is
   * said on the form. Runs before the config is saved, so a flag never names an
   * image that was not written.
   */
  async function settleDialogAvatar(
    edit: AvatarEdit,
    accountId: string,
    had: boolean,
  ): Promise<boolean | null> {
    try {
      if (edit.kind === "set") {
        await deps.invoke("save_account_avatar", new Uint8Array(await edit.png.arrayBuffer()), {
          headers: { "Pullcept-Account": deps.asciiJson(accountId) },
        });
        deps.setAvatarImage(accountId, edit.png);
        return true;
      }
      if (edit.kind === "clear") {
        await deps.invoke("delete_account_avatar", { accountId });
        deps.setAvatarImage(accountId, null);
        return false;
      }
    } catch (err) {
      dialogError(String(err));
      revealDialogField(dialogAvatarEl, false);
      return null;
    }
    return had;
  }

  /**
   * Take what the form holds and put it into the account list.
   *
   * The one moment anything here reaches the list. Returns false when the form
   * cannot be decided yet, so the dialog stays open on its own reason.
   */
  async function commitAccountDialog(): Promise<boolean> {
    if (!draft) return false;
    // Read before the await below. Escape closes the dialog on its own, and the
    // close handler clears both — reading them afterwards would push a second
    // copy of an account that was being edited.
    const target = editing;
    const settling = draft;
    const avatarEdit = dialogAvatar;

    const name = dialogNameEl.value.trim();
    if (!name) {
      dialogError("名前を入力してください。部屋での名乗りになります。");
      revealDialogField(dialogNameEl);
      return false;
    }

    const kind = dialogKindEl.value as AccountKind;
    // Made here, from nothing: the entry is written and the account with it
    // (#200). Only a form making an account reaches this — an existing account
    // is not offered the kind.
    if (kind === "mcp" && !target) return await createMcpAccount(settling, name, avatarEdit);
    // A server's account is a name and a colour over an entry in the file, and the
    // entry is the section below the form's fields (#193). Nothing else here
    // applies to it, and no other kind becomes it or stops being it.
    if (kind === "mcp" || target?.kind === "mcp") {
      if (!target || target.kind !== "mcp" || kind !== "mcp") {
        dialogError("MCP サーバのアカウントの種別は変えられません。");
        revealDialogField(dialogKindEl);
        return false;
      }
      // An edit to the server not yet saved is saved with the rest, rather than
      // lost to the form closing over it. A field that will not save keeps the
      // form open on its reason.
      if (mcpFieldsEdited() && !(await saveMcpServer())) {
        dialogError("サーバの設定を保存できませんでした。");
        revealDialogField(mcpCommandEl, false);
        return false;
      }
      const avatar = await settleDialogAvatar(avatarEdit, settling.id, settling.avatar);
      if (avatar === null) return false;
      const settled: Account = { ...settling, name, hue: deps.declaredHue(dialogHueEl), avatar };
      const at = deps.accounts.findIndex((one) => one.id === target.id);
      if (at >= 0) deps.accounts[at] = settled;
      deps.saveConfig();
      deps.renderPanel();
      deps.status(`アカウント「${settled.name}」を保存しました。`);
      return true;
    }
    // A running account cannot change kind. Its session is in the room under this
    // account, and turning it into a person would drop the working directory and
    // options that session was launched from while it is still running.
    if (target && kind !== target.kind && deps.seatedAnywhere(target.id)) {
      dialogError(`「${target.name}」は起動中です。種別を変えるには先に終了してください。`);
      revealDialogField(dialogKindEl);
      return false;
    }
    // The person at this screen is a person. Turning their account into one that
    // launches would list them under the wrong heading and offer to start a CLI
    // under their name, which is not a thing there is one of.
    if (target && target.id === deps.localAccountId && kind !== "admin") {
      dialogError("この画面の本人のアカウントは種別 admin のままです。");
      revealDialogField(dialogKindEl);
      return false;
    }
    const cwd = dialogCwdEl.value.trim();
    const character = dialogCharacterEl.value.trim();
    // Only for a session. A person's working directory, character and options
    // would be values nothing ever reads, kept alive by an edit that once set
    // them. A person is not launched, so nothing selects a style for them.
    const args =
      kind === "admin"
        ? []
        : await deps.invoke<string[]>("parse_launch_options", { text: dialogOptionsEl.value });

    // The character rides in `--settings`, so one written by hand up in the
    // options is the same setting declared twice. Said here because this is the
    // one place both fields are on screen together, and in the language they are
    // read in; the app refuses the launch as well, and that refusal is the
    // authority — this check only gets there first, at the moment it can be
    // fixed rather than at the moment it fails (#99).
    if (kind !== "codex_cli" && character && args.some((arg) => arg.split("=")[0] === "--settings")) {
      dialogError(
        "起動オプションの --settings とキャラクターは同じ設定を指します。どちらか一方にしてください。",
      );
      revealDialogField(dialogOptionsEl);
      return false;
    }

    // Sealed on the app's side before anything is stored (#163). Only when the
    // field was drawn and then changed: a field left as drawn is the stored
    // values, and one that was never drawn says nothing about them.
    let env = settling.env;
    if (kind === "admin") {
      env = [];
    } else if (dialogEnvDrawn !== null && dialogEnvEl.value !== dialogEnvDrawn) {
      try {
        env = await deps.invoke<EnvVar[]>("seal_account_env", {
          text: dialogEnvEl.value,
          previous: settling.env,
        });
      } catch (err) {
        dialogError(String(err));
        revealDialogField(dialogEnvEl);
        return false;
      }
    }

    // The character's body (#100), after everything about the account itself that
    // can still refuse the form: it is a file of its own, as the image is.
    const characterFile = await settleCharacterFile(settling.id);
    if (!characterFile.ok) return false;

    // Last, after everything that can still refuse the form: the image is a file
    // of its own, and one written for a form that is then refused would be an
    // image for an account that was never decided (#236).
    const avatar = await settleDialogAvatar(avatarEdit, settling.id, settling.avatar);
    if (avatar === null) return false;

    const settled: Account = {
      ...settling,
      command: launchesKind(kind) ? dialogCommandEl.value.trim() : settling.command,
      name,
      kind,
      avatar,
      hue: deps.declaredHue(dialogHueEl),
      cwd: kind === "admin" ? null : cwd || null,
      // Blank clears it, and clearing it is a state: the account goes back to
      // launching on whatever its working directory's own settings name.
      character: kind === "admin" ? null : character || null,
      // Only the kind whose form shows this field keeps it (#156, 決定6). On a
      // kind that holds its own way back, a line stored here would be one nothing
      // reads and nobody can see to correct. Blank is a state on the kind that
      // does keep it, and the common one: an account with no resume line joins a
      // reopened topic as a new session and reads back what it needs through the
      // room's own pull instead (#115, decision 4C).
      resume_command: kind === "cli" ? dialogResumeEl.value.trim() || null : null,
      // Kept only where it is read (#299).
      codex_app_server: kind === "codex_cli" && dialogCodexAppServerEl.checked,
      args,
      env,
    };

    if (target) {
      const at = deps.accounts.findIndex((one) => one.id === target.id);
      if (at >= 0) deps.accounts[at] = settled;
    } else {
      deps.accounts.push(settled);
    }
    deps.saveConfig();

    deps.renderPanel();
    deps.renderSessionFacts();
    // The room holds this screen's person's name and colour on its seat, so a
    // rename here has to be re-declared or the roster keeps the old pair.
    if (settled.id === deps.localAccountId) await deps.join();
    deps.status(
      (target
        ? `アカウント「${settled.name}」を保存しました。`
        : `アカウント「${settled.name}」を追加しました。`) +
      (characterFile.written ? "キャラクターの本文を書き込みました（席の次の起動から効きます）。" : ""),
    );
    return true;
  }

  /**
   * Make an account of kind `mcp` from the form, and the server it is (#200).
   *
   * The entry first, because it is what the account answers to: the app writes it
   * under a name taken from the account's and hands back that name and the id the
   * account is given. Then the account, saved before the server is started, so
   * the server's first post is said under the name and colour chosen here rather
   * than the entry's name the app would fall back to. Then the start, from the
   * file, the way 再起動 starts one.
   *
   * A field the app refuses keeps the form open on its reason, with nothing
   * written. A server that will not start is still made: its account is there,
   * and its window says what happened and holds 起動.
   */
  async function createMcpAccount(
    settling: Account,
    name: string,
    avatarEdit: AvatarEdit,
  ): Promise<boolean> {
    mcpErrorEl.textContent = "";
    let created: { server: string; account_id: string; };
    try {
      created = await deps.invoke<{ server: string; account_id: string; }>("create_mcp_server", {
        name,
        command: mcpCommandEl.value,
        args: mcpArgsEl.value,
        env: mcpEnvEl.value,
      });
    } catch (err) {
      mcpErrorEl.textContent = String(err);
      dialogError("サーバを設定ファイルに書けませんでした。");
      revealDialogField(mcpCommandEl, false);
      return false;
    }
    const settled: Account = {
      ...settling,
      id: created.account_id,
      name,
      kind: "mcp",
      hue: deps.declaredHue(dialogHueEl),
      // An account has one shape, and nothing is launched from these: the server
      // is started from its entry in the file (`mcp_servers::migrate_accounts`).
      command: "",
      args: [],
      cwd: null,
      character: null,
      resume_command: null,
      env: [],
      server: created.server,
      avatar: false,
    };
    // The id is the app's, so the image is written only now. A file that will not
    // write does not hold the form open: the entry is already in the file, and a
    // second 決定 would make a second one. The account is made without the image,
    // and the status line says so below (#236).
    const avatar = await settleDialogAvatar(avatarEdit, settled.id, false);
    settled.avatar = avatar ?? false;
    deps.accounts.push(settled);
    await deps.saveConfig();
    let started = true;
    try {
      await deps.invoke("restart_mcp_server", { name: created.server });
    } catch {
      started = false;
    }
    deps.renderPanel();
    await refreshMcpServers();
    if (avatar === null) {
      deps.status(`アカウント「${settled.name}」を追加しましたが、画像を保存できませんでした。`, "error");
    } else if (started) {
      deps.status(`アカウント「${settled.name}」を追加し、サーバ「${created.server}」を起動しました。`);
    } else {
      deps.status(
        `アカウント「${settled.name}」を追加しましたが、サーバ「${created.server}」を起動できませんでした。`,
        "error",
      );
    }
    return true;
  }

  /**
   * Delete the account the form is open on, on the second click.
   *
   * Two clicks rather than `window.confirm`, for the reason 終了 does not use one
   * either: a host that answers nothing makes the button either silently dead or
   * — the bias `confirm` defaults to — destructive on one click (#57). 終了 asks
   * in a `<dialog>` of the app's own since #71; whether this follows is #72.
   *
   * Refused while it is running: the session in the room belongs to this account,
   * and deleting the account under it would leave a participant on the roster
   * that nothing on this screen can name or account for. Refused for the person
   * at this screen too — they are in the room by being here, and there would be
   * nothing left to be here as.
   *
   * A server's account takes its server with it (#200): the entry comes out of
   * the file and the run is ended, before the account goes. In that order,
   * because an entry still listed would be given an account again on the next
   * read of the config — so a file that cannot be written keeps the account.
   */
  async function deleteFromDialog(): Promise<void> {
    const account = editing;
    if (!account) return;

    if (deps.seatedAnywhere(account.id)) {
      dialogError(`「${account.name}」は起動中です。セッションを終了してから削除してください。`);
      disarmDelete();
      return;
    }
    if (account.id === deps.localAccountId) {
      dialogError("この画面の本人のアカウントは削除できません。");
      disarmDelete();
      return;
    }
    const server = account.kind === "mcp" ? account.server : null;
    if (!deleteArmed) {
      deleteArmed = true;
      dialogDeleteEl.textContent = "本当に削除";
      dialogDeleteEl.classList.add("armed");
      dialogError(
        server
          ? `もう一度押すと削除します。設定ファイルからサーバ「${server}」を外し、動いていれば止めます。`
          : "もう一度押すと削除します。",
      );
      return;
    }

    if (server) {
      try {
        await deps.invoke("delete_mcp_server", { name: server });
      } catch (err) {
        dialogError(`サーバを設定ファイルから外せませんでした: ${err}`);
        disarmDelete();
        return;
      }
    }

    deps.accounts = deps.accounts.filter((candidate) => candidate.id !== account.id);
    // Its terminal goes with it. An account that no longer exists cannot be named
    // in the panel, and the row is the only way that pane could be reached.
    for (const view of [...deps.allViews()]) {
      if (view.accountId === account.id) deps.discardView(view);
    }
    deps.saveConfig();
    closeAccountDialog();
    deps.renderPanel();
    deps.renderSessionFacts();
    deps.status(`アカウント「${account.name}」を削除しました。`);
    if (server) void refreshMcpServers();
    // Its image goes with it (#236), whether or not the flag said there was one:
    // a file left behind would be found by nothing, and an `mcp` account made
    // again under the same entry name takes the same id.
    deps.setAvatarImage(account.id, null);
    try {
      await deps.invoke("delete_account_avatar", { accountId: account.id });
    } catch (err) {
      deps.status(`アカウント「${account.name}」を削除しましたが、画像を消せませんでした: ${err}`, "error");
    }
  }

  /** Drop the draft and close. Nothing it held reached the account list. */
  function closeAccountDialog(): void {
    editing = null;
    draft = null;
    disarmDelete();
    if (dialogEl.open) dialogEl.close();
  }

  /** The servers as last read, or null before the first read answered. */
  let mcpPanel: McpPanelView | null = null;

  /** A read is on its way, and another was asked for while it was. */
  let mcpReading = false;

  let mcpReadAgain = false;

  /** The fields as last drawn from the file, to tell an edit from what is saved.
   *  A refresh redraws state and log under an edit, never the edit itself. */
  let mcpDrawn: { name: string; command: string; args: string; env: string; } | null = null;

  /**
   * The server an `mcp` account answers to, as last read.
   *
   * `view` is undefined when the file lists no entry of that name and this run
   * has not run one under it: the account outlived its entry, and says so.
   */
  function mcpServerOf(account: Account): { name: string; view: McpServerView | undefined; } {
    const name = account.server ?? "";
    return { name, view: mcpPanel?.servers.find((server) => server.name === name) };
  }

  /**
   * What an `mcp` account's row says about its server (#193).
   *
   * The words the session rows use where they mean the same thing — 起動中,
   * 終了, 起動失敗, 未起動 — so one list does not say one state two ways. Running
   * says nothing, as a session in the room says nothing (#82). 停止 is a run
   * ended by hand, and 未登録 an account whose entry is gone from the file. Each
   * fits the width 起動失敗 already takes (#71).
   */
  function mcpNote(view: McpServerView | undefined): { text: string; kind: string; title: string; } {
    if (!view) return { text: "未登録", kind: "", title: "設定ファイルにこのサーバはありません。" };
    const state = view.state;
    if (!state) return { text: view.listed ? "未起動" : "未登録", kind: "", title: "" };
    switch (state.state) {
      case "starting":
        return { text: "起動中", kind: "", title: "" };
      case "running":
        return { text: "", kind: "", title: "" };
      case "ended":
        return { text: "終了", kind: "", title: state.detail };
      case "failed":
        return { text: "起動失敗", kind: "error", title: state.detail };
      case "stopped":
        return { text: "停止", kind: "", title: "" };
    }
  }

  function mcpStateText(state: McpRunState | null): string {
    if (!state) return "未起動";
    switch (state.state) {
      case "starting":
        return "起動中";
      case "running":
        return "実行中";
      case "ended":
        return `終了（${state.detail}）`;
      case "failed":
        return `失敗（${state.detail}）`;
      case "stopped":
        return "停止";
    }
  }

  /** `ok` for running, `error` for a run that ended or never started, and nothing
   *  for the states on the way — the same two colours the socket row uses. */
  function mcpStateKind(state: McpRunState | null): string {
    if (state?.state === "running") return "ok";
    if (state?.state === "ended" || state?.state === "failed") return "error";
    return "";
  }

  /** The server the account form is open on, or null when it is on no `mcp`
   *  account. */
  function dialogServer(): string | null {
    return draft?.kind === "mcp" ? draft.server : null;
  }

  function mcpFieldsEdited(): boolean {
    const name = dialogServer();
    if (!mcpDrawn || name === null || mcpDrawn.name !== name) return false;
    return (
      mcpCommandEl.value !== mcpDrawn.command ||
      mcpArgsEl.value !== mcpDrawn.args ||
      mcpEnvEl.value !== mcpDrawn.env
    );
  }

  function mcpLogText(lines: McpLogLine[]): string {
    return lines
      .map((line) => {
        const time = new Date(line.at_ms).toLocaleTimeString("ja-JP", { hour12: false });
        return `${time}  ${line.text}`;
      })
      .join("\n");
  }

  /**
   * What the rows read off the servers: the note each would draw. Compared before
   * and after a read, so a log line — which moves no row — does not redraw the
   * participant list under the person using it.
   */
  function mcpRowSignature(): string {
    return deps.accounts
      .filter((account) => account.kind === "mcp")
      .map((account) => `${account.id}:${mcpNote(mcpServerOf(account).view).text}`)
      .join("\n");
  }

  /**
   * Read the servers again: the rows, and the account form when it is open on
   * one of them.
   *
   * Called once at startup, after each act, and on every `mcp-servers-changed`.
   * A read asked for while one is on its way is folded into one more after it, so
   * a burst of log lines is not a burst of reads.
   */
  async function refreshMcpServers(): Promise<void> {
    if (mcpReading) {
      mcpReadAgain = true;
      return;
    }
    mcpReading = true;
    try {
      do {
        mcpReadAgain = false;
        const before = mcpRowSignature();
        try {
          mcpPanel = await deps.invoke<McpPanelView>("mcp_servers");
        } catch (err) {
          mcpFileErrorEl.textContent = String(err);
          continue;
        }
        if (mcpRowSignature() !== before) deps.renderPanel();
        drawDialogMcp();
      } while (mcpReadAgain);
    } finally {
      mcpReading = false;
    }
  }

  /**
   * Draw the account form's server section from the last read (#193).
   *
   * The fields are redrawn only when they hold what was last drawn into them: a
   * log line arriving while someone types is not a reason to take what they typed
   * away.
   */
  function drawDialogMcp(): void {
    const name = dialogServer();
    if (name === null || !dialogEl.open) return;
    // Put back what a form making a server took away (`drawNewMcp`).
    mcpSaveEl.hidden = false;
    mcpLogHeadEl.hidden = false;
    mcpLogEl.hidden = false;
    const panel = mcpPanel;
    mcpFileEl.textContent = panel?.file ?? "";
    mcpFileErrorEl.textContent = panel?.error ?? "";
    const server = panel?.servers.find((each) => each.name === name);

    mcpStateEl.textContent = server ? mcpStateText(server.state) : "未登録";
    mcpStateEl.dataset.kind = server ? mcpStateKind(server.state) : "";
    // One button, named for what it will do: start what has not run, start again
    // what has, and stop what the file no longer lists. Nothing to do for an entry
    // that is neither listed nor running.
    mcpRestartEl.hidden = !server;
    if (server) {
      mcpRestartEl.textContent = !server.listed ? "停止" : server.state ? "再起動" : "起動";
    }
    mcpStaleEl.textContent = !server
      ? `設定ファイルにサーバ「${name}」はありません。このアカウントは、そのサーバが部屋で言ったことの話し手として残っています。`
      : !server.listed
        ? "設定ファイルにこのサーバはありません。停止しても、一覧から消えるのはアプリを起動し直したときです。"
        : server.stale
          ? "保存した設定は、再起動するまで反映されません。"
          : "";

    mcpFieldsEl.hidden = !server?.listed;
    if (server?.listed && !mcpFieldsEdited()) {
      mcpCommandEl.value = server.command;
      mcpArgsEl.value = server.args;
      mcpEnvEl.value = server.env;
      mcpDrawn = { name: server.name, command: server.command, args: server.args, env: server.env };
    }

    // Follow the tail while it is being followed: a log scrolled back up to read
    // is left where it was put.
    const following = mcpLogEl.scrollTop + mcpLogEl.clientHeight >= mcpLogEl.scrollHeight - 4;
    mcpLogEl.textContent = server?.log.length ? mcpLogText(server.log) : "（まだ何も出ていません）";
    if (following) mcpLogEl.scrollTop = mcpLogEl.scrollHeight;
  }

  /**
   * Draw the server section for a form making an account of kind `mcp` (#200).
   *
   * The three fields, empty, and nothing that answers for a server: there is none
   * yet to have a state, a log, a 保存 or a 再起動 of its own. 決定 writes it and
   * starts it (`createMcpAccount`). The file it will be written to is named, as
   * the section of an existing server names it.
   */
  function drawNewMcp(): void {
    mcpStateEl.textContent = "決定で設定ファイルに書き、起動します";
    mcpStateEl.dataset.kind = "";
    mcpRestartEl.hidden = true;
    mcpStaleEl.textContent = "";
    mcpFieldsEl.hidden = false;
    mcpSaveEl.hidden = true;
    mcpLogHeadEl.hidden = true;
    mcpLogEl.hidden = true;
    mcpFileEl.textContent = mcpPanel?.file ?? "";
    mcpFileErrorEl.textContent = mcpPanel?.error ?? "";
  }

  /** Write the open server's fields into the file. The running server is left as
   *  it is; the section then says the two differ until 再起動. */
  async function saveMcpServer(): Promise<boolean> {
    const name = dialogServer();
    if (name === null) return false;
    mcpErrorEl.textContent = "";
    try {
      await deps.invoke("save_mcp_server", {
        name,
        command: mcpCommandEl.value,
        args: mcpArgsEl.value,
        env: mcpEnvEl.value,
      });
    } catch (err) {
      mcpErrorEl.textContent = String(err);
      return false;
    }
    // Drawn again from the file, so what the fields hold is what was stored —
    // blank lines dropped, quotes taken off — rather than what was typed.
    mcpDrawn = null;
    await refreshMcpServers();
    return true;
  }

  /** Start the open server again from what the file holds. A field typed into
   *  and not saved is not what starts, so an unsaved edit is said instead. */
  async function restartMcpServer(): Promise<void> {
    const name = dialogServer();
    if (name === null) return;
    if (mcpFieldsEdited()) {
      mcpErrorEl.textContent = "保存していない変更があります。先に保存してください。";
      return;
    }
    mcpErrorEl.textContent = "";
    try {
      await deps.invoke("restart_mcp_server", { name });
    } catch (err) {
      mcpErrorEl.textContent = String(err);
      return;
    }
    await refreshMcpServers();
  }

  async function openMcpServersFile(): Promise<void> {
    try {
      await deps.invoke("open_mcp_servers_file");
    } catch (err) {
      mcpFileErrorEl.textContent = String(err);
    }
  }

  const accountNewEl = document.getElementById("account-new") as HTMLButtonElement;

  const dialogEl = document.getElementById("account-dialog") as HTMLDialogElement;

  const dialogFormEl = document.getElementById("account-form") as HTMLFormElement;

  const dialogTitleEl = document.getElementById("account-dialog-title") as HTMLElement;

  const dialogNameEl = document.getElementById("dialog-name") as HTMLInputElement;

  const dialogKindEl = document.getElementById("dialog-kind") as HTMLSelectElement;

  const dialogHueEl = document.getElementById("dialog-hue") as HTMLSelectElement;

  // The image field (#236): the circle as it will be drawn, and its two buttons.
  const dialogAvatarEl = document.getElementById("dialog-avatar") as HTMLElement;

  const dialogAvatarPickEl = document.getElementById("dialog-avatar-pick") as HTMLButtonElement;

  const dialogAvatarClearEl = document.getElementById("dialog-avatar-clear") as HTMLButtonElement;

  const dialogAvatarInputEl = document.getElementById("dialog-avatar-input") as HTMLInputElement;

  const dialogCwdEl = document.getElementById("dialog-cwd") as HTMLInputElement;

  const dialogCharacterEl = document.getElementById("dialog-character") as HTMLInputElement;

  // The character's body (#100): the file the three fields name, its place, and what is said about it.
  const dialogCharacterFileEl = document.getElementById("dialog-character-file") as HTMLElement;

  const dialogCharacterBodyEl = document.getElementById("dialog-character-body") as HTMLTextAreaElement;

  const dialogCharacterPathEl = document.getElementById("dialog-character-path") as HTMLElement;

  const dialogCharacterNoticeEl = document.getElementById("dialog-character-notice") as HTMLElement;

  const dialogCharacterReloadEl = document.getElementById("dialog-character-reload") as HTMLButtonElement;

  const dialogCodexAppServerEl = document.getElementById("dialog-codex-app-server") as HTMLInputElement;

  const dialogCodexAppServerFieldEl = document.getElementById("dialog-codex-app-server-field") as HTMLElement;

  const dialogCommandEl = document.getElementById("dialog-cli-command") as HTMLInputElement;

  const dialogOptionsEl = document.getElementById("dialog-options") as HTMLInputElement;

  const dialogResumeEl = document.getElementById("dialog-resume") as HTMLInputElement;

  const dialogResumeFieldEl = document.getElementById("dialog-resume-field") as HTMLElement;

  const dialogEnvEl = document.getElementById("dialog-env") as HTMLTextAreaElement;

  const dialogPreviewEl = document.getElementById("dialog-preview") as HTMLElement;

  const dialogNoticeEl = document.getElementById("dialog-notice") as HTMLElement;

  const dialogErrorEl = document.getElementById("dialog-error") as HTMLElement;

  const dialogDeleteEl = document.getElementById("dialog-delete") as HTMLButtonElement;

  const dialogCancelEl = document.getElementById("dialog-cancel") as HTMLButtonElement;

  const mcpOpenFileEl = document.getElementById("mcp-open-file") as HTMLButtonElement;

  const mcpFileEl = document.getElementById("mcp-file") as HTMLElement;

  const mcpFileErrorEl = document.getElementById("mcp-file-error") as HTMLElement;

  const dialogSectionTabEls = Array.from(
    dialogEl.querySelectorAll<HTMLButtonElement>("[data-section-tab]"),
  );

  const dialogSectionPaneEls = Array.from(dialogEl.querySelectorAll<HTMLElement>("[data-section]"));

  const dialogKindMcpEl = dialogKindEl.querySelector('option[value="mcp"]') as HTMLOptionElement;

  const mcpStateEl = document.getElementById("mcp-state") as HTMLElement;

  const mcpRestartEl = document.getElementById("mcp-restart") as HTMLButtonElement;

  const mcpStaleEl = document.getElementById("mcp-stale") as HTMLElement;

  const mcpFieldsEl = document.getElementById("mcp-fields") as HTMLElement;

  const mcpCommandEl = document.getElementById("mcp-command") as HTMLInputElement;

  const mcpArgsEl = document.getElementById("mcp-args") as HTMLTextAreaElement;

  const mcpEnvEl = document.getElementById("mcp-env") as HTMLTextAreaElement;

  const mcpErrorEl = document.getElementById("mcp-error") as HTMLElement;

  const mcpSaveEl = document.getElementById("mcp-save") as HTMLButtonElement;

  const mcpLogEl = document.getElementById("mcp-log") as HTMLElement;

  const mcpLogHeadEl = document.getElementById("mcp-log-head") as HTMLElement;
  function wireNewAccount(): void {
    accountNewEl.addEventListener("click", () => openAccountDialog(null));
  }
  function wireForm(): void {
    dialogKindEl.addEventListener("change", () => {
      if (dialogKindEl.value === "codex_cli" && dialogCommandEl.value === "claude") dialogCommandEl.value = "codex";
      if (dialogKindEl.value === "claude_code" && dialogCommandEl.value === "codex") dialogCommandEl.value = "claude";
      showDialogKind();
    });
    // The form's circle is drawn from the name and the colour above it (#236).
    dialogNameEl.addEventListener("input", () => drawDialogAvatar());
    dialogHueEl.addEventListener("change", () => drawDialogAvatar());
    dialogAvatarPickEl.addEventListener("click", () => {
      disarmDelete();
      dialogAvatarInputEl.click();
    });
    dialogAvatarInputEl.addEventListener("change", () => void pickDialogAvatar());
    dialogAvatarClearEl.addEventListener("click", () => {
      disarmDelete();
      resetDialogAvatar();
      dialogAvatar = { kind: "clear" };
      drawDialogAvatar();
    });
    dialogOptionsEl.addEventListener("input", () => refreshDialogLine());
    dialogCommandEl.addEventListener("input", () => refreshDialogLine());
    // The character ends up in the line that runs, so it redraws the preview for
    // the same reason the options do: the line shown has to be the line spawned.
    dialogCharacterEl.addEventListener("input", () => refreshDialogLine());
    // The name and the working directory place the character file (#100).
    dialogCharacterEl.addEventListener("input", () => scheduleCharacterFile());
    dialogCwdEl.addEventListener("input", () => scheduleCharacterFile());
    dialogCharacterReloadEl.addEventListener("click", () => void refreshCharacterFile(true));
    dialogCodexAppServerEl.addEventListener("change", () => refreshDialogLine());
    // So does the working directory: which registrations the line stops is read
    // out of the directory it is pointed at (#103).
    dialogCwdEl.addEventListener("input", () => refreshDialogLine());
    dialogEnvEl.addEventListener("input", () => refreshDialogLine());
    // The resume line is not in the preview — the preview answers for a fresh
    // launch — but it is a line that runs, and what it cannot carry is a topic
    // this account cannot go back into (#154, 決定4).
    dialogResumeEl.addEventListener("input", () => refreshDialogLine());
    // Anything but the second click of 削除 disarms it: an arm left standing is
    // one that an unrelated click fires later.
    for (const field of [
      dialogNameEl,
      dialogKindEl,
      dialogHueEl,
      dialogCwdEl,
      dialogCharacterEl,
      dialogOptionsEl,
      dialogResumeEl,
      dialogEnvEl,
      dialogCharacterBodyEl,
    ]) {
      field.addEventListener("input", () => disarmDelete());
    }
    dialogDeleteEl.addEventListener("click", () => void deleteFromDialog());
    for (const tab of dialogSectionTabEls) {
      tab.addEventListener("click", () => showDialogSection(tab.dataset.sectionTab as DialogSection));
      tab.addEventListener("keydown", stepDialogSection);
    }
    dialogCancelEl.addEventListener("click", () => closeAccountDialog());
    // Escape closes the dialog itself, and it means 取消: the draft is dropped by
    // the close handler below, so there is no path out of this form that leaves
    // half of it applied.
    dialogEl.addEventListener("close", () => {
      editing = null;
      draft = null;
      mcpDrawn = null;
      resetDialogAvatar();
      resetCharacterFile();
      disarmDelete();
    });
    dialogFormEl.addEventListener("submit", (event) => {
      // Always prevented: `method="dialog"` would close on submit, and the form
      // may not be decidable yet. The commit closes it once it has succeeded.
      event.preventDefault();
      void commitAccountDialog().then((done) => {
        if (done) closeAccountDialog();
      });
    });

  }
  async function wireMcp(): Promise<void> {
    mcpOpenFileEl.addEventListener("click", () => void openMcpServersFile());
    mcpSaveEl.addEventListener("click", () => void saveMcpServer());
    mcpRestartEl.addEventListener("click", () => void restartMcpServer());
    await deps.listen<string>("mcp-servers-changed", () => void refreshMcpServers());

  }
  function initializeHue(): void {
    deps.fillHues(dialogHueEl, null);
  }
  return { openAccountDialog, mcpServerOf, mcpNote, refreshMcpServers, wireNewAccount, wireForm, wireMcp, initializeHue };
}
