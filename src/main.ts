import { Multiplexer } from "./multiplexer";
import { aiCommand, type SessionInfo } from "./api";
import "@xterm/xterm/css/xterm.css";

const statusLeft = document.getElementById("status-left")!;
const statusRight = document.getElementById("status-right")!;
const workspace = document.getElementById("workspace") as HTMLDivElement;

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
  const span = document.createElement("span");
  span.id = "leader-hint";
  span.textContent = msg;
  statusRight.append(span);
}

// Capture phase so the leader key is seen before xterm.js consumes it.
window.addEventListener(
  "keydown",
  async (e) => {
    // Ctrl+A arms the leader.
    if (e.ctrlKey && e.key.toLowerCase() === "a" && !leaderArmed) {
      e.preventDefault();
      e.stopPropagation();
      leaderArmed = true;
      hint(`LEADER: h=v-split · v=h-split · z=zoom · q=close · c=${aiCmd} · Esc=exit`);
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
