import Link from 'next/link';
import { HomeLayout } from 'fumadocs-ui/layouts/home';
import { Card, Cards } from 'fumadocs-ui/components/card';
import { ServerCodeBlock } from 'fumadocs-ui/components/codeblock.rsc';
import { baseOptions } from '@/lib/layout.shared';

const INSTALL = `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/marcrdgz/kumo/releases/latest/download/kumo-installer.sh | sh`;

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
      <main className="relative flex flex-1 flex-col items-center px-4 pb-24 text-center">
        <div
          aria-hidden
          className="spider-halftone pointer-events-none absolute inset-x-0 top-0 h-[560px]"
        />
        <div className="relative mt-20 mb-6 flex flex-col items-center">
          <h1 className="spider-gradient-text mb-1 mt-6 text-5xl font-bold tracking-tight sm:text-6xl">
            Kumo
          </h1>
          <p className="text-md font-medium text-fd-muted-foreground">
            蜘蛛 — <span className="italic">spider</span> in Japanese.
          </p>
        </div>

        <p className="relative max-w-2xl text-xl font-semibold sm:text-2xl">
          Weave your AI agents into a single web
        </p>
        <p className="relative mt-3 max-w-2xl text-fd-muted-foreground">
          A featherweight terminal multiplexer built in Rust. Every pane is a
          real Ghostty-emulated terminal, every AI CLI you run is a thread in
          the web, and the sidebar always tells you what each agent is doing.
        </p>

        <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
          <Link
            href="/docs"
            className="inline-flex h-10 items-center rounded-md bg-fd-primary px-5 text-sm font-medium text-fd-primary-foreground transition-colors hover:bg-fd-primary/90"
          >
            Get Started
          </Link>
          <Link
            href="https://github.com/marcrdgz/kumo"
            className="inline-flex h-10 items-center rounded-md border border-fd-border bg-fd-card px-5 text-sm font-medium transition-colors hover:bg-fd-accent"
          >
            GitHub
          </Link>
        </div>

        <div className="relative mt-10 w-full max-w-2xl text-left">
          <ServerCodeBlock code={INSTALL} lang="sh" />
          <p className="mt-2 text-center text-xs text-fd-muted-foreground">
            Featherweight by design: kumo idles at ~0.7% CPU and ~6 MB of
            memory while you work. 🪶
          </p>
        </div>

        <div className="relative mt-16 w-full max-w-4xl text-left">
          <Cards>
            <Card
              title="Real terminal emulation"
              description="Each pane is a genuine VT/xterm emulator (vendored Ghostty) — shells, TUIs, and full-screen apps behave exactly like in a native terminal."
              href="/docs/introduction"
            />
            <Card
              title="Split panes & sessions"
              description="Binary split tree, mouse-drag resizing, zoom, multiple independent sessions."
              href="/docs/guides/sessions-and-panes"
            />
            <Card
              title="AI CLI panes"
              description="Spawn opencode, claude, codex… with one keystroke; kumo auto-detects agents in any pane."
              href="/docs/guides/agents"
            />
            <Card
              title="Agent status at a glance"
              description="Green working, orange blocked, gray idle — detected from the live buffer, with audible alerts."
              href="/docs/guides/agents"
            />
            <Card
              title="Detach & re-attach"
              description="The daemon keeps your web alive in the background; come back from any terminal, exactly where you left it."
              href="/docs/guides/detach-and-reattach"
            />
            <Card
              title="Configurable"
              description="Leader keys, bindings, shells, AI commands — all in one config.toml."
              href="/docs/configuration"
            />
          </Cards>
        </div>
      </main>
    </HomeLayout>
  );
}
