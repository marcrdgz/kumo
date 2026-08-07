import { Multiplexer } from "./multiplexer";
import type { SessionInfo } from "./api";
import "@xterm/xterm/css/xterm.css";

const statusLeft = document.getElementById("status-left")!;
const statusRight = document.getElementById("status-right")!;
const workspace = document.getElementById("workspace") as HTMLDivElement;

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

window.addEventListener("keydown", async (e) => {
  // Ctrl+A arms the leader.
  if (e.ctrlKey && e.key.toLowerCase() === "a" && !leaderArmed) {
    e.preventDefault();
    leaderArmed = true;
    hint("LEADER: h=v-split · v=h-split · z=zoom · q=close · c=claude · Esc=exit");
    return;
  }

  if (leaderArmed) {
    leaderArmed = false;
    hint("");
    const key = e.key.toLowerCase();
    switch (key) {
      case "h":
        e.preventDefault();
        await mux.splitActive("v");
        return;
      case "v":
        e.preventDefault();
        await mux.splitActive("h");
        return;
      case "z":
        e.preventDefault();
        await mux.toggleZoom();
        return;
      case "q":
        e.preventDefault();
        await mux.closeActive();
        return;
      case "c":
        e.preventDefault();
        await mux.write("\r\n[neomux] Claude AI pane coming in next phase\r\n");
        return;
      case "escape":
        return;
      default:
        return;
    }
  }
});

void mux.init();
