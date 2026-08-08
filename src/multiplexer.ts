import {
  attachPane,
  createSession,
  closePane,
  aiCommandLine,
  editorContext,
  focusPane,
  defaultShell,
  loadLayout,
  onPaneClosed,
  onPaneOutput,
  openAiPane,
  paneCwd,
  paneShell,
  paneTitle,
  resizePane,
  saveLayout,
  splitPane,
  writePane,
  type PaneInfo,
  type SessionInfo,
} from "./api";
import { PaneTerminal } from "./terminal";
import { readText } from "@tauri-apps/plugin-clipboard-manager";

type Leaf = {
  kind: "leaf";
  id: number;
  term: PaneTerminal;
  el: HTMLDivElement;
  titleEl: HTMLSpanElement;
};

type Node =
  | Leaf
  | {
      kind: "split";
      dir: "h" | "v";
      a: Node;
      b: Node;
      el: HTMLDivElement;
      aSlot: HTMLDivElement;
      bSlot: HTMLDivElement;
      divider: HTMLDivElement;
      ratio: number;
    };

type SplitNode = Extract<Node, { kind: "split" }>;

/** Serializable layout node persisted to disk. */
type LayoutSpec =
  | { kind: "leaf"; shell: string; cwd: string | null; ai: boolean }
  | { kind: "split"; dir: "h" | "v"; ratio: number; a: LayoutSpec; b: LayoutSpec };

type LayoutLeaf = Extract<LayoutSpec, { kind: "leaf" }>;

function mkLeafEl(): { el: HTMLDivElement; host: HTMLDivElement; header: HTMLDivElement } {
  const el = document.createElement("div");
  el.className = "pane";

  const header = document.createElement("div");
  header.className = "pane-header";

  const close = document.createElement("button");
  close.className = "pane-close";
  close.type = "button";
  close.title = "close pane";
  close.textContent = "×";

  const title = document.createElement("span");
  title.className = "pane-title";
  const idx = document.createElement("span");
  idx.className = "pane-idx";
  header.append(close, title, idx);

  const host = document.createElement("div");
  host.className = "terminal-host";

  el.append(header, host);
  return { el, host, header };
}

function mkSlot(): HTMLDivElement {
  const el = document.createElement("div");
  el.className = "pane-slot";
  el.style.flex = "1 1 0";
  return el;
}

export class Multiplexer {
  private root: Node | null = null;
  private workspaceEl: HTMLDivElement;
  private sessionId = 0;
  private shell = "/bin/zsh";
  private workspace: string | null = null;
  private onSessionChange: (session: SessionInfo) => void;
  private aiPaneIds: Set<number> = new Set();
  private saveTimer: number | null = null;

  constructor(workspace: HTMLDivElement, onSessionChange: (s: SessionInfo) => void) {
    this.workspaceEl = workspace;
    this.onSessionChange = onSessionChange;
  }

  /** Root folder of the current workspace (all panes spawn relative to it). */
  setWorkspaceRoot(path: string | null): void {
    this.workspace = path;
  }

  getSessionId(): number {
    return this.sessionId;
  }

  async init(): Promise<void> {
    this.shell = await defaultShell();

    await onPaneOutput((evt) => {
      if (evt.sessionId !== this.sessionId) return;
      this.findTerm(evt.paneId)?.writeBase64(evt.data);
    });
    await onPaneClosed((evt) => {
      if (evt.sessionId !== this.sessionId) return;
      this.handlePaneClosed(evt.paneId);
    });

    const saved = await loadLayout();
    if (saved) {
      const restored = await this.restoreLayout(saved);
      if (restored) return;
    }

    // Fresh session (no layout or restore failed).
    const session = await createSession({
      name: "main",
      cols: 80,
      rows: 24,
      shell: this.shell,
      cwd: this.workspace ?? undefined,
    });
    this.sessionId = session.sessionId;
    this.root = null;

    let firstLeaf: Node | null = null;
    for (const pane of session.panes) {
      const leaf = this.addLeaf(pane);
      if (!firstLeaf) firstLeaf = leaf;
    }
    this.root = firstLeaf;

    // Attach PTY output streams only now that the terminal is in the tree,
    // so nothing (e.g. fish's DA query) is dropped before rendering starts.
    for (const pane of session.panes) {
      await attachPane({ sessionId: this.sessionId, paneId: pane.paneId });
    }

    await this.focusLeaf(session.activePane);
    this.onSessionChange(session);
    window.addEventListener("resize", () => this.relayout());
    this.relayout();
    this.startTitlePolling();
  }

