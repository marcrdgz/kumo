import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";

import { decodeBase64, writePane } from "./api";

const BASE_THEME = {
  background: "#1e1e2e",
  foreground: "#cdd6f4",
  cursor: "#f5e0dc",
  cursorAccent: "#1e1e2e",
  selectionBackground: "#585b70",
  black: "#45475a",
  brightBlack: "#585b70",
  red: "#f38ba8",
  brightRed: "#f38ba8",
  green: "#a6e3a1",
  brightGreen: "#a6e3a1",
  yellow: "#f9e2af",
  brightYellow: "#f9e2af",
  blue: "#89b4fa",
  brightBlue: "#89b4fa",
  magenta: "#cba6f7",
  brightMagenta: "#cba6f7",
  cyan: "#94e2d5",
  brightCyan: "#94e2d5",
  white: "#cdd6f4",
  brightWhite: "#ffffff",
};

export class PaneTerminal {
  readonly paneId: number;
  readonly sessionId: number;
  readonly host: HTMLDivElement;
  private term: Terminal;
  private fit: FitAddon;
  private disposed = false;

  constructor(sessionId: number, paneId: number, host: HTMLDivElement) {
    this.sessionId = sessionId;
    this.paneId = paneId;
    this.host = host;

    this.fit = new FitAddon();
    this.term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily: '"JetBrains Mono", "SFMono-Regular", Menlo, monospace',
      fontSize: 13,
      lineHeight: 1.1,
      scrollback: 10000,
      theme: BASE_THEME,
    });

    this.term.loadAddon(this.fit);
    this.term.open(host);

    this.term.onData((data) => {
      void writePane(
        { sessionId: this.sessionId, paneId: this.paneId },
        data,
      );
    });

    // Fit immediately so the PTY size matches the real cell dimensions.
    // Deferred so the host has a chance to be laid out in the DOM.
    requestAnimationFrame(() => this.fitNow());
  }

  writeRaw(bytes: Uint8Array): void {
    if (this.disposed) return;
    this.term.write(bytes);
  }

  writeBase64(b64: string): void {
    if (this.disposed) return;
    this.term.write(decodeBase64(b64));
  }

  fitNow(): void {
    if (this.disposed) return;
    try {
      this.fit.fit();
    } catch {
      /* ignore */
    }
  }

  cols(): number {
    return this.term.cols;
  }

  rows(): number {
    return this.term.rows;
  }

  /** Extract recent scrollback text (walking up from the cursor). */
  contextText(maxChars = 4000): string {
    const buffer = this.term.buffer.active;
    const endY = buffer.baseY + buffer.cursorY;
    const lines: string[] = [];
    let total = 0;
    for (let y = endY; y >= 0 && total < maxChars; y--) {
      const line = buffer.getLine(y);
      if (!line) break;
      const text = line.translateToString(true);
      total += text.length + 1;
      lines.push(text);
    }
    lines.reverse();
    let out = lines.join("\n");
    if (out.length > maxChars) out = out.slice(out.length - maxChars);
    return out;
  }

  focus(): void {
    this.term.focus();
  }

  dispose(): void {
    this.disposed = true;
    this.term.dispose();
  }
}
