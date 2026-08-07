import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";

import { decodeBase64, writePane } from "./api";

const BASE_THEME = {
  background: "#1a1b26",
  foreground: "#c0caf5",
  cursor: "#7aa2f7",
  selectionBackground: "#3b4261",
  black: "#1d202f",
  brightBlack: "#565f89",
  red: "#f7768e",
  brightRed: "#ff7a93",
  green: "#9ece6a",
  brightGreen: "#b9f27c",
  yellow: "#e0af68",
  brightYellow: "#ff9e64",
  blue: "#7aa2f7",
  brightBlue: "#7da6ff",
  magenta: "#bb9af7",
  brightMagenta: "#bb9af7",
  cyan: "#7dcfff",
  brightCyan: "#89ddff",
  white: "#c0caf5",
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

  focus(): void {
    this.term.focus();
  }

  dispose(): void {
    this.disposed = true;
    this.term.dispose();
  }
}
