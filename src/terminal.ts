import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";

import { decodeBase64, writePane } from "./api";

export const DEFAULT_THEME: Record<string, string> = {
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
      theme: DEFAULT_THEME,
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

  /** True when a fullscreen TUI (vim/nvim, etc.) owns the alternate screen. */
  inAlternateScreen(): boolean {
    return this.term.buffer.active.type === "alternate";
  }

  /** Text selected with the mouse via xterm's own selection, if any. */
  mouseSelection(): string | null {
    if (!this.term.hasSelection()) return null;
    const text = this.term.getSelection();
    return text.length > 0 ? text : null;
  }

  /**
   * Text currently selected in vim's visual mode, detected from the
   * reverse-video (non-default background) cells the editor paints in the
   * alternate screen. Returns null when no visual selection is active or the
   * pane isn't running a fullscreen editor.
   */
  visualSelection(): string | null {
    const buffer = this.term.buffer.active;
    if (buffer.type !== "alternate") return null;

    const isHighlighted = (line: any, x: number): boolean => {
      const cell = line.getCell(x);
      if (!cell) return false;
      return cell.getBgColorMode() !== 0 || cell.isInverse() !== 0;
    };

    // Find contiguous rows whose text cells carry a non-default background,
    // stopping at the statusline/command row (vim paints that separately).
    const rows = buffer.length;
    const rowHighlighted: boolean[] = [];
    for (let y = 0; y < rows; y++) {
      const line = buffer.getLine(y);
      if (!line) continue;
      let found = false;
      for (let x = 0; x < line.length; x++) {
        if (isHighlighted(line, x)) {
          found = true;
          break;
        }
      }
      rowHighlighted.push(found);
    }

    // Take the highlighted block that contains the cursor row.
    const cy = buffer.cursorY;
    if (!rowHighlighted[cy]) return null;
    let top = cy;
    while (top > 0 && rowHighlighted[top - 1]) top--;
    let bottom = cy;
    while (bottom < rows - 1 && rowHighlighted[bottom + 1]) bottom++;

    const lines: string[] = [];
    let minX = Infinity;
    let maxX = -Infinity;
    for (let y = top; y <= bottom; y++) {
      const line = buffer.getLine(y);
      if (!line) continue;
      for (let x = 0; x < line.length; x++) {
        if (isHighlighted(line, x)) {
          if (x < minX) minX = x;
          if (x > maxX) maxX = x;
        }
      }
    }
    if (maxX < minX) return null;
    for (let y = top; y <= bottom; y++) {
      const line = buffer.getLine(y);
      if (!line) continue;
      const parts: string[] = [];
      for (let x = minX; x <= maxX; x++) {
        // The cursor cell on the active row is not painted with the Visual
        // background, so read the full range instead of only highlighted cells.
        parts.push(line.getCell(x)?.getChars() ?? " ");
      }
      lines.push(parts.join("").replace(/\s+$/, ""));
    }
    const text = lines.join("\n");
    return text.length > 0 ? text : null;
  }

  /**
   * Best-effort line:col of a fullscreen editor, parsed from the statusline
   * / ruler rendered in the bottom rows of the alternate screen. Returns
   * null when no editor-position pattern (e.g. `1,1` in a statusline) is
   * visible.
   */
  statusPosition(): { line: number; col: number } | null {
    const buffer = this.term.buffer.active;
    const start = buffer.baseY + Math.max(0, buffer.length - 2);
    for (let y = start; y < buffer.baseY + buffer.length; y++) {
      const line = buffer.getLine(y);
      if (!line) continue;
      const text = line.translateToString(true);
      const matches = [...text.matchAll(/(\d+)\s*,\s*(\d+)/g)];
      const last = matches.pop();
      if (last) return { line: Number(last[1]), col: Number(last[2]) };
    }
    return null;
  }

  focus(): void {
    this.term.focus();
  }

  dispose(): void {
    this.disposed = true;
    this.term.dispose();
  }
}
