'use client';

import { useEffect, useState } from 'react';

/**
 * Illustrative, animated rendering of the kumo TUI for the landing page.
 * It loops through the moments kumo exists for: an agent blocks on an
 * approval (floats to the top, glows), another finishes (toast), and the
 * blocked one resumes. The terminal stays dark in both site themes.
 */

type Status = 'working' | 'blocked' | 'done' | 'idle';
type AgentName = 'api' | 'web' | 'docs' | 'infra';

interface Frame {
  status: Record<AgentName, Status>;
  toast?: { agent: AgentName; text: string; status: Status };
  caption: number;
}

const FRAMES: Frame[] = [
  { status: { api: 'working', web: 'working', docs: 'working', infra: 'idle' }, caption: 0 },
  {
    status: { api: 'working', web: 'blocked', docs: 'working', infra: 'idle' },
    toast: { agent: 'web', text: 'needs your approval', status: 'blocked' },
    caption: 1,
  },
  {
    status: { api: 'working', web: 'blocked', docs: 'done', infra: 'idle' },
    toast: { agent: 'docs', text: 'finished', status: 'done' },
    caption: 2,
  },
  { status: { api: 'working', web: 'working', docs: 'done', infra: 'idle' }, caption: 0 },
];

/** The frame shown when motion is reduced: every state visible at once. */
const STILL = 2;
const STEP_MS = 2600;

const CAPTIONS = [
  'Working agents pulse blue while they stream output.',
  'An agent waiting on you floats to the top, glows orange and chimes.',
  'One that finishes while you are elsewhere raises a toast in every client.',
];

const CLI: Record<AgentName, string> = { api: 'claude', web: 'opencode', docs: 'codex', infra: 'zsh' };
const ORDER: AgentName[] = ['api', 'web', 'docs', 'infra'];
const RANK: Record<Status, number> = { blocked: 0, done: 1, working: 2, idle: 3 };

const DOT: Record<Status, string> = {
  working: 'bg-status-working pulse-dot',
  blocked: 'bg-status-blocked',
  done: 'bg-status-done',
  idle: 'bg-status-idle/70',
};

const BLOCKED_GLOW =
  'shadow-[inset_0_0_0_1px_rgb(255_159_10/0.6),inset_0_0_36px_rgb(255_159_10/0.1)]';

function useAgentTimeline() {
  const [index, setIndex] = useState(STILL);

  useEffect(() => {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    setIndex(0);
    const id = setInterval(() => setIndex((i) => (i + 1) % FRAMES.length), STEP_MS);
    return () => clearInterval(id);
  }, []);

  return FRAMES[index];
}

