import { Multiplexer } from "./multiplexer";
import { aiCommand, type SessionInfo } from "./api";
import { getCurrentWindow } from "@tauri-apps/api/window";
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

const mux = new Multiplexer(workspace, (s: SessionInfo) => {
  if (s.panes.length === 0) {
    statusLeft.textContent = "session closed";
    return;
  }
  statusLeft.innerHTML = `<span class="mode">neomux</span>${s.name} · ${s.panes.length} pane(s)`;
  statusRight.textContent = `session ${s.sessionId}`;
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
      hint(`LEADER: h=v-split · v=h-split · z=zoom · q=close · /=search · c=${aiCmd} · Esc=exit`);
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

void mux.init();

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
