import { Multiplexer } from "./multiplexer";
import {
  aiCommand,
  getRecentWorkspaces,
  getWorkspace,
  setWorkspace,
  gitDiff,
  type GitStatus,
} from "./api";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import "@xterm/xterm/css/xterm.css";

const statusLeft = document.getElementById("status-left")!;
const statusRight = document.getElementById("status-right")!;
const workspace = document.getElementById("workspace") as HTMLDivElement;
const tabbar = document.getElementById("tabs") as HTMLDivElement;
const gitView = document.getElementById("git-view") as HTMLDivElement;
const gitBranch = document.getElementById("git-branch")!;
const gitMeta = document.getElementById("git-meta")!;
const gitBody = document.getElementById("git-body")!;
const searchBar = document.getElementById("search-bar") as HTMLDivElement;
const searchInput = document.getElementById("search-input") as HTMLInputElement;

let aiCmd = "ai";
void aiCommand().then((c) => {
  aiCmd = c;
});

let currentWorkspace: string | null = null;

function basename(path: string): string {
  return path.split("/").filter(Boolean).pop() || path;
}

function renderStatus(): void {
  if (currentWorkspace) {
    statusLeft.innerHTML = `<span class="mode">●</span> <span class="session-name">${basename(currentWorkspace)}</span>`;
  } else {
    statusLeft.innerHTML = `<span class="mode">●</span> <span class="session-name">${currentSessionName}</span>`;
  }
  const tabs = currentSessions > 1 ? `${currentSessions} tabs · ` : "";
  statusRight.textContent = `${tabs}${panesLabel()}`;
}

function panesLabel(): string {
  return `${currentPanes} panes${currentWorkspace ? "" : ` · ${currentSessionName}`}`;
}

let currentPanes = 0;
let currentSessionName = "";
let currentSessions = 0;

const mux = new Multiplexer(
  workspace,
  tabbar,
  gitView,
  (s: { sessionId: number; name: string; panes: unknown[]; activePane: number }) => {
    currentPanes = s.panes.length;
    currentSessionName = s.name;
    renderStatus();
  },
  (list) => {
    currentSessions = list.sessions.length;
    renderTabs(list);
  },
  renderGitPanel,
);

// ----- session tabs -----

function renderTabs(list: { activeId: number; sessions: { id: number; name: string; agentCount: number }[] }): void {
  tabbar.innerHTML = "";
  for (const s of list.sessions) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "tab" + (s.id === list.activeId ? " active" : "");
    btn.title = s.name;
    const label = document.createElement("span");
    label.textContent = s.name;
    btn.append(label);
    if (s.agentCount > 0) {
      const badge = document.createElement("span");
      badge.className = "tab-badge";
      badge.textContent = `⚡${s.agentCount}`;
      btn.append(badge);
    }
    btn.addEventListener("click", () => void mux.switchSession(s.id));
    tabbar.append(btn);
  }

  const gitBtn = document.createElement("button");
  gitBtn.type = "button";
  gitBtn.className = "tab git-tab" + (mux.isGitActive() ? " active" : "");
  gitBtn.title = "git changes";
  gitBtn.textContent = "git";
  gitBtn.addEventListener("click", () => void mux.toggleGit());
  tabbar.append(gitBtn);
}

// ----- git panel -----

const STATUS_ICON: Record<string, string> = {
  "??": "?",
  M: "M",
  A: "A",
  D: "D",
  R: "R",
  C: "C",
  U: "U",
};

function gitStatusSymbol(status: string): string {
  return STATUS_ICON[status] ?? status.slice(0, 1);
}

function renderGitPanel(data: { status: GitStatus | null; error: string | null }): void {
  if (data.error) {
    gitBranch.textContent = "git";
    gitMeta.textContent = data.error;
    gitBody.innerHTML = "";
    return;
  }
  if (!data.status) {
    gitBranch.textContent = "git";
    gitMeta.textContent = "not a repo";
    gitBody.innerHTML = "";
    return;
  }
  const { status } = data;
  gitBranch.textContent = status.branch || "(no branch)";
  const parts: string[] = [];
  if (status.ahead > 0) parts.push(`↑${status.ahead}`);
  if (status.behind > 0) parts.push(`↓${status.behind}`);
  gitMeta.textContent = parts.join(" ") || "clean";

  gitBody.innerHTML = "";
  if (status.changes.length === 0) {
    const empty = document.createElement("div");
    empty.className = "git-empty";
    empty.textContent = "no changes";
    gitBody.append(empty);
    return;
  }

  const group = (label: string, items: GitStatus["changes"]) => {
    if (items.length === 0) return;
    const h = document.createElement("div");
    h.className = "git-group";
    h.textContent = label;
    gitBody.append(h);
    for (const c of items) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "git-row";
      row.title = c.path;
      const icon = document.createElement("span");
      icon.className = "git-status" + (c.staged ? " staged" : "");
      icon.textContent = gitStatusSymbol(c.status);
      const path = document.createElement("span");
      path.className = "git-path";
      path.textContent = c.path;
      row.append(icon, path);
      row.addEventListener("click", () => void showGitDiff(c.path));
      gitBody.append(row);
    }
  };

  group("staged", status.changes.filter((c) => c.staged));
  group("changes", status.changes.filter((c) => !c.staged && c.status !== "??"));
  group("untracked", status.changes.filter((c) => c.status === "??"));
}

