'use client';

import { useEffect, useRef, useState } from 'react';

export type CopyState = 'idle' | 'copied' | 'failed';

const RESET_MS = 1600;

/** Clipboard copy with a transient result, so buttons can show success or failure. */
export function useCopy() {
  const [state, setState] = useState<CopyState>('idle');
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  async function copy(text: string) {
    let result: CopyState;
    try {
      await navigator.clipboard.writeText(text);
      result = 'copied';
    } catch {
      // Denied permission or an insecure context: say so rather than pretend.
      result = 'failed';
    }
    setState(result);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState('idle'), RESET_MS);
  }

  function reset() {
    window.clearTimeout(timer.current);
    setState('idle');
  }

  return { state, copy, reset };
}

/**
 * Polite live region announcing the copy result. It stays mounted so screen
 * readers pick up the text change; a changed `aria-label` is not re-read.
 */
export function CopyAnnouncement({ state }: { state: CopyState }) {
  return (
    <span role="status" className="sr-only">
      {state === 'copied' ? 'Copied to clipboard' : state === 'failed' ? 'Copy failed' : ''}
    </span>
  );
}
