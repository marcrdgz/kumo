import Link from 'next/link';
import type { ReactNode } from 'react';
import { HomeLayout } from 'fumadocs-ui/layouts/home';
import { baseOptions } from '@/lib/layout.shared';
import { CopyCommand } from '@/components/home/copy-command';
import { HeroDemo } from '@/components/home/terminal-preview';

const INSTALL = `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/marcrdgz/kumo/releases/latest/download/kumo-installer.sh | sh`;
const GITHUB = 'https://github.com/marcrdgz/kumo';

export const metadata = {
  title: 'Kumo — the terminal multiplexer for your AI agents',
  description:
    'Kumo is a featherweight terminal multiplexer that weaves your AI agents together into a single web.',
};

export default function HomePage() {
  return (
    <HomeLayout
      {...baseOptions()}
      links={[
        { text: 'Documentation', url: '/docs' },
        { text: 'Roadmap', url: '/docs/roadmap' },
      ]}
    >
      <main className="relative flex flex-1 flex-col items-center overflow-x-clip px-5">
        {/* Hero */}
        {/* Headline left, supporting copy in the right column: the type keeps a
            left reading edge, the full width is used, and the demo starts above
            the fold at common laptop heights. */}
        <section className="w-full max-w-6xl pt-14 sm:pt-20">
          <div className="grid gap-x-12 gap-y-8 lg:grid-cols-12 lg:items-end">
            <h1 className="font-display text-[clamp(2.25rem,5vw,4rem)] leading-[1.08] font-medium tracking-[-0.045em] text-balance lg:col-span-7">
              tmux, if it knew what your agents were doing.
            </h1>

            <div className="lg:col-span-5 lg:pb-1.5">
              <p className="max-w-[52ch] text-lg leading-relaxed text-fd-muted-foreground text-pretty">
                kumo is a terminal multiplexer for AI coding agents. Every pane is a real
                Ghostty terminal, and the sidebar shows which agent is working, which one
                is waiting on you, and which one just finished.
              </p>
              <div className="mt-7 flex flex-wrap items-center gap-x-7 gap-y-3">
                <Link
                  href="/docs/getting-started/installation"
                  className="inline-flex h-11 items-center rounded-full bg-fd-foreground px-6 text-[15px] font-medium text-fd-background transition-opacity hover:opacity-85"
                >
                  Install kumo
                </Link>
                <Link
                  href="/docs/getting-started/quickstart"
                  className="text-[15px] font-medium underline decoration-fd-border underline-offset-[6px] transition-colors hover:decoration-fd-foreground"
                >
                  Read the quickstart
                </Link>
              </div>
            </div>
          </div>

          <div className="mt-12 sm:mt-14">
            <HeroDemo />
          </div>

          <div className="mt-14 flex flex-col gap-3 sm:flex-row sm:items-center sm:gap-6">
            <div className="w-full max-w-2xl">
              <CopyCommand command={INSTALL} />
            </div>
            <p className="text-sm text-fd-muted-foreground">
              Open source under MIT. kumo itself idles at about 6 MB of memory.
            </p>
          </div>
        </section>

        {/* Bento */}
        <section className="w-full max-w-6xl py-24 sm:py-32">
          <h2 className="max-w-2xl text-3xl font-semibold tracking-[-0.03em] text-balance sm:text-4xl">
            Everything else you&apos;d expect from a multiplexer, and a few things you wouldn&apos;t.
          </h2>

          <div className="mt-12 grid grid-cols-1 gap-4 md:grid-cols-3">
            <Tile
              className="md:col-span-2"
              title="An inbox for agents"
              href="/docs/guides/agents"
              body="Press leader+i to step through the agents that need you, blocked first, then finished, then running. Enter jumps straight to the pane, switching session and tab for you."
            >
              <p className="mt-8 flex flex-wrap items-center gap-2 text-sm text-fd-muted-foreground">
                <kbd className="rounded-md border border-b-2 border-fd-border bg-fd-background px-2 py-0.5 font-sans text-fd-foreground">⌃B</kbd>
                <kbd className="rounded-md border border-b-2 border-fd-border bg-fd-background px-2 py-0.5 font-sans text-fd-foreground">i</kbd>
                <span className="ml-1">then</span>
                <kbd className="rounded-md border border-b-2 border-fd-border bg-fd-background px-2 py-0.5 font-sans text-fd-foreground">j</kbd>
                <kbd className="rounded-md border border-b-2 border-fd-border bg-fd-background px-2 py-0.5 font-sans text-fd-foreground">k</kbd>
                <kbd className="rounded-md border border-b-2 border-fd-border bg-fd-background px-2 py-0.5 font-sans text-fd-foreground">↵</kbd>
              </p>
            </Tile>
            <Tile
              title="Real terminals"
              href="/docs"
              body="Every pane is a genuine VT/xterm emulator powered by Ghostty. Shells, TUIs and full-screen apps behave exactly as they do natively."
            />
            <Tile
              title="Orchestration"
              href="/docs/guides/orchestration"
              body="Agents drive their own web: wait, prompt and read other panes with server-owned waits. No polling."
            >
              <code className="mt-6 block overflow-x-auto rounded-xl bg-fd-muted px-3.5 py-2.5 font-mono text-[12.5px] whitespace-nowrap">
                kumo agent wait -p 42 --until done
              </code>
            </Tile>
            <Tile
              title="Panes & sessions"
              href="/docs/guides/sessions-and-panes"
              body="A binary split tree with mouse-drag resizing, zoom, tabs and any number of independent sessions."
            />
            <Tile
              title="Detach & re-attach"
              href="/docs/guides/detach-and-reattach"
              body="The daemon keeps your web alive, even across a reboot. Come back from any terminal, right where you left off."
            />
            <Tile
              className="md:col-span-2"
              title="Make it yours"
              href="/docs/configuration"
              body="Leader key, bindings, shells and AI commands, all in a single config.toml that reloads live."
            >
              <pre className="mt-6 overflow-x-auto rounded-xl bg-fd-muted px-4 py-3 font-mono text-[12.5px] leading-relaxed">
                <span className="text-fd-muted-foreground"># ~/.config/kumo/config.toml</span>
                {'\n'}ai-cmd = <span className="text-fd-primary">&quot;claude&quot;</span>
                {'\n'}
                {'\n'}[keymap]
                {'\n'}leader = <span className="text-fd-primary">&quot;ctrl+b&quot;</span>
              </pre>
            </Tile>
            <Tile
              title="Any AI CLI"
              href="/docs/guides/agents"
              body="opencode, claude, codex… spawn one with a keystroke, or just run it. kumo detects agents in any pane."
            />
          </div>
        </section>

        {/* Closing CTA */}
        <section className="flex w-full max-w-6xl flex-col items-center border-t border-fd-border py-24 text-center sm:py-32">
          <p className="text-5xl font-semibold text-fd-muted-foreground/40">蜘蛛</p>
          <h2 className="mt-6 max-w-xl text-3xl font-semibold tracking-[-0.03em] text-balance sm:text-5xl">
            A spider always knows what&apos;s happening in its web.
          </h2>
          <Link
            href="/docs/getting-started/quickstart"
            className="mt-9 inline-flex h-11 items-center gap-1.5 rounded-full bg-fd-primary px-6 text-[15px] font-medium text-fd-primary-foreground transition-transform hover:scale-[1.03] active:scale-[0.98]"
          >
            Start the quickstart
          </Link>
        </section>

        <footer className="w-full max-w-6xl border-t border-fd-border py-8 text-xs text-fd-muted-foreground">
          MIT licensed · © 2026 Marc Rodriguez
        </footer>
      </main>
    </HomeLayout>
  );
}

function Tile({
  title,
  body,
  href,
  className = '',
  children,
}: {
  title: string;
  body: string;
  href: string;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <Link
      href={href}
      className={`group flex flex-col rounded-3xl border border-fd-border bg-fd-card p-7 transition-colors hover:border-fd-foreground/20 ${className}`}
    >
      <h3 className="text-xl font-semibold tracking-[-0.02em]">{title}</h3>
      <p className="mt-2 leading-relaxed text-fd-muted-foreground text-pretty">{body}</p>
      {children}
      <span className="mt-auto inline-flex items-center gap-0.5 pt-6 text-sm font-medium text-fd-muted-foreground group-hover:text-fd-foreground">
        Learn more
      </span>
    </Link>
  );
}
