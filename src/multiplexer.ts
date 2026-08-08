import {
  closeSession,
  defaultShell,
  getWorkspace,
  gitCreateWorktree,
  gitRemoveWorktree,
  gitStatus,
  loadLayout,
  onPaneClosed,
  onPaneOutput,
  saveLayout,
  type GitStatus,
  type SessionInfo,
} from "./api";
import { SessionView, type LayoutSpec } from "./session-view";

export interface SessionTab {
  id: number;
  name: string;
  agentCount: number;
  /** Git branch this session is checked out on (worktree sessions). */
  branch: string | null;
}

export interface SessionList {
  activeId: number;
  activeName: string;
  sessions: SessionTab[];
}

export interface GitPanelData {
  status: GitStatus | null;
  error: string | null;
}

/**
 * Multi-session manager. Owns the backend session lifecycle, a tab bar,
 * event routing (pane-output/pane-closed) and the git panel. All per-tree
 * logic lives in {@link SessionView}.
 */
export class Multiplexer {
  private workspaceEl: HTMLDivElement;
  private sessions = new Map<number, SessionView>();
  private activeId = 0;
  private shell = "/bin/zsh";
  private workspace: string | null = null;
  private onSessionChange: (s: SessionInfo) => void;
  private onSessionsChange: (s: SessionList) => void;
  private onGitPanel: (d: GitPanelData) => void;
  private saveTimer: number | null = null;

  constructor(
    workspace: HTMLDivElement,
    onSessionChange: (s: SessionInfo) => void,
    onSessionsChange: (s: SessionList) => void,
    onGitPanel: (d: GitPanelData) => void,
  ) {
    this.workspaceEl = workspace;
    this.onSessionChange = onSessionChange;
    this.onSessionsChange = onSessionsChange;
    this.onGitPanel = onGitPanel;
  }

  setWorkspaceRoot(path: string | null): void {
    this.workspace = path;
  }

  getActiveSessionId(): number {
    return this.activeId;
  }

  async init(): Promise<void> {
    this.shell = await defaultShell();
    if (this.workspace === null) {
      this.workspace = await getWorkspace();
    }

    await onPaneOutput((evt) => {
      this.sessions.get(evt.sessionId)?.handleOutput(evt.paneId, evt.data);
    });
    await onPaneClosed((evt) => {
      this.sessions.get(evt.sessionId)?.handlePaneClosed(evt.paneId);
    });

    const saved = await loadLayout();
    if (saved) {
      const restored = await this.restoreLayout(saved);
      if (restored) return;
    }

    await this.createSession("main");

    void this.refreshGit();
    window.addEventListener("resize", () => this.relayout());
    this.startTitlePolling();
  }

  // ----- session lifecycle -----

  /**
   * Create a new session (fresh single pane) and make it active. The first
   * session uses the workspace root; every further session gets its own git
   * worktree (`<workspace>.worktrees/<name>`) so it is fully independent.
   */
  async createSession(name?: string): Promise<void> {
    const sessionName = name ?? `session-${this.sessions.size + 1}`;
    let spawnRoot: string | null = this.workspace;
    let worktree: string | null = null;
    if (this.sessions.size > 0 && this.workspace) {
      try {
        spawnRoot = await gitCreateWorktree(sessionName);
        worktree = sessionName;
      } catch (err) {
        console.warn(`worktree create failed for ${sessionName}, using workspace root`, err);
        spawnRoot = this.workspace;
        worktree = null;
      }
    }

    const container = this.mkSessionContainer();
    let viewRef: SessionView | null = null;
    const onChanged = () => this.scheduleSave();
    const onEmpty = () => {
      if (viewRef) this.handleSessionRemoved(viewRef.sessionId);
    };
    viewRef = await SessionView.createFresh({
      name: sessionName,
      shell: this.shell,
      workspace: spawnRoot,
      worktree,
      containerEl: container,
      onChanged,
      onEmpty,
    });
    this.sessions.set(viewRef.sessionId, viewRef);
    this.activeId = viewRef.sessionId;
    this.updateVisibility();
    this.emitSessionChange(viewRef);
    this.emitSessions();
    this.relayout();
  }

  /** Switch the active session to the given id. */
  async switchSession(sessionId: number): Promise<void> {
    const view = this.sessions.get(sessionId);
    if (!view) return;
    this.activeId = sessionId;
    this.updateVisibility();
    this.emitSessionChange(view);
    this.emitSessions();
    this.relayout();
  }

