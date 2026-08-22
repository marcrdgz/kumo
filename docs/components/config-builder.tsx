'use client';

import { useMemo, useRef, useState } from 'react';

type Position =
  | 'top-left'
  | 'top-right'
  | 'center'
  | 'bottom-left'
  | 'bottom-right'
  | 'off';

interface Row {
  id: number;
  chord: string;
  action: string;
}

interface BuilderState {
  leader: string;
  rows: Row[];
  aiCmd: string;
  shell: string;
  updateCheck: boolean;
  position: Position;
  sound: boolean;
  blocked: boolean;
  finished: boolean;
}

const ACTION_GROUPS: { label: string; actions: string[] }[] = [
  {
    label: 'layout',
    actions: [
      'split-vertical',
      'split-horizontal',
      'zoom',
      'focus-left',
      'focus-down',
      'focus-up',
      'focus-right',
      'resize-left',
      'resize-down',
      'resize-up',
      'resize-right',
    ],
  },
  {
    label: 'panes',
    actions: [
      'split-ai',
      'close-pane',
      'cycle-pane',
      'swap-panes',
      'rotate-layout',
      'show-pane-numbers',
    ],
  },
  {
    label: 'tabs',
    actions: [
      'new-tab',
      'close-tab',
      'rename-tab',
      'next-tab',
      'prev-tab',
      'jump-tab-1',
      'jump-tab-2',
      'jump-tab-3',
      'jump-tab-4',
      'jump-tab-5',
      'jump-tab-6',
      'jump-tab-7',
      'jump-tab-8',
      'jump-tab-9',
    ],
  },
  {
    label: 'sessions',
    actions: [
      'new-session',
      'new-worktree',
      'next-session',
      'prev-session',
      'jump-session-1',
      'jump-session-2',
      'jump-session-3',
      'jump-session-4',
      'jump-session-5',
      'jump-session-6',
      'jump-session-7',
      'jump-session-8',
      'jump-session-9',
    ],
  },
  { label: 'chrome', actions: ['toggle-sidebar'] },
  {
    label: 'general',
    actions: ['detach', 'show-keybinds', 'copy-mode', 'copy-mode-search'],
  },
];

const LEADER_PRESETS = ['ctrl+b', 'ctrl+a', 'ctrl+space', 'f12'];
const AI_PRESETS = ['opencode', 'claude', 'claude --model sonnet'];

const SPOT_CLASS: Record<Exclude<Position, 'off'>, string> = {
  'top-left': 'left-2 top-5',
  'top-right': 'right-2 top-5',
  center: 'left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2',
  'bottom-left': 'bottom-5 left-2',
  'bottom-right': 'bottom-5 right-2',
};