async function showGitDiff(path: string): Promise<void> {
  gitBody.innerHTML = "";
  const back = document.createElement("button");
  back.type = "button";
  back.className = "git-back";
  back.textContent = "← back";
  back.addEventListener("click", () => void mux.refreshGit());
  gitBody.append(back);

  const pre = document.createElement("pre");
  pre.className = "git-diff";
  pre.textContent = await gitDiff(path);
  gitBody.append(pre);
}

// Leader key state machine (Ctrl+A prefix).
let leaderArmed = false;

function hint(msg: string): void {
  const existing = document.querySelector("#leader-hint");
  if (existing) existing.remove();
  if (!msg) return;
  const span = document.createElement("span");
  span.id = "leader-hint";
  span.textContent = msg;
  statusRight.append(span);
}

// ----- scrollback search (Ctrl+Space then /) -----

let searchOpen = false;

function openSearch(): void {
  searchOpen = true;
  searchBar.classList.add("open");
  searchInput.value = "";
  searchInput.focus();
}

function closeSearch(): void {
  searchOpen = false;
  searchBar.classList.remove("open");
  searchInput.value = "";
  mux.clearSearchFocused();
  void mux.refocus();
}

function runSearch(query: string, backwards: boolean): boolean {
  if (!query) return false;
  return backwards
    ? mux.searchFocused(query, true)
    : mux.searchFocused(query);
}

searchInput.addEventListener("input", () => {
  const found = runSearch(searchInput.value, false);
  hint(found ? `search: ${searchInput.value}` : `no match: ${searchInput.value}`);
});

searchInput.addEventListener("keydown", (e) => {
  e.stopPropagation();
  if (e.key === "Enter") {
    e.preventDefault();
    const found = runSearch(searchInput.value, e.shiftKey);
    hint(found ? `search: ${searchInput.value}` : "no more matches");
  } else if (e.key === "Escape") {
    e.preventDefault();
    closeSearch();
  }
});

// Capture phase so the leader key is seen before xterm.js consumes it.
window.addEventListener(
  "keydown",
  async (e) => {
    // While the search bar is focused, only its own handler deals with keys.
    if (searchOpen) return;

    // Cmd+C copies the selection to the OS clipboard; Cmd+V pastes it into
    // the focused pane (bracketed paste). xterm.js consumes both by default,
    // so intercept them here. Cmd+Backspace deletes the whole line (Ctrl+U,
    // the macOS convention) — xterm would otherwise send a plain DEL.
    if (e.metaKey && !e.ctrlKey) {
      if (e.code === "KeyC") {
        const copied = await mux.copySelection();
        if (copied) {
          e.preventDefault();
          e.stopPropagation();
          hint("copied to clipboard");
        }
        return;
      }
      if (e.code === "KeyV") {
        e.preventDefault();
        e.stopPropagation();
        await mux.pasteClipboard();
        return;
      }
      if (e.code === "Backspace") {
        e.preventDefault();
        e.stopPropagation();
        await mux.write("\x15");
        return;
      }
    }

    // Ctrl+Space arms the leader.
    if (e.ctrlKey && e.code === "Space" && !leaderArmed) {
      e.preventDefault();
      e.stopPropagation();
      leaderArmed = true;
      hint(`LEADER · h=v-split · v=h-split · z=zoom · q=close · /=search · o=open · c=${aiCmd} · n=new · t/g=git · tab=next · esc=exit`);
      return;
    }

    if (leaderArmed) {
      leaderArmed = false;
      hint("");
      const key = e.key.toLowerCase();
      switch (key) {
        case "h":
          e.preventDefault();
          e.stopPropagation();
          await mux.splitActive("v");
          return;
        case "v":
          e.preventDefault();
          e.stopPropagation();
          await mux.splitActive("h");
          return;
        case "z":
          e.preventDefault();
          e.stopPropagation();
          await mux.toggleZoom();
          return;
        case "q":
          e.preventDefault();
          e.stopPropagation();
          await mux.closeActive();
          return;
        case "c":
          e.preventDefault();
          e.stopPropagation();
          await mux.openAiPane();
          return;
        case "n":
          e.preventDefault();
          e.stopPropagation();
          await mux.createSession();
          return;
        case "g":
        case "t":
          e.preventDefault();
          e.stopPropagation();
          await mux.toggleGit();
          return;
        case "tab":
          e.preventDefault();
          e.stopPropagation();
          await mux.nextSession();
          return;
        case "x":
          e.preventDefault();
          e.stopPropagation();
          const sent = await mux.sendContextToAi();
          hint(sent > 0 ? `sent ${sent} chars to AI pane` : "no AI pane open");
          return;
        case "/":
          e.preventDefault();
          e.stopPropagation();
          openSearch();
          return;
        case "o":
          e.preventDefault();
          e.stopPropagation();
          await changeWorkspace();
          return;
        case "escape":
          e.preventDefault();
          e.stopPropagation();
          return;
        default:
          return;
      }
    }
  },
  true,
);