  async nextSession(): Promise<void> {
    const ids = [...this.sessions.keys()];
    if (ids.length < 2) return;
    const i = ids.indexOf(this.activeId);
    await this.switchSession(ids[(i + 1) % ids.length]);
  }

  async prevSession(): Promise<void> {
    const ids = [...this.sessions.keys()];
    if (ids.length < 2) return;
    const i = ids.indexOf(this.activeId);
    await this.switchSession(ids[(i - 1 + ids.length) % ids.length]);
  }

  async closeSession(sessionId: number): Promise<void> {
    const view = this.sessions.get(sessionId);
    if (!view) return;
    await closeSession(sessionId);
    this.handleSessionRemoved(sessionId);
  }

  /** Remove a session from the manager after its panes are gone. */
  private handleSessionRemoved(sessionId: number): void {
    const view = this.sessions.get(sessionId);
    if (!view) return;
    const worktree = view.worktree;
    view.dispose();
    this.sessions.delete(sessionId);

    if (worktree) {
      void gitRemoveWorktree(worktree).catch((err) =>
        console.warn(`worktree remove failed for ${worktree}`, err),
      );
    }

    if (this.sessions.size === 0) {
      void this.createSession();
      return;
    }
    if (this.activeId === sessionId) {
      this.activeId = this.sessions.keys().next().value!;
    }
    this.updateVisibility();
    const active = this.sessions.get(this.activeId)!;
    this.emitSessionChange(active);
    this.emitSessions();
    this.relayout();
    this.scheduleSave();
  }

  private mkSessionContainer(): HTMLDivElement {
    const el = document.createElement("div");
    el.className = "session-view";
    this.workspaceEl.append(el);
    return el;
  }

  private activeView(): SessionView | null {
    return this.sessions.get(this.activeId) ?? null;
  }

  // ----- git panel -----

  /** Re-fetch the workspace git status and render it into the sidebar. */
  async refreshGit(): Promise<void> {
    try {
      const status = await gitStatus();
      this.onGitPanel({ status, error: null });
    } catch (err) {
      this.onGitPanel({ status: null, error: String(err) });
    }
  }

  private updateVisibility(): void {
    for (const [id, view] of this.sessions) {
      view.containerEl.classList.toggle("hidden", id !== this.activeId);
    }
  }

  // ----- delegation to active session -----

  splitActive(dir: "h" | "v"): Promise<void> {
    return this.activeView()?.splitActive(dir) ?? Promise.resolve();
  }

  openAiPane(): Promise<void> {
    return this.activeView()?.openAiPane() ?? Promise.resolve();
  }

  sendContextToAi(): Promise<number> {
    return this.activeView()?.sendContextToAi() ?? Promise.resolve(0);
  }

  focusLeaf(paneId: number): Promise<void> {
    return this.activeView()?.focusLeaf(paneId) ?? Promise.resolve();
  }

  refocus(): Promise<void> {
    return this.activeView()?.refocus() ?? Promise.resolve();
  }

  write(data: string): Promise<void> {
    return this.activeView()?.write(data) ?? Promise.resolve();
  }

  copySelection(): Promise<boolean> {
    return this.activeView()?.copySelection() ?? Promise.resolve(false);
  }

  pasteClipboard(): Promise<void> {
    return this.activeView()?.pasteClipboard() ?? Promise.resolve();
  }

  searchFocused(query: string, backwards = false): boolean {
    return this.activeView()?.searchFocused(query, backwards) ?? false;
  }

  clearSearchFocused(): void {
    this.activeView()?.clearSearchFocused();
  }

  closeActive(): Promise<void> {
    return this.activeView()?.closeActive() ?? Promise.resolve();
  }

  toggleZoom(): Promise<void> {
    return this.activeView()?.toggleZoom() ?? Promise.resolve();
  }

  relayout(): void {
    this.activeView()?.relayout();
  }

  // ----- layout persistence -----

