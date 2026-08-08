import {
  attachPane,
  createSession,
  closePane,
  aiCommandLine,
  editorContext,
  focusPane,
  openAiPane,
  paneCwd,
  paneShell,
  paneTitle,
  resizePane,
  splitPane,
  writePane,
  type PaneInfo,
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
export type LayoutSpec =
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

export interface SessionViewOptions {
  sessionId: number;
  name: string;
  containerEl: HTMLDivElement;
  shell: string;
  workspace: string | null;
  onChanged: () => void;
  onEmpty: () => void;
}

/** One session: its own layout tree, AI panes and backend session id. */
export class SessionView {
  readonly sessionId: number;
  name: string;
  root: Node | null = null;
  aiPaneIds = new Set<number>();
  containerEl: HTMLDivElement;
  private onChanged: () => void;
  private onEmpty: () => void;

  constructor(opts: SessionViewOptions) {
    this.sessionId = opts.sessionId;
    this.name = opts.name;
    this.containerEl = opts.containerEl;
    this.onChanged = opts.onChanged;
    this.onEmpty = opts.onEmpty;
  }

  /** Create a backend session with a single pane and wrap it in a view. */
  static async createFresh(opts: {
    name: string;
    shell: string;
    workspace: string | null;
    containerEl: HTMLDivElement;
    onChanged: () => void;
    onEmpty: () => void;
  }): Promise<SessionView> {
    const session = await createSession({
      name: opts.name,
      cols: 80,
      rows: 24,
      shell: opts.shell,
      cwd: opts.workspace ?? undefined,
    });
    const view = new SessionView({
      sessionId: session.sessionId,
      name: session.name || opts.name,
      containerEl: opts.containerEl,
      shell: opts.shell,
      workspace: opts.workspace,
      onChanged: opts.onChanged,
      onEmpty: opts.onEmpty,
    });
    let first: Node | null = null;
    for (const pane of session.panes) {
      const leaf = view.addLeaf(pane);
      if (!first) first = leaf;
    }
    view.root = first;
    for (const pane of session.panes) {
      await attachPane({ sessionId: view.sessionId, paneId: pane.paneId });
    }
    await view.focusLeaf(session.activePane);
    view.relayout();
    return view;
  }

  /** Recreate a session from a persisted layout spec. */
  static async restore(opts: {
    name: string;
    spec: LayoutSpec;
    shell: string;
    workspace: string | null;
    containerEl: HTMLDivElement;
    onChanged: () => void;
    onEmpty: () => void;
  }): Promise<SessionView> {
    const { name, spec, shell, workspace } = opts;
    const leaves: LayoutLeaf[] = [];
    const collect = (n: LayoutSpec): void => {
      if (n.kind === "leaf") leaves.push(n);
      else {
        collect(n.a);
        collect(n.b);
      }
    };
    collect(spec);
    if (leaves.length === 0) throw new Error("no leaves in spec");

    // First pane carries the root leaf's shell/cwd; every other pane is a
    // split (the backend ignores direction; the frontend owns geometry).
    const session = await createSession({
      name,
      cols: 80,
      rows: 24,
      shell: leaves[0].shell || shell,
      cwd: leaves[0].cwd ?? workspace ?? undefined,
    });
    const view = new SessionView({
      sessionId: session.sessionId,
      name: session.name || name,
      containerEl: opts.containerEl,
      shell,
      workspace,
      onChanged: opts.onChanged,
      onEmpty: opts.onEmpty,
    });
    const infos: PaneInfo[] = [session.panes[0]];

    const [aiProg, aiArgs] = await aiCommandLine();
    for (let i = 1; i < leaves.length; i++) {
      const leaf = leaves[i];
      const info = await splitPane({
        sessionId: view.sessionId,
        cols: 80,
        rows: 24,
        direction: "v",
        shell: leaf.ai ? undefined : leaf.shell || shell,
        program: leaf.ai ? aiProg : undefined,
        args: leaf.ai ? aiArgs : undefined,
        cwd: leaf.cwd ?? workspace ?? undefined,
        ai: leaf.ai,
      });
      infos.push(info);
      if (leaf.ai) view.aiPaneIds.add(info.paneId);
      await attachPane({ sessionId: view.sessionId, paneId: info.paneId });
    }

    view.root = null;
    let idx = 0;
    const build = (n: LayoutSpec): Node => {
      if (n.kind === "leaf") return view.addLeaf(infos[idx++]);
      const a = build(n.a);
      const b = build(n.b);
      const split = view.makeSplit(n.dir, a, b);
      split.ratio = n.ratio;
      view.applyRatio(split);
      return split;
    };
    const root = build(spec);
    view.root = root;
    if (!root.el.isConnected) {
      view.containerEl.append(root.el);
    }

    await attachPane({ sessionId: view.sessionId, paneId: session.panes[0].paneId });
    await view.focusLeaf(session.activePane);
    view.relayout();
    return view;
  }

  // ----- events -----

  handleOutput(paneId: number, data: string): void {
    this.findTerm(paneId)?.writeBase64(data);
  }

  // ----- tree helpers -----

  paneCount(): number {
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    return leaves.length;
  }

  agentCount(): number {
    let n = 0;
    for (const id of this.aiPaneIds) {
      if (this.findLeaf(id)) n++;
    }
    return n;
  }

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

  addLeaf(info: PaneInfo): Leaf {
    const { el, host, header } = mkLeafEl();
    const term = new PaneTerminal(this.sessionId, info.paneId, host);
    const titleEl = header.querySelector(".pane-title")! as HTMLSpanElement;
    titleEl.textContent = info.ai ? "ai" : info.shell;
    header.querySelector(".pane-idx")!.textContent = `#${info.paneId}`;
    header
      .querySelector(".pane-close")!
      .addEventListener("click", (e) => {
        e.stopPropagation();
        void this.closePaneById(info.paneId);
      });
    el.addEventListener("mousedown", () => void this.focusLeaf(info.paneId));
    const leaf: Leaf = { kind: "leaf", id: info.paneId, term, el, titleEl };
    this.containerEl.append(el);
    return leaf;
  }

  makeSplit(dir: "h" | "v", a: Node, b: Node): SplitNode {
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

  applyRatio(split: SplitNode): void {
    const { ratio } = split;
    split.aSlot.style.flex = `${ratio} 1 0`;
    split.bSlot.style.flex = `${1 - ratio} 1 0`;
  }

  private attachDivider(split: SplitNode): void {
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
        this.onChanged();
      };
      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    });
  }

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

  private rebuildSplitEl(split: SplitNode): void {
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
      this.containerEl.append(split.el);
      this.root = split;
    }

    await attachPane({ sessionId: this.sessionId, paneId: info.paneId });
    await this.focusLeaf(info.paneId);
    this.relayout();
    this.onChanged();
  }

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
      this.containerEl.append(split.el);
      this.root = split;
    }

    await attachPane({ sessionId: this.sessionId, paneId: info.paneId });
    this.aiPaneIds.add(info.paneId);
    await this.focusLeaf(info.paneId);
    this.relayout();
    this.onChanged();
  }

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

  async refocus(): Promise<void> {
    const leaf = this.focusedLeaf();
    if (leaf) leaf.term.focus();
  }

  async write(data: string): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    await writePane({ sessionId: this.sessionId, paneId: active.id }, data);
  }

  async copySelection(): Promise<boolean> {
    const active = this.focusedLeaf();
    if (!active) return false;
    const copied = await active.term.copySelection();
    return copied !== null;
  }

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

  searchFocused(query: string, backwards = false): boolean {
    const active = this.focusedLeaf();
    if (!active) return false;
    return backwards
      ? active.term.findPrevious(query)
      : active.term.findNext(query);
  }

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
    if (removed || this.paneCount() === 0) this.onEmpty();
    else this.onChanged();
  }

  async toggleZoom(): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);

    if (active.el.classList.contains("zoomed")) {
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
    requestAnimationFrame(() => this.relayout());
  }

  handlePaneClosed(paneId: number): void {
    const leaf = this.findLeaf(paneId);
    if (leaf) {
      leaf.term.dispose();
    }
    this.aiPaneIds.delete(paneId);
    this.containerEl.querySelectorAll(".pane").forEach((el) => {
      el.classList.remove("zoomed", "hidden");
    });
    this.root = this.removeLeafFromTree(this.root, paneId);
    if (this.root && !this.root.el.isConnected) {
      this.containerEl.append(this.root.el);
    }
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    if (leaves.length === 0) {
      this.containerEl.remove();
      this.onEmpty();
      return;
    }
    for (const l of leaves) {
      if (l.kind === "leaf") {
        void this.focusLeaf(l.id);
        break;
      }
    }
    this.relayout();
    this.onChanged();
  }

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

  /** Dispose every terminal and detach the session's DOM container. */
  dispose(): void {
    const leaves: Node[] = [];
    this.collectLeaves(this.root, leaves);
    for (const l of leaves) {
      if (l.kind === "leaf") l.term.dispose();
    }
    this.containerEl.remove();
    this.root = null;
  }

  // ----- layout serialization -----

  async serializeNode(n: Node): Promise<LayoutSpec> {
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

  // ----- dynamic pane titles -----

  async refreshTitles(): Promise<void> {
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
