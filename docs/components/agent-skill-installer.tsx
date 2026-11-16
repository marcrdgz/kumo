'use client';

import { useState } from 'react';
import { CopyAnnouncement, useCopy } from '@/components/use-copy';

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
  const { state, copy, reset } = useCopy();
  const agent = AGENTS.find((item) => item.id === selected) ?? AGENTS[0];

  return (
    <div className="not-prose my-6 overflow-hidden rounded-xl border border-fd-border bg-fd-card">
      {/* A native radio group: arrow keys, focus and the selected state come for free. */}
      <fieldset className="flex flex-wrap gap-1 border-b border-fd-border bg-fd-muted/40 p-2">
        <legend className="sr-only">Agent to install the skill for</legend>
        {AGENTS.map((item) => (
          <label
            key={item.id}
            className="cursor-pointer rounded-md px-3 py-1.5 text-sm font-medium text-fd-muted-foreground outline-fd-primary transition-colors hover:bg-fd-muted hover:text-fd-foreground has-checked:bg-fd-primary has-checked:text-fd-primary-foreground has-focus-visible:outline-2 has-focus-visible:outline-offset-2"
          >
            <input
              type="radio"
              name="agent-skill-target"
              value={item.id}
              checked={item.id === selected}
              onChange={() => {
                setSelected(item.id);
                reset();
              }}
              className="sr-only"
            />
            {item.label}
          </label>
        ))}
      </fieldset>

      <div className="space-y-3 p-4">
        <div>
          <p className="text-sm font-medium text-fd-foreground">
            Install for {agent.label}
          </p>
          <p className="mt-1 text-xs text-fd-muted-foreground">
            Personal skill · available in every project · {agent.path}
          </p>
        </div>

        <div className="flex items-center gap-2 rounded-lg border border-fd-border bg-fd-background p-2 pl-3">
          <code className="min-w-0 flex-1 overflow-x-auto font-mono text-sm whitespace-nowrap text-fd-foreground [scrollbar-width:thin]">
            {agent.command}
          </code>
          <button
            type="button"
            onClick={() => copy(agent.command)}
            aria-label={`Copy ${agent.label} installation command`}
            className={`shrink-0 cursor-pointer rounded-md border px-2.5 py-1 text-xs font-medium outline-fd-primary transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 ${
              state === 'copied'
                ? 'border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                : state === 'failed'
                  ? 'border-red-500/40 bg-red-500/10 text-red-600 dark:text-red-400'
                  : 'border-fd-border text-fd-muted-foreground hover:bg-fd-muted hover:text-fd-foreground'
            }`}
          >
            {state === 'copied' ? '✓ copied' : state === 'failed' ? 'copy failed' : 'copy'}
          </button>
          <CopyAnnouncement state={state} />
        </div>
      </div>
    </div>
  );
}