  private scheduleSave(): void {
    if (this.saveTimer !== null) return;
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = null;
      void this.saveLayout();
    }, 400);
  }

  /** Serialize every session into the v2 multi-session layout format. */
  async saveLayout(): Promise<void> {
    const sessions: { name: string; worktree: string | null; root: LayoutSpec }[] = [];
    for (const view of this.sessions.values()) {
      if (!view.root) continue;
      const root = await view.serializeNode(view.root);
      sessions.push({ name: view.name, worktree: view.worktree, root });
    }
    await saveLayout(JSON.stringify({ version: 2, sessions }));
  }

  private async restoreLayout(raw: string): Promise<boolean> {
    try {
      return await this.doRestoreLayout(raw);
    } catch (err) {
      console.warn("layout restore failed, starting fresh session", err);
      return false;
    }
  }

  private async doRestoreLayout(raw: string): Promise<boolean> {
    let parsed: {
      version?: number;
      name?: string;
      root?: LayoutSpec;
      sessions?: { name: string; worktree?: string | null; root: LayoutSpec }[];
    };
    try {
      parsed = JSON.parse(raw);
    } catch {
      return false;
    }

    // v2 multi-session format.
    if (parsed.version === 2 && Array.isArray(parsed.sessions)) {
      if (parsed.sessions.length === 0) return false;
      for (const spec of parsed.sessions) {
        const container = this.mkSessionContainer();
        let viewRef: SessionView | null = null;
        const onChanged = () => this.scheduleSave();
        const onEmpty = () => {
          if (viewRef) this.handleSessionRemoved(viewRef.sessionId);
        };
        const spawnRoot = await this.sessionSpawnRoot(spec.worktree ?? null);
        viewRef = await SessionView.restore({
          name: spec.name || `session-${this.sessions.size + 1}`,
          spec: spec.root,
          shell: this.shell,
          workspace: spawnRoot,
          worktree: spec.worktree ?? null,
          containerEl: container,
          onChanged,
          onEmpty,
        });
        this.sessions.set(viewRef.sessionId, viewRef);
      }
      this.activeId = this.sessions.keys().next().value!;
      this.updateVisibility();
      const active = this.sessions.get(this.activeId)!;
      this.emitSessionChange(active);
      this.emitSessions();
      void this.refreshGit();
      window.addEventListener("resize", () => this.relayout());
      this.startTitlePolling();
      return true;
    }

    // v1 / legacy single-session format.
    const rootSpec = parsed.root;
    if (!rootSpec) return false;
    const container = this.mkSessionContainer();
    let viewRef: SessionView | null = null;
    const onChanged = () => this.scheduleSave();
    const onEmpty = () => {
      if (viewRef) this.handleSessionRemoved(viewRef.sessionId);
    };
    viewRef = await SessionView.restore({
      name: parsed.name || "main",
      spec: rootSpec,
      shell: this.shell,
      workspace: this.workspace,
      worktree: null,
      containerEl: container,
      onChanged,
      onEmpty,
    });
    this.sessions.set(viewRef.sessionId, viewRef);
    this.activeId = viewRef.sessionId;
    this.updateVisibility();
    this.emitSessionChange(viewRef);
    this.emitSessions();
    void this.refreshGit();
    window.addEventListener("resize", () => this.relayout());
    this.startTitlePolling();
    return true;
  }

  /**
   * Resolve the spawn root for a restored session: if it owned a worktree,
   * re-attach (or re-create) it so the session's panes land in the right
   * directory. Falls back to the workspace root on any git error.
   */
  private async sessionSpawnRoot(worktree: string | null): Promise<string | null> {
    if (worktree && this.workspace) {
      try {
        return await gitCreateWorktree(worktree);
      } catch (err) {
        console.warn(`worktree restore failed for ${worktree}, using workspace root`, err);
      }
    }
    return this.workspace;
  }

  // ----- titles -----

  startTitlePolling(): void {
    window.setInterval(() => {
      this.activeView()?.refreshTitles();
    }, 1000);
  }

  // ----- callbacks -----

  private emitSessionChange(view: SessionView): void {
    this.onSessionChange({
      sessionId: view.sessionId,
      name: view.name,
      panes: new Array(view.paneCount()),
      activePane: 0,
    });
  }

  private emitSessions(): void {
    const sessions: SessionTab[] = [];
    for (const [id, view] of this.sessions) {
      sessions.push({
        id,
        name: view.name,
        agentCount: view.agentCount(),
        branch: view.worktree ? `nmx/${view.worktree}` : null,
      });
    }
    this.onSessionsChange({ activeId: this.activeId, activeName: this.sessions.get(this.activeId)?.name ?? "", sessions });
  }
}
