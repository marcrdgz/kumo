import {
  attachPane,
  createSession,
  closePane,
  focusPane,
  defaultShell,
  onPaneClosed,
  onPaneOutput,
  resizePane,
  splitPane,
  writePane,
  type PaneInfo,
  type SessionInfo,
} from "./api";
import { PaneTerminal } from "./terminal";

type Leaf = { kind: "leaf"; id: number; term: PaneTerminal; el: HTMLDivElement };

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

function mkLeafEl(): { el: HTMLDivElement; host: HTMLDivElement; header: HTMLDivElement } {
  const el = document.createElement("div");
  el.className = "pane";

  const header = document.createElement("div");
  header.className = "pane-header";
  const title = document.createElement("span");
  title.className = "pane-title";
  const idx = document.createElement("span");
  idx.className = "pane-idx";
  header.append(title, idx);

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
  private workspace: HTMLDivElement;
  private sessionId = 0;
  private shell = "/bin/zsh";
  private onSessionChange: (session: SessionInfo) => void;

  constructor(workspace: HTMLDivElement, onSessionChange: (s: SessionInfo) => void) {
    this.workspace = workspace;
    this.onSessionChange = onSessionChange;
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

    // Start the PTY at xterm's default size (80x24). The terminal will fit
    // immediately and relayout() will resize the PTY to match the real cell
    // dimensions, avoiding prompt-wrap artifacts.
    const session = await createSession({
      name: "main",
      cols: 80,
      rows: 24,
      shell: this.shell,
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
    header.querySelector(".pane-title")!.textContent = `${info.shell}`;
    header.querySelector(".pane-idx")!.textContent = `#${info.paneId}`;
    el.addEventListener("mousedown", () => void this.focusLeaf(info.paneId));
    const leaf: Leaf = { kind: "leaf", id: info.paneId, term, el };
    this.workspace.append(el);
    return leaf;
  }

  /** Build a split node. The given `a` and `b` node elements are moved into slots. */
  private makeSplit(dir: "h" | "v", a: Node, b: Node): Node {
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

    const info = await splitPane({
      sessionId: this.sessionId,
      cols: active.term.cols(),
      rows: active.term.rows(),
      direction: dir,
    });

    const newLeaf = this.addLeaf(info);
    const split = this.makeSplit(dir, active, newLeaf);

    if (!this.replaceInTree(this.root!, active, split)) {
      // Active was the root itself: makeSplit already re-parented it into
      // split.aSlot, so the split element becomes the new workspace root.
      this.workspace.append(split.el);
      this.root = split;
    }

    await attachPane({ sessionId: this.sessionId, paneId: info.paneId });
    await this.focusLeaf(info.paneId);
    this.relayout();
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

  async write(data: string): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    await writePane({ sessionId: this.sessionId, paneId: active.id }, data);
  }

  async closeActive(): Promise<void> {
    const active = this.focusedLeaf();
    if (!active) return;
    const removed = await closePane({ sessionId: this.sessionId, paneId: active.id });
    this.handlePaneClosed(active.id);
    if (removed) {
      this.root = null;
      this.onSessionChange({ sessionId: this.sessionId, name: "", panes: [], activePane: 0 });
    }
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
      leaf.el.remove();
    }
    // Clear any zoom state from the whole workspace so promoted siblings
    // don't stay hidden/zoomed after a pane is removed.
    this.workspace.querySelectorAll(".pane").forEach((el) => {
      el.classList.remove("zoomed", "hidden");
    });
    this.collapseTree();
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

  /** Remove splits that lost a child (promote the surviving child). */
  private collapseTree(): void {
    const collapse = (n: Node): Node | null => {
      if (n.kind === "leaf") return n;
      const a = collapse(n.a);
      const b = collapse(n.b);
      if (a && !b) {
        n.el.remove();
        return a;
      }
      if (!a && b) {
        n.el.remove();
        return b;
      }
      if (!a || !b) return null;
      n.a = a;
      n.b = b;
      this.rebuildSplitEl(n);
      return n;
    };
    this.root = this.root ? collapse(this.root) : null;
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
}