// Persist the layout before the window closes. We don't preventDefault: the
// onCloseRequested wrapper destroys the window automatically once the handler
// resolves. `destroy` needs the `core:window:allow-destroy` capability.
void getCurrentWindow().onCloseRequested(async () => {
  try {
    await mux.saveLayout();
  } catch (err) {
    console.error("failed to save layout on close", err);
  }
});

// ----- workspace bootstrap -----

const welcome = document.getElementById("welcome") as HTMLDivElement;
const openFolderBtn = document.getElementById("open-folder-btn") as HTMLButtonElement;
const recentList = document.getElementById("recent-list") as HTMLDivElement;
const recentItems = document.getElementById("recent-items") as HTMLDivElement;

/** Start a session rooted at `path`, hiding the welcome screen. If a session
 *  is already running (e.g. picked from the native "Open Recent" menu) reload
 *  so everything spawns cleanly relative to the new root. */
async function openWorkspace(path: string): Promise<void> {
  await setWorkspace(path);
  if (currentPanes > 0) {
    window.location.reload();
    return;
  }
  mux.setWorkspaceRoot(path);
  hideWelcome();
  setCurrentWorkspace(path);
  await mux.init();
}

/** Populate the "Recent" list on the welcome screen (most recent first). */
async function renderRecents(): Promise<void> {
  const recents = await getRecentWorkspaces();
  if (recents.length === 0) return;
  recentItems.innerHTML = "";
  for (const path of recents) {
    const item = document.createElement("button");
    item.className = "recent-item";
    item.title = path;
    const label = document.createElement("span");
    label.className = "recent-name";
    label.textContent = basename(path);
    const full = document.createElement("span");
    full.className = "recent-path";
    full.textContent = path;
    item.append(label, full);
    item.addEventListener("click", () => {
      void openWorkspace(path);
    });
    recentItems.append(item);
  }
  recentList.classList.remove("hidden");
}

/** Show the "Open Folder…" welcome screen and wait for a selection. */
function showWelcome(): void {
  welcome.classList.remove("hidden");
  void renderRecents();
}

/** Hide the welcome screen once a session has started. */
function hideWelcome(): void {
  welcome.classList.add("hidden");
}

function setCurrentWorkspace(path: string): void {
  currentWorkspace = path;
  renderStatus();
}

async function boot(): Promise<void> {
  const saved = await getWorkspace();
  if (saved) {
    mux.setWorkspaceRoot(saved);
    hideWelcome();
    setCurrentWorkspace(saved);
    await mux.init();
    return;
  }
  showWelcome();
}

openFolderBtn.addEventListener("click", async () => {
  try {
    const picked = await open({ directory: true, multiple: false, title: "Open Folder" });
    if (typeof picked === "string" && picked) {
      await openWorkspace(picked);
    }
  } catch (err) {
    console.error("failed to pick folder", err);
  }
});

/** Change the workspace at runtime: pick a folder, persist it, and reload so
 *  the whole session (and opencode) spawns relative to the new root. */
async function changeWorkspace(): Promise<void> {
  try {
    const picked = await open({ directory: true, multiple: false, title: "Open Folder" });
    if (typeof picked !== "string" || !picked) return;
    await setWorkspace(picked);
    window.location.reload();
  } catch (err) {
    console.error("failed to change workspace", err);
  }
}

// ----- native macOS menu -----
// The Rust backend builds the app menu bar and emits `menu-*` events when an
// item is clicked. Forward them to the matching action here.

void listen("menu-open-folder", () => {
  void openFolderBtn.click();
});

void listen("menu-open-recent", (e) => {
  if (typeof e.payload === "string" && e.payload) {
    void openWorkspace(e.payload);
  }
});

void listen("menu-split-h", () => {
  void mux.splitActive("h");
});

void listen("menu-split-v", () => {
  void mux.splitActive("v");
});

void listen("menu-zoom", () => {
  void mux.toggleZoom();
});

void listen("menu-close-pane", () => {
  void mux.closeActive();
});

void listen("menu-search", () => {
  openSearch();
});

document.getElementById("new-session-btn")!.addEventListener("click", () => {
  void mux.createSession();
});

document.getElementById("git-refresh")!.addEventListener("click", () => {
  void mux.refreshGit();
});

void boot();