function TerminalPreview({ frame }: { frame: Frame }) {
  const { status, toast } = frame;
  // Blocked and finished agents rise to the top, like the TUI's agent list.
  const sorted = [...ORDER].sort((a, b) => RANK[status[a]] - RANK[status[b]]);

  return (
    <div
      role="img"
      aria-label="kumo running four AI agents, with a sidebar of live status dots next to split terminal panes"
      className="window-shadow overflow-hidden rounded-[14px] bg-[#0d0d0f] text-left font-mono text-[11.5px] leading-[1.6] text-[#d6d6db] sm:text-[12.5px]"
    >
      <div className="flex h-9 items-center gap-2 border-b border-white/[0.06] px-4">
        <span className="size-3 rounded-full bg-[#ff5f57]" />
        <span className="size-3 rounded-full bg-[#febc2e]" />
        <span className="size-3 rounded-full bg-[#28c840]" />
        <span className="flex-1 text-center font-sans text-xs text-white/40">kumo</span>
        <span className="w-[52px]" />
      </div>

      <div className="relative grid h-[264px] grid-cols-1 sm:h-[400px] sm:grid-cols-[184px_1fr]">
        <aside className="p-3 sm:border-r sm:border-white/[0.06]">
          <p className="mb-2 px-2 text-white/35">agents</p>
          <ul className="relative h-[176px]">
            {ORDER.map((name) => {
              const s = status[name];
              return (
                <li
                  key={name}
                  className="absolute inset-x-0 rounded-md px-2 py-1 transition-transform duration-500 ease-[cubic-bezier(0.22,1,0.36,1)]"
                  style={{ transform: `translateY(${sorted.indexOf(name) * 44}px)` }}
                >
                  <div className="flex items-center gap-2">
                    <span className={`size-2 rounded-full transition-colors ${DOT[s]}`} />
                    <span className={s === 'idle' ? 'text-white/50' : 'text-white'}>{name}</span>
                    {s === 'blocked' && <span className="text-status-blocked">blocked</span>}
                    {s === 'done' && <span className="text-status-done">done</span>}
                  </div>
                  <p className="pl-4 text-white/30">{CLI[name]}</p>
                </li>
              );
            })}
          </ul>
        </aside>

        <div className="hidden min-w-0 grid-cols-1 gap-px bg-white/[0.06] sm:grid md:grid-cols-2">
          <section className="min-w-0 overflow-hidden bg-[#0d0d0f] p-4">
            <p className="text-white/35">api · claude</p>
            <p className="mt-3 text-white">› add rate limiting to the auth endpoints</p>
            <p className="mt-3">
              <span className="text-status-working">●</span> Read src/auth/mod.rs
            </p>
            <p>
              <span className="text-status-working">●</span> Edit src/auth/limiter.rs
            </p>
            <p className="pl-4 text-[#7ee2a8]">+ pub struct RateLimiter {'{'}</p>
            <p className="pl-4 text-[#7ee2a8]">+ &nbsp;&nbsp;window: Duration,</p>
            <p className="pl-4 text-[#7ee2a8]">+ &nbsp;&nbsp;burst: u32,</p>
            <p className="pl-4 text-[#7ee2a8]">+ {'}'}</p>
            <p className="mt-3 text-white/50">
              <span className="pulse-dot text-status-working">✻</span> Working…
            </p>
          </section>

          <div className="hidden min-w-0 grid-rows-2 gap-px md:grid">
            <section
              className={`overflow-hidden bg-[#0d0d0f] p-4 transition-shadow duration-500 ${
                status.web === 'blocked' ? BLOCKED_GLOW : ''
              }`}
            >
              <p className="text-white/35">web · opencode</p>
              {status.web === 'blocked' ? (
                <>
                  <p className="mt-3">Run `npm install @tanstack/query`?</p>
                  <p className="mt-2">
                    <span className="rounded bg-status-blocked px-1.5 text-black">allow</span>
                    <span className="ml-2 text-white/50">deny</span>
                  </p>
                </>
              ) : (
                <>
                  <p className="mt-3 text-white/60">Fixing hydration mismatch in Header.tsx</p>
                  <p className="text-white/50">
                    <span className="pulse-dot text-status-working">✻</span> Working…
                  </p>
                </>
              )}
            </section>
            <section className="overflow-hidden bg-[#0d0d0f] p-4">
              <p className="text-white/35">docs · codex</p>
              <p className="mt-3 text-white/60">Updated 4 pages in content/guides</p>
              {status.docs === 'done' ? (
                <p className="text-status-done">✓ Done. Ready for review.</p>
              ) : (
                <p className="text-white/50">
                  <span className="pulse-dot text-status-working">✻</span> Working…
                </p>
              )}
            </section>
          </div>
        </div>

        {toast && (
          <div
            key={toast.agent}
            className="toast-in absolute right-3 bottom-3 flex items-center gap-2 rounded-lg border border-white/10 bg-[#1c1c1e] px-3 py-2 font-sans text-xs text-white/85"
          >
            <span className={`size-2 rounded-full ${DOT[toast.status]}`} />
            <span className="font-medium text-white">{toast.agent}</span> {toast.text}
          </div>
        )}
      </div>

      <div className="flex h-7 items-center gap-4 border-t border-white/[0.06] px-4 text-[11px] text-white/45">
        <span className="text-white/80">1:api</span>
        <span>2:web</span>
        <span>3:docs</span>
        <span className="ml-auto hidden sm:inline">leader ⌃B</span>
      </div>
    </div>
  );
}

const CAPTION_DOT = ['bg-status-working', 'bg-status-blocked', 'bg-status-done'];

/** The animated TUI with captions that follow whichever moment is on screen. */
export function HeroDemo() {
  const frame = useAgentTimeline();

  return (
    <figure>
      <TerminalPreview frame={frame} />
      <figcaption className="mt-6 grid gap-x-10 gap-y-3 text-[15px] leading-snug sm:grid-cols-3">
        {CAPTIONS.map((text, i) => (
          <p
            key={text}
            className={`flex gap-3 transition-colors duration-500 ${
              frame.caption === i ? 'text-fd-foreground' : 'text-fd-muted-foreground/70'
            }`}
          >
            <span className={`mt-[0.45em] size-2 shrink-0 rounded-full ${CAPTION_DOT[i]}`} />
            {text}
          </p>
        ))}
      </figcaption>
    </figure>
  );
}