const CHORD_RE =
  /^(?:(?:ctrl|alt|shift|cmd|super)\+)*(?:f(?:1[0-2]|[1-9])|[a-z0-9!-/:-@[-`{-~])$/i;

function initial(): BuilderState {
  return {
    leader: 'ctrl+b',
    rows: [{ id: 1, chord: 's', action: 'split-vertical' }],
    aiCmd: '',
    shell: '',
    updateCheck: true,
    position: 'top-right',
    sound: true,
    blocked: true,
    finished: true,
  };
}

type SegKind = 'cmt' | 'tbl' | 'key' | 'eq' | 'str' | 'bool';

interface Seg {
  t: string;
  k: SegKind;
}

function tomlStr(v: string): string {
  return `"${v.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

function isBareKey(s: string): boolean {
  return /^[A-Za-z0-9_-]+$/.test(s);
}

function buildToml(b: BuilderState): Seg[][] {
  const lines: Seg[][] = [];

  const comment = (t: string) => lines.push([{ t: `# ${t}`, k: 'cmt' }]);
  const blank = () => lines.push([]);
  const table = (name: string) => {
    blank();
    lines.push([{ t: `[${name}]`, k: 'tbl' }]);
  };
  const kv = (
    key: string,
    rawVal: string,
    kind: Extract<SegKind, 'str' | 'bool'>,
    note?: string,
  ) => {
    const segs: Seg[] = [
      { t: key, k: 'key' },
      { t: ' = ', k: 'eq' },
      { t: kind === 'str' ? tomlStr(rawVal) : rawVal, k: kind },
    ];
    if (note) segs.push({ t: `   # ${note}`, k: 'cmt' });
    lines.push(segs);
  };

  comment('~/.config/kumo/config.toml');
  comment('Overrides only — every other option keeps its default.');

  const leaderSet = b.leader.trim() !== '' && b.leader.trim().toLowerCase() !== 'ctrl+b';
  const aiSet = b.aiCmd.trim() !== '' && b.aiCmd.trim().toLowerCase() !== 'opencode';
  const shellSet = b.shell.trim() !== '';
  const bindings = new Map<string, string>();
  for (const r of b.rows) {
    const chord = r.chord.trim();
    const action = r.action.trim();
    if (chord !== '' && action !== '') bindings.set(chord, action);
  }
  const notifChanged =
    b.position !== 'top-right' ||
    !b.sound ||
    !b.blocked ||
    !b.finished;

  const changed =
    leaderSet ||
    aiSet ||
    shellSet ||
    bindings.size > 0 ||
    !b.updateCheck ||
    notifChanged;

  if (!changed) {
    blank();
    comment('Everything is at its default — tweak the controls to generate config.');
    return lines;
  }

  if (aiSet) kv('ai-cmd', b.aiCmd.trim(), 'str');
  if (!b.updateCheck) kv('update-check', 'false', 'bool');

  if (shellSet) {
    table('terminal');
    kv('shell', b.shell.trim(), 'str');
  }

  if (leaderSet) {
    table('keymap');
    kv('leader', b.leader.trim(), 'str');
  }

  if (bindings.size > 0) {
    table('keymap.bindings');
    for (const [chord, action] of bindings) {
      const keySeg: Seg = {
        t: isBareKey(chord) ? chord : tomlStr(chord),
        k: 'key',
      };
      const segs: Seg[] = [
        keySeg,
        { t: ' = ', k: 'eq' },
        { t: tomlStr(action), k: 'str' },
      ];
      lines.push(segs);
    }
  }

  if (notifChanged) {
    table('notifications');
    if (b.position !== 'top-right') {
      kv(
        'position',
        b.position,
        'str',
        b.position === 'off' ? 'silences toasts; the chime is unaffected' : undefined,
      );
    }
    if (!b.blocked) kv('blocked', 'false', 'bool');
    if (!b.finished) kv('finished', 'false', 'bool');
    if (!b.sound) kv('sound', 'false', 'bool');
  }

  return lines;
}

function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className="inline-flex cursor-pointer items-center gap-2 text-sm"
    >
      <span
        className={`relative inline-block h-5 w-9 shrink-0 rounded-full transition-colors ${
          checked ? 'bg-fd-primary' : 'border border-fd-border bg-fd-muted'
        }`}
      >
        <span
          className={`absolute top-[3px] left-[3px] h-3.5 w-3.5 rounded-full bg-white shadow transition-transform ${
            checked ? 'translate-x-4' : ''
          }`}
        />
      </span>
      <span>{label}</span>
    </button>
  );
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`cursor-pointer rounded-md border px-2 py-1 font-mono text-xs transition-colors ${
        active
          ? 'border-transparent bg-fd-primary text-fd-primary-foreground'
          : 'border-fd-border text-fd-muted-foreground hover:bg-fd-muted hover:text-fd-foreground'
      }`}
    >
      {children}
    </button>
  );
}

function Field({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <fieldset className="space-y-2">
      <legend className="text-xs font-semibold tracking-wider text-fd-muted-foreground uppercase">
        {title}
      </legend>
      {children}
    </fieldset>
  );
}

const SEG_CLASS: Record<SegKind, string> = {
  cmt: 'text-fd-muted-foreground',
  tbl: 'text-fd-primary font-semibold',
  key: 'text-fd-foreground',
  eq: 'text-fd-muted-foreground',
  str: 'text-emerald-600 dark:text-emerald-400',
  bool: 'text-amber-600 dark:text-amber-400',
};

