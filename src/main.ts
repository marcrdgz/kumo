import { Multiplexer } from "./multiplexer";
import { aiCommand, getRecentWorkspaces, getWorkspace, setWorkspace, type SessionInfo } from "./api";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import "@xterm/xterm/css/xterm.css";

const statusLeft = document.getElementById("status-left")!;
const statusRight = document.getElementById("status-right")!;
const workspace = document.getElementById("workspace") as HTMLDivElement;
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
  const panes = currentPanes;
  if (currentWorkspace) {
    statusLeft.innerHTML = `<span class="mode">●</span> <span class="session-name">${basename(currentWorkspace)}</span>`;
  } else {
    statusLeft.innerHTML = `<span class="mode">●</span> <span class="session-name">${currentSessionName}</span>`;
  }
  statusRight.textContent = `${panes} panes${currentWorkspace ? "" : ` · ${currentSessionName}`}`;
}

let currentPanes = 0;
let currentSessionName = "";

const mux = new Multiplexer(workspace, (s: SessionInfo) => {
  if (s.panes.length === 0) {
    statusLeft.textContent = "session closed";
    statusRight.textContent = "";
    currentPanes = 0;
    currentSessionName = "";
    return;
  }
  currentPanes = s.panes.length;
  currentSessionName = s.name;
  renderStatus();
});

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
    // so intercept them here.
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
    }

    // Ctrl+Space arms the leader.
    if (e.ctrlKey && e.code === "Space" && !leaderArmed) {
      e.preventDefault();
      e.stopPropagation();
      leaderArmed = true;
      hint(`LEADER · h=v-split · v=h-split · z=zoom · q=close · /=search · o=open · c=${aiCmd} · esc=exit`);
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

void boot();
