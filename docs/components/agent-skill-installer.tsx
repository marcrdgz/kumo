'use client';

import { useRef, useState } from 'react';

type AgentId = 'codex' | 'claude' | 'gemini' | 'opencode';

interface AgentTarget {
  id: AgentId;
  label: string;
  path: string;
  command: string;
}

const AGENTS: AgentTarget[] = [
  {
    id: 'codex',
    label: 'Codex',
    path: '~/.agents/skills/kumo/SKILL.md',
    command: 'kumo agent skill --output ~/.agents/skills/kumo/SKILL.md',
  },
  {
    id: 'claude',
    label: 'Claude Code',
    path: '~/.claude/skills/kumo/SKILL.md',
    command: 'kumo agent skill --output ~/.claude/skills/kumo/SKILL.md',
  },
  {
    id: 'gemini',
    label: 'Gemini CLI',
    path: '~/.gemini/skills/kumo/SKILL.md',
    command: 'kumo agent skill --output ~/.gemini/skills/kumo/SKILL.md',
  },
  {
    id: 'opencode',
    label: 'OpenCode',
    path: '~/.config/opencode/skills/kumo/SKILL.md',
    command:
      'kumo agent skill --output ~/.config/opencode/skills/kumo/SKILL.md',
  },
];

export function AgentSkillInstaller() {
  const [selected, setSelected] = useState<AgentId>('codex');
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | null>(null);
  const agent = AGENTS.find((item) => item.id === selected) ?? AGENTS[0];

  async function copy() {
    try {
      await navigator.clipboard.writeText(agent.command);
    } catch {
      const textarea = document.createElement('textarea');
      textarea.value = agent.command;
      document.body.appendChild(textarea);
      textarea.select();
      document.execCommand('copy');
      textarea.remove();
    }
    setCopied(true);
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 1600);
  }

  return (
    <div className="not-prose my-6 overflow-hidden rounded-xl border border-fd-border bg-fd-card">
      <div
        className="flex flex-wrap gap-1 border-b border-fd-border bg-fd-muted/40 p-2"
        role="tablist"
        aria-label="Agent skill installation target"
      >
        {AGENTS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={item.id === selected}
            onClick={() => {
              setSelected(item.id);
              setCopied(false);
            }}
            className={`cursor-pointer rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
              item.id === selected
                ? 'bg-fd-primary text-fd-primary-foreground'
                : 'text-fd-muted-foreground hover:bg-fd-muted hover:text-fd-foreground'
            }`}
          >
            {item.label}
          </button>
        ))}
      </div>

      <div role="tabpanel" className="space-y-3 p-4">
        <div>
          <p className="text-sm font-medium text-fd-foreground">
            Install for {agent.label}
          </p>
          <p className="mt-1 text-xs text-fd-muted-foreground">
            Personal skill · available in every project · {agent.path}
          </p>
        </div>

        <div className="flex items-center gap-2 rounded-lg border border-fd-border bg-fd-background p-2 pl-3">
          <code className="min-w-0 flex-1 overflow-x-auto font-mono text-sm whitespace-nowrap text-fd-foreground">
            {agent.command}
          </code>
          <button
            type="button"
            onClick={copy}
            aria-label={`Copy ${agent.label} installation command`}
            className={`shrink-0 cursor-pointer rounded-md border px-2.5 py-1 text-xs font-medium transition-colors ${
              copied
                ? 'border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                : 'border-fd-border text-fd-muted-foreground hover:bg-fd-muted hover:text-fd-foreground'
            }`}
          >
            {copied ? '✓ copied' : 'copy'}
          </button>
        </div>
      </div>
    </div>
  );
}
