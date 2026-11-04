import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { ImageResponse } from 'next/og';
import { EYES } from '@/components/kumo-mark';

// Satori can't read the CSS custom properties, so the status hexes repeat here.
const STATUS_COLOR = {
  working: '#0a84ff',
  blocked: '#ff9f0a',
  done: '#30d158',
  idle: '#8e8e93',
};

/*
 * Social preview image, rendered once at build time. A route named og.png
 * rather than an `opengraph-image` file convention: the static export
 * writes that one without an extension, which GitHub Pages serves as
 * application/octet-stream. Wired up in the root layout's metadata.
 */

const SIZE = { width: 1200, height: 630 };
export const dynamic = 'force-static';

// Satori reads woff but not woff2, so the font comes from @fontsource
// rather than next/font.
const FONT = 'node_modules/@fontsource/martian-mono/files/martian-mono-latin-500-normal.woff';

const AGENTS = [
  { name: 'web', cli: 'opencode', color: STATUS_COLOR.blocked, label: 'blocked' },
  { name: 'docs', cli: 'codex', color: STATUS_COLOR.done, label: 'done' },
  { name: 'api', cli: 'claude', color: STATUS_COLOR.working, label: '' },
  { name: 'infra', cli: 'zsh', color: STATUS_COLOR.idle, label: '' },
];

export async function GET() {
  const font = await readFile(join(process.cwd(), FONT));

  return new ImageResponse(
    (
      <div
        style={{
          display: 'flex',
          width: '100%',
          height: '100%',
          padding: 72,
          background: '#000000',
          color: '#f5f5f7',
          fontFamily: 'Martian Mono',
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'column', flex: 1 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 16 }}>
            <svg width="44" height="44" viewBox="0 0 64 64">
              {EYES.map(({ cx, cy, r, status }) => (
                <circle
                  key={`${cx}-${cy}`}
                  cx={cx}
                  cy={cy}
                  r={r}
                  fill={status ? STATUS_COLOR[status] : '#f5f5f7'}
                />
              ))}
            </svg>
            <div style={{ fontSize: 34, letterSpacing: '-0.04em' }}>kumo</div>
          </div>
          <div
            style={{
              marginTop: 'auto',
              fontSize: 64,
              lineHeight: 1.12,
              letterSpacing: '-0.045em',
              maxWidth: 640,
            }}
          >
            tmux, if it knew what your agents were doing.
          </div>
          <div style={{ marginTop: 28, fontSize: 22, color: '#a1a1a6', letterSpacing: '-0.02em' }}>
            A terminal multiplexer for AI coding agents
          </div>
        </div>

        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignSelf: 'center',
            width: 330,
            padding: '28px 32px',
            gap: 22,
            borderRadius: 20,
            border: '1px solid rgba(255,255,255,0.1)',
            background: '#0d0d0f',
            fontSize: 22,
          }}
        >
          <div style={{ color: 'rgba(255,255,255,0.35)' }}>agents</div>
          {AGENTS.map((agent) => (
            <div key={agent.name} style={{ display: 'flex', flexDirection: 'column' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 14 }}>
                <div style={{ width: 13, height: 13, borderRadius: 13, background: agent.color }} />
                <div style={{ color: agent.label ? '#ffffff' : 'rgba(255,255,255,0.75)' }}>
                  {agent.name}
                </div>
                {agent.label && <div style={{ color: agent.color }}>{agent.label}</div>}
              </div>
              <div style={{ paddingLeft: 27, color: 'rgba(255,255,255,0.3)', fontSize: 18 }}>
                {agent.cli}
              </div>
            </div>
          ))}
        </div>
      </div>
    ),
    { ...SIZE, fonts: [{ name: 'Martian Mono', data: font, weight: 500 }] },
  );
}
