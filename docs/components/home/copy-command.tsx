'use client';

import { Check, Copy, X } from 'lucide-react';
import { CopyAnnouncement, useCopy } from '@/components/use-copy';

export function CopyCommand({ command }: { command: string }) {
  const { state, copy } = useCopy();

  return (
    <div className="flex items-center gap-3 rounded-xl border border-fd-border bg-fd-card py-1.5 pr-1.5 pl-4">
      <span aria-hidden className="select-none font-mono text-sm text-fd-muted-foreground">
        $
      </span>
      <code className="scroll-fade min-w-0 flex-1 overflow-x-auto whitespace-nowrap font-mono text-sm [scrollbar-width:thin]">
        {command}
        <span aria-hidden className="scroll-fade-hint" />
      </code>
      <button
        type="button"
        onClick={() => copy(command)}
        aria-label="Copy install command"
        className="inline-flex size-9 shrink-0 cursor-pointer items-center justify-center rounded-lg text-fd-muted-foreground outline-fd-primary transition-colors hover:bg-fd-muted hover:text-fd-foreground focus-visible:outline-2 focus-visible:outline-offset-2"
      >
        {state === 'copied' ? (
          <Check className="size-4 text-status-done" />
        ) : state === 'failed' ? (
          <X className="size-4 text-status-blocked" />
        ) : (
          <Copy className="size-4" />
        )}
      </button>
      <CopyAnnouncement state={state} />
    </div>
  );
}
