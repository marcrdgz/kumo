import type { SVGProps } from 'react';

/**
 * The kumo mark: a jumping spider's eyes (two principal, two lateral, two
 * posterior) on a 64-unit grid. `status` names the agent state each eye
 * takes in the coloured variant; the posterior pair stays neutral.
 */
export const EYES = [
  { cx: 21, cy: 38, r: 9, status: 'working' },
  { cx: 43, cy: 38, r: 9, status: 'blocked' },
  { cx: 6, cy: 40, r: 4.5, status: 'done' },
  { cx: 58, cy: 40, r: 4.5, status: 'idle' },
  { cx: 15, cy: 21, r: 3, status: null },
  { cx: 49, cy: 21, r: 3, status: null },
] as const;

export function KumoMark(props: SVGProps<SVGSVGElement>) {
  return (
    <svg viewBox="0 0 64 64" fill="currentColor" aria-hidden {...props}>
      {EYES.map(({ cx, cy, r }) => (
        <circle key={`${cx}-${cy}`} cx={cx} cy={cy} r={r} />
      ))}
    </svg>
  );
}