  // ----- layout persistence -----

  /** Debounced auto-save: persist the layout ~400ms after the last change so
   *  closing the window never blocks on slow `lsof` IPC round-trips. */
  private scheduleSave(): void {
    if (this.saveTimer !== null) return;
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = null;
      void this.saveLayout();
    }, 400);
  }

  /** Serialize the current layout tree, annotating each leaf with its real
   *  working directory and whether it is an AI pane. */
  async saveLayout(): Promise<void> {
    if (!this.root) return;
    const spec = await this.serializeNode(this.root);
    await saveLayout(
      JSON.stringify({ version: 1, name: "main", root: spec }),
    );
  }

  private async serializeNode(n: Node): Promise<LayoutSpec> {
    if (n.kind === "leaf") {
      const ai = this.aiPaneIds.has(n.id);
      const cwd = await paneCwd({ sessionId: this.sessionId, paneId: n.id });
      const shell = await paneShell({ sessionId: this.sessionId, paneId: n.id });
      return { kind: "leaf", shell, cwd, ai };
    }
    const a = await this.serializeNode(n.a);
    const b = await this.serializeNode(n.b);
    return { kind: "split", dir: n.dir, ratio: n.ratio, a, b };
  }

  /** Restore a previously saved layout. Returns true on success. */
  private async restoreLayout(raw: string): Promise<boolean> {
    try {
      return await this.doRestoreLayout(raw);
    } catch (err) {
      console.warn("layout restore failed, starting fresh session", err);
      return false;
    }
  }

  private async doRestoreLayout(raw: string): Promise<boolean> {
    let parsed: { version?: number; name?: string; root: LayoutSpec };
    try {
      parsed = JSON.parse(raw);
    } catch {
      return false;
    }
    const spec = parsed.root;
    if (!spec) return false;

    const leaves: LayoutLeaf[] = [];
    const collect = (n: LayoutSpec): void => {
      if (n.kind === "leaf") leaves.push(n);
      else {
        collect(n.a);
        collect(n.b);
      }
    };
    collect(spec);
    if (leaves.length === 0) return false;

    // Spawn the first pane with the root leaf's shell/cwd; every other pane
    // is created as a split (the backend ignores direction; the frontend owns
    // geometry, so order doesn't matter for layout).
    const session = await createSession({
      name: parsed.name || "main",
      shell: leaves[0].shell || this.shell,
      cwd: leaves[0].cwd ?? this.workspace ?? undefined,
      cols: 80,
      rows: 24,
    });
    this.sessionId = session.sessionId;
    const infos: PaneInfo[] = [session.panes[0]];

    const [aiProg, aiArgs] = await aiCommandLine();
    for (let i = 1; i < leaves.length; i++) {
      const leaf = leaves[i];
      const info = await splitPane({
        sessionId: this.sessionId,
        cols: 80,
        rows: 24,
        direction: "v",
        shell: leaf.ai ? undefined : leaf.shell || this.shell,
        program: leaf.ai ? aiProg : undefined,
        args: leaf.ai ? aiArgs : undefined,
        cwd: leaf.cwd ?? this.workspace ?? undefined,
      });
      infos.push(info);
      if (leaf.ai) this.aiPaneIds.add(info.paneId);
      await attachPane({ sessionId: this.sessionId, paneId: info.paneId });
    }

    // Rebuild the DOM tree from the spec, consuming PaneInfos in leaf order.
    this.root = null;
    let idx = 0;
    const build = (n: LayoutSpec): Node => {
      if (n.kind === "leaf") {
        return this.addLeaf(infos[idx++]);
      }
      const a = build(n.a);
      const b = build(n.b);
      const split = this.makeSplit(n.dir, a, b);
      split.ratio = n.ratio;
      this.applyRatio(split);
      return split;
    };
    const root = build(spec);
    this.root = root;
    if (!root.el.isConnected) {
      this.workspaceEl.append(root.el);
    }

    await attachPane({ sessionId: this.sessionId, paneId: session.panes[0].paneId });
    await this.focusLeaf(session.activePane);
    this.onSessionChange(session);
    window.addEventListener("resize", () => this.relayout());
    this.relayout();
    this.startTitlePolling();
    return true;
  }

  // ----- tree helpers -----

  private findTerm(paneId: number): PaneTerminal | null {
    return this.findLeaf(paneId)?.term ?? null;
  }

  private findLeaf(paneId: number): Leaf | null {
    const walk = (n: Node): Leaf | null => {
      if (n.kind === "leaf") return n.id === paneId ? n : null;
      return walk(n.a) ?? walk(n.b);
    };
    return this.root ? walk(this.root) : null;
  }

  private collectLeaves(n: Node | null, out: Node[]): void {
    if (!n) return;
    if (n.kind === "leaf") {
      out.push(n);
      return;
    }
    this.collectLeaves(n.a, out);
    this.collectLeaves(n.b, out);
  }

  private focusedLeaf(): Leaf | null {
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    for (const l of leaves) {
      if (l.kind === "leaf" && l.el.classList.contains("focused")) return l;
    }
    for (const l of leaves) {
      if (l.kind === "leaf") return l;
    }
    return null;
  }

  // ----- construction -----

  private addLeaf(info: PaneInfo): Leaf {
    const { el, host, header } = mkLeafEl();
    const term = new PaneTerminal(this.sessionId, info.paneId, host);
    const titleEl = header.querySelector(".pane-title")! as HTMLSpanElement;
    titleEl.textContent = `${info.shell}`;
    header.querySelector(".pane-idx")!.textContent = `#${info.paneId}`;
    header
      .querySelector(".pane-close")!
      .addEventListener("click", (e) => {
        e.stopPropagation();
        void this.closePaneById(info.paneId);
      });
    el.addEventListener("mousedown", () => void this.focusLeaf(info.paneId));
    const leaf: Leaf = { kind: "leaf", id: info.paneId, term, el, titleEl };
    this.workspaceEl.append(el);
    return leaf;
  }
  /** Build a split node. The given `a` and `b` node elements are moved into slots. */
  private makeSplit(dir: "h" | "v", a: Node, b: Node): SplitNode {
    const el = document.createElement("div");
    el.className = dir === "h" ? "split-h" : "split-v";

    const aSlot = mkSlot();
    const bSlot = mkSlot();
    const divider = document.createElement("div");
    divider.className = "divider";

    aSlot.append(a.el);
    bSlot.append(b.el);

    el.append(aSlot, divider, bSlot);

    const split: Node = { kind: "split", dir, a, b, el, aSlot, bSlot, divider, ratio: 0.5 };
    this.applyRatio(split);
    this.attachDivider(split);
    return split;
  }

  private applyRatio(split: Extract<Node, { kind: "split" }>): void {
    const { ratio } = split;
    if (split.dir === "h") {
      split.aSlot.style.flex = `${ratio} 1 0`;
      split.bSlot.style.flex = `${1 - ratio} 1 0`;
    } else {
      split.aSlot.style.flex = `${ratio} 1 0`;
      split.bSlot.style.flex = `${1 - ratio} 1 0`;
    }
  }

  private attachDivider(split: Extract<Node, { kind: "split" }>): void {
    split.divider.addEventListener("mousedown", (e) => {
      e.preventDefault();
      const startX = e.clientX;
      const startY = e.clientY;
      const startRatio = split.ratio;
      const total = split.dir === "h" ? split.el.clientWidth : split.el.clientHeight;
      split.divider.classList.add("dragging");

      const onMove = (ev: MouseEvent) => {
        const delta = split.dir === "h" ? ev.clientX - startX : ev.clientY - startY;
        const min = 0.15;
        split.ratio = Math.min(0.85, Math.max(min, startRatio + delta / total));
        this.applyRatio(split);
      };
      const onUp = () => {
        split.divider.classList.remove("dragging");
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
        this.relayout();
        this.scheduleSave();
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    });
  }

  /** Replace a leaf in the tree with a new node. */
  private replaceInTree(n: Node, oldLeaf: Leaf, newNode: Node): boolean {
    if (n.kind === "split") {
      if (n.a.kind === "leaf" && n.a === oldLeaf) {
        n.a = newNode;
        this.rebuildSplitEl(n);
        return true;
      }
      if (n.b.kind === "leaf" && n.b === oldLeaf) {
        n.b = newNode;
        this.rebuildSplitEl(n);
        return true;
      }
      return this.replaceInTree(n.a, oldLeaf, newNode) || this.replaceInTree(n.b, oldLeaf, newNode);
    }
    return false;
  }

  /** Detach leaf's element from wherever it is and place it into the split slot. */
  private rebuildSplitEl(split: Extract<Node, { kind: "split" }>): void {
    split.aSlot.innerHTML = "";
    split.bSlot.innerHTML = "";
    split.aSlot.append(split.a.el);
    split.bSlot.append(split.b.el);
    this.applyRatio(split);
  }

  // ----- public actions -----

  async splitActive(dir: "h" | "v"): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;

    const cwd = (await paneCwd({ sessionId: this.sessionId, paneId: active.id })) ?? undefined;
    const info = await splitPane({
      sessionId: this.sessionId,
      cols: active.term.cols(),
      rows: active.term.rows(),
      direction: dir,
      cwd,
    });

    const newLeaf = this.addLeaf(info);
    const split = this.makeSplit(dir, active, newLeaf);

    if (!this.replaceInTree(this.root!, active, split)) {
      // Active was the root itself: makeSplit already re-parented it into
      // split.aSlot, so the split element becomes the new workspace root.
      this.workspaceEl.append(split.el);
      this.root = split;
    }

    await attachPane({ sessionId: this.sessionId, paneId: info.paneId });
    await this.focusLeaf(info.paneId);
    this.relayout();
    this.scheduleSave();
  }

  /** Open an AI CLI pane (opencode/claude/...) splitting the active pane. */
  async openAiPane(): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;

    const info = await openAiPane({
      sessionId: this.sessionId,
      cols: active.term.cols(),
      rows: active.term.rows(),
    });

    const newLeaf = this.addLeaf(info);
    const split = this.makeSplit("h", active, newLeaf);

    if (!this.replaceInTree(this.root!, active, split)) {
      this.workspaceEl.append(split.el);
      this.root = split;
    }

    await attachPane({ sessionId: this.sessionId, paneId: info.paneId });
    this.aiPaneIds.add(info.paneId);
    await this.focusLeaf(info.paneId);
    this.relayout();
    this.scheduleSave();
  }

  /**
   * Send context from the focused pane to the AI pane. Prefers, in order:
   * the xterm mouse selection, then vim's visual-mode selection, then a
   * `@file:line:col` reference when a fullscreen editor is active, and falls
   * back to a scrollback paste.
   * Returns the number of characters sent, or 0 if there is no AI pane.
   */
  async sendContextToAi(): Promise<number> {
    const active = this.focusedLeaf();
    if (!active) return 0;

    let aiPaneId: number | null = null;
    for (const id of this.aiPaneIds) {
      if (this.findLeaf(id)) {
        aiPaneId = id;
        break;
      }
    }
    if (aiPaneId === null) return 0;

    let payload: string;
    const mouse = active.term.mouseSelection();
    if (mouse) {
      payload = `\x1b[200~[selection from pane #${active.id}]\n\n${mouse}\x1b[201~`;
    } else if (active.term.inAlternateScreen()) {
      const visual = active.term.visualSelection();
      if (visual) {
        payload = `\x1b[200~[visual selection from pane #${active.id}]\n\n${visual}\x1b[201~`;
      } else {
        const ctx = await editorContext({
          sessionId: this.sessionId,
          paneId: active.id,
        });
        if (ctx?.file) {
          const pos = active.term.statusPosition();
          const ref = pos ? `${ctx.file}:${pos.line}:${pos.col}` : ctx.file;
          payload = `\x1b[200~[editing ${ctx.editor}: ${ref}]\n\n@${ref}\x1b[201~`;
        } else {
          payload = this.scrollbackPayload(active);
        }
      }
    } else {
      payload = this.scrollbackPayload(active);
    }

    await writePane(
      { sessionId: this.sessionId, paneId: aiPaneId },
      payload,
    );
    await this.focusLeaf(aiPaneId);
    return payload.length;
  }

  private scrollbackPayload(active: Leaf): string {
    const context = active.term.contextText(4000);
    // Bracketed paste so the AI TUI treats it as a single paste.
    return `\x1b[200~[context from pane #${active.id}]\n\n${context}\x1b[201~`;
  }

  async focusLeaf(paneId: number): Promise<void> {
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    for (const l of leaves) {
      if (l.kind !== "leaf") continue;
      const focused = l.id === paneId;
      l.el.classList.toggle("focused", focused);
      if (focused) l.term.focus();
    }
    await focusPane({ sessionId: this.sessionId, paneId });
  }

  /** Refocus the currently focused pane (used after closing the search bar). */
  async refocus(): Promise<void> {
    const leaf = this.focusedLeaf();
    if (leaf) leaf.term.focus();
  }

  async write(data: string): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    await writePane({ sessionId: this.sessionId, paneId: active.id }, data);
  }

  /** Copy the focused pane's selection to the OS clipboard. */
  async copySelection(): Promise<boolean> {
    const active = this.focusedLeaf();
    if (!active) return false;
    const copied = await active.term.copySelection();
    return copied !== null;
  }

  /** Paste the OS clipboard into the focused pane using bracketed paste. */
  async pasteClipboard(): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    const text = await readText();
    if (!text) return;
    await writePane(
      { sessionId: this.sessionId, paneId: active.id },
      `\x1b[200~${text}\x1b[201~`,
    );
  }

  /** Focus a pane and run a search query over its scrollback. */
  searchFocused(query: string, backwards = false): boolean {
    const active = this.focusedLeaf();
    if (!active) return false;
    return backwards
      ? active.term.findPrevious(query)
      : active.term.findNext(query);
  }

  /** Clear search highlights on the focused pane. */
  clearSearchFocused(): void {
    const active = this.focusedLeaf();
    if (!active) return;
    active.term.clearSearch();
  }

  async closeActive(): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    await this.closePaneById(active.id);
  }

  async closePaneById(paneId: number): Promise<void> {
    const removed = await closePane({ sessionId: this.sessionId, paneId });
    this.handlePaneClosed(paneId);
    if (removed) {
      this.root = null;
      this.onSessionChange({ sessionId: this.sessionId, name: "", panes: [], activePane: 0 });
    }
    this.scheduleSave();
  }

  /** Toggle zoom of the focused pane (fills the whole workspace). */
  async toggleZoom(): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);

    if (active.el.classList.contains("zoomed")) {
      // unzoom
      for (const l of leaves) {
        if (l.kind !== "leaf") continue;
        l.el.classList.remove("zoomed");
      }
      this.relayout();
      return;
    }

    for (const l of leaves) {
      if (l.kind !== "leaf") continue;
      l.el.classList.add("zoomed", l.id !== active.id ? "hidden" : "");
      if (l.id === active.id) l.el.classList.remove("hidden");
    }
    // Wait a frame so the zoomed pane takes full size, then fit.
    requestAnimationFrame(() => this.relayout());
  }

  private handlePaneClosed(paneId: number): void {
    const leaf = this.findLeaf(paneId);
    if (leaf) {
      leaf.term.dispose();
    }
    this.aiPaneIds.delete(paneId);
    // Clear any zoom state from the whole workspace so promoted siblings
    // don't stay hidden/zoomed after a pane is removed.
    this.workspaceEl.querySelectorAll(".pane").forEach((el) => {
      el.classList.remove("zoomed", "hidden");
    });
    // Remove the leaf from the tree (collapsing splits) and the DOM.
    this.root = this.removeLeafFromTree(this.root, paneId);
    // The surviving node was inside the removed split's slot; re-attach it.
    if (this.root && !this.root.el.isConnected) {
      this.workspaceEl.append(this.root.el);
    }
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    for (const l of leaves) {
      if (l.kind === "leaf") {
        void this.focusLeaf(l.id);
        break;
      }
    }
    this.relayout();
  }

  /**
   * Remove a leaf from the tree, collapsing any split that loses a child.
   * Removed split/leaf elements are detached from the DOM; the surviving
   * child is promoted to the parent's position.
   */
  private removeLeafFromTree(n: Node | null, paneId: number): Node | null {
    if (!n) return null;
    if (n.kind === "leaf") {
      if (n.id === paneId) {
        n.el.remove();
        return null;
      }
      return n;
    }
    const a = this.removeLeafFromTree(n.a, paneId);
    const b = this.removeLeafFromTree(n.b, paneId);
    if (a && b) {
      n.a = a;
      n.b = b;
      this.rebuildSplitEl(n);
      return n;
    }
    n.el.remove();
    if (a) return a;
    if (b) return b;
    return null;
  }

  relayout(): void {
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    for (const l of leaves) {
      if (l.kind !== "leaf") continue;
      l.term.fitNow();
      void resizePane({
        sessionId: this.sessionId,
        paneId: l.id,
        cols: l.term.cols(),
        rows: l.term.rows(),
      });
    }
  }

  // ----- dynamic pane titles -----

  /** Poll the active process of every pane and update its title (tmux-style:
   *  `vim: main.rs` while editing, otherwise the running program). Also
   *  re-sync the app chrome to the colorscheme of the focused editor. */
  startTitlePolling(): void {
    window.setInterval(() => void this.refreshTitles(), 1000);
  }

  private async refreshTitles(): Promise<void> {
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    for (const l of leaves) {
      if (l.kind !== "leaf") continue;
      const title = await paneTitle({ sessionId: this.sessionId, paneId: l.id });
      if (title && title !== l.titleEl.textContent) {
        l.titleEl.textContent = title;
      }
    }
  }
}
