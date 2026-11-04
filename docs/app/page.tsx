import Link from 'next/link';
import type { ReactNode } from 'react';
import { HomeLayout } from 'fumadocs-ui/layouts/home';
import { baseOptions } from '@/lib/layout.shared';
import { CopyCommand } from '@/components/home/copy-command';
import { HeroBackdrop, PeekingEyes } from '@/components/home/hero-backdrop';
import { HeroDemo } from '@/components/home/terminal-preview';
import { OG_IMAGE } from '@/lib/site';

const INSTALL = `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/marcrdgz/kumo/releases/latest/download/kumo-installer.sh | sh`;
const GITHUB = 'https://github.com/marcrdgz/kumo';
const RELEASES = `${GITHUB}/releases`;

const TITLE = 'kumo — a terminal multiplexer for AI coding agents';
const DESCRIPTION =
  'kumo is a terminal multiplexer for AI coding agents. Every pane is a real Ghostty terminal, and the sidebar shows which agent is working, waiting on you, or done.';

export const metadata = {
  title: TITLE,
  description: DESCRIPTION,
  // Page-level openGraph replaces the layout's, so its fields repeat here.
  openGraph: {
    title: TITLE,
    description: DESCRIPTION,
    siteName: 'kumo',
    type: 'website',
    images: [OG_IMAGE],
  },
};

const KBD =
  'rounded-md border border-b-2 border-fd-border bg-fd-background px-2 py-0.5 font-sans text-fd-foreground';
const TEXT_LINK =
  'font-medium underline decoration-fd-border underline-offset-[6px] transition-colors hover:decoration-fd-foreground';

