'use client';

import { Check, Copy } from 'lucide-react';
import { useState } from 'react';

export function CopyCommand({ command }: { command: string }) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    await navigator.clipboard.writeText(command);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  }

  return (
    <div className="flex items-center gap-3 rounded-xl border border-fd-border bg-fd-card py-1.5 pr-1.5 pl-4">
      <span aria-hidden className="select-none font-mono text-sm text-fd-muted-foreground">
        $
      </span>
      <code className="fade-x min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-sm [scrollbar-width:none]">
        {command}
      </code>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? 'Copied' : 'Copy install command'}
        className="inline-flex size-9 shrink-0 cursor-pointer items-center justify-center rounded-lg text-fd-muted-foreground transition-colors hover:bg-fd-muted hover:text-fd-foreground"
      >
        {copied ? <Check className="size-4 text-status-done" /> : <Copy className="size-4" />}
      </button>
    </div>
  );
}