export function ConfigBuilder() {
  const [st, setSt] = useState<BuilderState>(initial);
  const [copied, setCopied] = useState(false);
  const nextId = useRef(2);
  const timer = useRef<number | null>(null);

  const lines = useMemo(() => buildToml(st), [st]);
  const plain = useMemo(
    () =>
      lines
        .map((segs) => segs.map((s) => s.t).join(''))
        .join('\n'),
    [lines],
  );

  const patch = (p: Partial<BuilderState>) => setSt((s) => ({ ...s, ...p }));

  const setRow = (id: number, p: Partial<Row>) =>
    setSt((s) => ({
      ...s,
      rows: s.rows.map((r) => (r.id === id ? { ...r, ...p } : r)),
    }));

  const addRow = () =>
    setSt((s) => ({
      ...s,
      rows: [...s.rows, { id: nextId.current++, chord: '', action: '' }],
    }));

  const dropRow = (id: number) =>
    setSt((s) => ({ ...s, rows: s.rows.filter((r) => r.id !== id) }));

  async function copy() {
    try {
      await navigator.clipboard.writeText(plain);
    } catch {
      const ta = document.createElement('textarea');
      ta.value = plain;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      ta.remove();
    }
    setCopied(true);
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 1600);
  }

  const leaderBad = st.leader.trim() !== '' && !CHORD_RE.test(st.leader.trim());

  return (
    <div className="not-prose my-6">
      <div className="grid gap-6 lg:grid-cols-2">
        <div className="space-y-6">
          <Field title="leader chord">
            <input
              value={st.leader}
              onChange={(e) => patch({ leader: e.target.value })}
              spellCheck={false}
              aria-label="Leader chord"
              placeholder="ctrl+b"
              className={`h-8 w-full rounded-md border bg-transparent px-2 font-mono text-sm outline-none focus-visible:border-fd-primary ${
                leaderBad ? 'border-red-500 dark:border-red-400' : 'border-fd-border'
              }`}
            />
            <div className="flex flex-wrap gap-1.5">
              {LEADER_PRESETS.map((p) => (
                <Chip
                  key={p}
                  active={st.leader === p}
                  onClick={() => patch({ leader: p })}
                >
                  {p}
                </Chip>
              ))}
            </div>
          </Field>

          <Field title="key bindings">
            <div className="space-y-2">
              {st.rows.map((r) => (
                <div key={r.id} className="flex items-center gap-2">
                  <input
                    value={r.chord}
                    onChange={(e) => setRow(r.id, { chord: e.target.value })}
                    spellCheck={false}
                    aria-label="Binding chord"
                    placeholder="s"
                    className={`h-8 w-24 shrink-0 rounded-md border bg-transparent px-2 font-mono text-sm outline-none focus-visible:border-fd-primary ${
                      r.chord.trim() !== '' && !CHORD_RE.test(r.chord.trim())
                        ? 'border-red-500 dark:border-red-400'
                        : 'border-fd-border'
                    }`}
                  />
                  <select
                    value={r.action}
                    onChange={(e) => setRow(r.id, { action: e.target.value })}
                    aria-label="Binding action"
                    className="h-8 min-w-0 flex-1 rounded-md border border-fd-border bg-transparent px-2 text-sm"
                  >
                    <option value="">choose an action…</option>
                    {ACTION_GROUPS.map((g) => (
                      <optgroup key={g.label} label={g.label}>
                        {g.actions.map((a) => (
                          <option key={a} value={a}>
                            {a}
                          </option>
                        ))}
                      </optgroup>
                    ))}
                  </select>
                  <button
                    type="button"
                    aria-label="Remove binding"
                    onClick={() => dropRow(r.id)}
                    className="h-8 w-8 shrink-0 cursor-pointer rounded-md border border-fd-border text-fd-muted-foreground transition-colors hover:bg-fd-muted hover:text-fd-foreground"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
            <button
              type="button"
              onClick={addRow}
              className="cursor-pointer rounded-md border border-dashed border-fd-border px-2.5 py-1 text-xs text-fd-muted-foreground transition-colors hover:bg-fd-muted hover:text-fd-foreground"
            >
              + add binding
            </button>
          </Field>

          <Field title="AI command">
            <input
              value={st.aiCmd}
              onChange={(e) => patch({ aiCmd: e.target.value })}
              spellCheck={false}
              aria-label="AI command"
              placeholder="opencode"
              className="h-8 w-full rounded-md border border-fd-border bg-transparent px-2 font-mono text-sm outline-none focus-visible:border-fd-primary"
            />
            <div className="flex flex-wrap gap-1.5">
              {AI_PRESETS.map((p) => (
                <Chip key={p} active={st.aiCmd === p} onClick={() => patch({ aiCmd: p })}>
                  {p}
                </Chip>
              ))}
            </div>
          </Field>

          <Field title="shell">
            <input
              value={st.shell}
              onChange={(e) => patch({ shell: e.target.value })}
              spellCheck={false}
              aria-label="Login shell"
              placeholder="$SHELL → /bin/zsh"
              className="h-8 w-full rounded-md border border-fd-border bg-transparent px-2 font-mono text-sm outline-none focus-visible:border-fd-primary"
            />
          </Field>

          <Field title="startup">
            <Toggle
              checked={st.updateCheck}
              onChange={(v) => patch({ updateCheck: v })}
              label="Check for updates on launch"
            />
          </Field>

          <Field title="agent notifications">
            <div className="relative aspect-[16/9] w-full overflow-hidden rounded-lg border border-fd-border bg-fd-background">
              <div className="flex h-4 items-end gap-1 border-b border-fd-border bg-fd-muted px-1.5 pb-0.5">
                <span className="h-2 w-8 rounded-t-sm bg-fd-border" />
                <span className="h-2 w-8 rounded-t-sm bg-fd-background" />
                <span className="h-2 w-8 rounded-t-sm bg-fd-border" />
              </div>
              <div className="absolute inset-x-0 bottom-0 flex h-4 items-center justify-center border-t border-fd-border bg-fd-muted font-mono text-[8px] text-fd-muted-foreground">
                status bar
              </div>
              {(Object.keys(SPOT_CLASS) as Exclude<Position, 'off'>[]).map(
                (pos) => (
                  <button
                    key={pos}
                    type="button"
                    aria-label={`Anchor toasts ${pos}`}
                    onClick={() => patch({ position: pos })}
                    className={`absolute h-6 w-6 rounded-md transition-colors ${SPOT_CLASS[pos]} ${
                      st.position === pos
                        ? 'border border-solid border-fd-primary bg-fd-primary/20'
                        : 'border border-dashed border-fd-border hover:border-fd-primary hover:bg-fd-primary/10'
                    }`}
                  />
                ),
              )}
              {st.position !== 'off' &&
                (st.finished || st.blocked) &&
                (() => {
                  const dot = st.finished
                    ? 'text-emerald-500'
                    : 'text-amber-500';
                  const text = st.finished
                    ? 'agent finished'
                    : 'agent blocked';
                  return (
                    <span
                      className={`pointer-events-none absolute flex items-center gap-1 rounded-md border border-fd-border bg-fd-popover px-1.5 py-0.5 font-mono text-[9px] whitespace-nowrap shadow-sm ${SPOT_CLASS[st.position]} ${
                        st.position === 'center'
                          ? '-translate-x-1/2 -translate-y-[calc(50%+14px)]'
                          : ''
                      }`}
                    >
                      <span className={dot}>●</span> {text}
                    </span>
                  );
                })()}
              {st.position === 'off' && (
                <span className="pointer-events-none absolute inset-0 flex items-center justify-center font-mono text-[10px] text-fd-muted-foreground">
                  toasts off
                </span>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              <Chip
                active={st.position === 'off'}
                onClick={() => patch({ position: 'off' })}
              >
                off
              </Chip>
              <span className="text-xs text-fd-muted-foreground">
                click a zone to anchor the toast there
              </span>
            </div>
            <div className="flex flex-wrap gap-x-5 gap-y-2 pt-1">
              <Toggle
                checked={st.sound}
                onChange={(v) => patch({ sound: v })}
                label="chime"
              />
              <Toggle
                checked={st.blocked}
                onChange={(v) => patch({ blocked: v })}
                label="notify on blocked"
              />
              <Toggle
                checked={st.finished}
                onChange={(v) => patch({ finished: v })}
                label="notify on finished"
              />
            </div>
          </Field>
        </div>

        <div className="self-start lg:sticky lg:top-16">
          <div className="overflow-hidden rounded-xl border border-fd-border bg-fd-card font-mono text-xs">
            <div className="flex items-center gap-2 border-b border-fd-border bg-fd-muted px-3 py-2">
              <span className="h-2.5 w-2.5 rounded-full bg-red-500/80" />
              <span className="h-2.5 w-2.5 rounded-full bg-amber-500/80" />
              <span className="h-2.5 w-2.5 rounded-full bg-emerald-500/80" />
              <span className="truncate text-fd-muted-foreground">
                ~/.config/kumo/config.toml
              </span>
              <span className="ml-auto flex shrink-0 gap-1.5">
                <button
                  type="button"
                  onClick={copy}
                  className={`cursor-pointer rounded-md border px-2 py-0.5 transition-colors ${
                    copied
                      ? 'border-transparent bg-emerald-600 text-white'
                      : 'border-fd-border text-fd-muted-foreground hover:bg-fd-muted hover:text-fd-foreground'
                  }`}
                >
                  {copied ? '✓ copied' : 'copy'}
                </button>
                <button
                  type="button"
                  onClick={() => setSt(initial())}
                  className="cursor-pointer rounded-md border border-fd-border px-2 py-0.5 text-fd-muted-foreground transition-colors hover:bg-fd-muted hover:text-fd-foreground"
                >
                  reset
                </button>
              </span>
            </div>
            <pre className="overflow-x-auto p-4 leading-relaxed">
              {lines.map((segs, i) => (
                <div key={i}>
                  {segs.length === 0 ? (
                    <br />
                  ) : (
                    segs.map((s, j) => (
                      <span key={j} className={SEG_CLASS[s.k]}>
                        {s.t}
                      </span>
                    ))
                  )}
                </div>
              ))}
            </pre>
          </div>
          <p className="mt-2 text-xs text-fd-muted-foreground">
            Live-generated — only options that differ from the defaults are
            written, in canonical form (`[terminal] shell`, `[keymap] leader`).
          </p>
        </div>
      </div>
    </div>
  );
}

export default ConfigBuilder;