export default function HomePage() {
  return (
    <HomeLayout
      {...baseOptions()}
      links={[
        { text: 'Documentation', url: '/docs' },
        { text: 'Roadmap', url: '/docs/roadmap' },
      ]}
    >
      {/* isolate keeps the backdrop's negative z-index inside main. */}
      <main className="relative isolate flex flex-1 flex-col items-center overflow-x-clip px-5">
        <HeroBackdrop />

        {/* Hero */}
        {/* Headline left, supporting copy and the install command in the right
            column: the type keeps a left reading edge, the full width is used,
            and the demo starts above the fold at common laptop heights. */}
        <section className="w-full max-w-6xl pt-14 sm:pt-20">
          <div className="grid gap-x-12 gap-y-8 lg:grid-cols-12 lg:items-end">
            <h1 className="font-display text-[clamp(2rem,5vw,4rem)] leading-[1.08] font-medium tracking-[-0.045em] text-balance lg:col-span-7">
              tmux, if it knew what your agents were doing.
            </h1>

            <div className="min-w-0 lg:col-span-5 lg:pb-1.5">
              {/* The demo below shows the agent states, so the copy stays short. */}
              <p className="max-w-[44ch] text-lg leading-relaxed text-fd-muted-foreground text-pretty">
                A terminal multiplexer for AI coding agents, built on Ghostty.
              </p>
              <div className="mt-6">
                <CopyCommand command={INSTALL} />
              </div>
              <Link
                href="/docs/getting-started/quickstart"
                className={`mt-4 inline-block text-[15px] ${TEXT_LINK}`}
              >
                Read the quickstart
              </Link>
            </div>
          </div>

          <div className="relative mt-12 sm:mt-14">
            <PeekingEyes />
            <HeroDemo />
          </div>
        </section>

        {/* Features */}
        <section className="w-full max-w-6xl py-24 sm:py-32">
          <h2 className="max-w-2xl text-3xl font-semibold tracking-[-0.03em] text-balance sm:text-4xl">
            Also a complete multiplexer.
          </h2>

          <div className="mt-12 grid grid-cols-1 gap-4 md:grid-cols-3">
            <Tile
              className="md:col-span-2"
              title="An inbox for agents"
              href="/docs/guides/agents"
              body="claude, opencode and codex are detected in any pane. Press leader+i to step through the ones that need you, blocked first, then finished, then running. Enter jumps straight to the pane, switching session and tab for you."
            >
              <p className="mt-8 flex flex-wrap items-center gap-2 text-sm text-fd-muted-foreground">
                <kbd className={KBD}>⌃B</kbd>
                <kbd className={KBD}>i</kbd>
                <span className="ml-1">then</span>
                <kbd className={KBD}>j</kbd>
                <kbd className={KBD}>k</kbd>
                <kbd className={KBD}>↵</kbd>
              </p>
            </Tile>
            <Tile
              title="Orchestration"
              href="/docs/guides/orchestration"
              body="Agents can wait on, prompt and read other panes from the CLI. The daemon holds the wait, so nothing polls."
            >
              <code className="mt-6 block overflow-x-auto rounded-xl bg-fd-muted px-3.5 py-2.5 font-mono text-[12.5px] whitespace-nowrap">
                kumo agent wait -p 42 --until done
              </code>
            </Tile>
            <Tile
              title="Sessions that outlive the terminal"
              href="/docs/guides/detach-and-reattach"
              body="Split panes, tabs, zoom and any number of sessions. The daemon keeps them running, even across a reboot, and you reattach from any terminal."
            />
            <Tile
              className="md:col-span-2"
              title="One config file"
              href="/docs/configuration"
              body="Leader key, bindings, shells and AI commands live in a single config.toml that reloads without a restart."
            >
              <pre className="mt-6 overflow-x-auto rounded-xl bg-fd-muted px-4 py-3 font-mono text-[12.5px] leading-relaxed">
                <span className="text-fd-muted-foreground"># ~/.config/kumo/config.toml</span>
                {'\n'}ai-cmd = <span className="text-fd-primary">&quot;claude&quot;</span>
                {'\n'}
                {'\n'}[keymap]
                {'\n'}leader = <span className="text-fd-primary">&quot;ctrl+b&quot;</span>
              </pre>
            </Tile>
          </div>
        </section>

        {/* Closing: the install command again, for readers who scrolled. */}
        <section className="w-full max-w-6xl border-t border-fd-border py-20 sm:py-24">
          <div className="grid gap-x-12 gap-y-6 lg:grid-cols-12 lg:items-center">
            <h2 className="text-3xl font-semibold tracking-[-0.03em] lg:col-span-4">
              Install kumo
            </h2>
            <div className="min-w-0 lg:col-span-8">
              <CopyCommand command={INSTALL} />
              <p className="mt-3 text-sm text-fd-muted-foreground">
                MIT licensed, idles at about 6 MB. Nightly builds and building from source are covered in the{' '}
                <Link href="/docs/getting-started/installation" className={TEXT_LINK}>
                  installation guide
                </Link>
                .
              </p>
            </div>
          </div>
        </section>

        <footer className="flex w-full max-w-6xl flex-wrap items-center gap-x-6 gap-y-3 border-t border-fd-border py-8 text-sm text-fd-muted-foreground">
          <span className="font-display text-fd-foreground">kumo</span>
          <Link href="/docs" className="hover:text-fd-foreground">Docs</Link>
          <Link href="/docs/roadmap" className="hover:text-fd-foreground">Roadmap</Link>
          <a href={RELEASES} className="hover:text-fd-foreground">Changelog</a>
          <a href={GITHUB} className="hover:text-fd-foreground">GitHub</a>
          <span className="text-xs sm:ml-auto">MIT licensed · © 2026 Marc Rodriguez</span>
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
      className={`flex flex-col rounded-3xl border border-fd-border bg-fd-card p-7 transition-colors hover:border-fd-foreground/20 ${className}`}
    >
      <h3 className="text-xl font-semibold tracking-[-0.02em]">{title}</h3>
      <p className="mt-2 leading-relaxed text-fd-muted-foreground text-pretty">{body}</p>
      {children}
    </Link>
  );
}
