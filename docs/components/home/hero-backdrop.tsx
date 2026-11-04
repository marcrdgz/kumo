import { EYES } from '@/components/kumo-mark';

/** Lifts the top of the page so the hero has volume, not a flat fill. */
export function HeroBackdrop() {
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-x-0 top-0 -z-10 h-[980px] bg-[radial-gradient(ellipse_75%_60%_at_68%_0%,color-mix(in_oklab,var(--color-fd-foreground)_6%,transparent),transparent_72%)]"
    />
  );
}

// Mark width in px; the eyes' centre line (y = 38 of 64) sits on the
// window's top edge, so the upper halves peek out above it.
const WIDTH = 1000;
const TOP = -(38 / 64) * WIDTH;

/**
 * The kumo eyes peeking over the demo window, like a spider watching from
 * behind it. Render inside the demo's positioned wrapper, before the
 * window, so it is anchored to the window at every viewport width. Only
 * the principal and lateral eyes: the posterior pair would sit behind the
 * hero copy (as in the mark's small-size reduction). The "blocked" eye
 * glows amber, as in the mark's status variant.
 */
export function PeekingEyes() {
  return (
    <svg
      aria-hidden
      viewBox="0 0 64 64"
      // Below md the eyes crowd the stacked hero copy.
      className="pointer-events-none absolute left-1/2 -z-10 hidden max-w-none -translate-x-1/2 text-fd-foreground md:block"
      style={{ width: WIDTH, top: TOP }}
    >
      <defs>
        <filter id="eye-glow" x="-100%" y="-100%" width="300%" height="300%">
          <feGaussianBlur stdDeviation="3" />
        </filter>
      </defs>
      {EYES.filter(({ r }) => r > 3).map(({ cx, cy, r, status }) =>
        status === 'blocked' ? (
          <g key={`${cx}-${cy}`}>
            <circle cx={cx} cy={cy} r={r} fill="var(--color-status-blocked)" opacity="0.16" filter="url(#eye-glow)" />
            <circle cx={cx} cy={cy} r={r} fill="var(--color-status-blocked)" opacity="0.05" />
          </g>
        ) : (
          <circle key={`${cx}-${cy}`} cx={cx} cy={cy} r={r} fill="currentColor" opacity="0.07" />
        ),
      )}
    </svg>
  );
}
