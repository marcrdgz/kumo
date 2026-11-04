// Next does not apply the basePath to metadata URLs, so the base carries it.
export const SITE_URL = `https://marcrdgz.github.io${process.env.NEXT_PUBLIC_BASE_PATH ?? ''}/`;

/** Rendered by app/og.png/route.tsx; relative to SITE_URL. */
export const OG_IMAGE = {
  url: 'og.png',
  width: 1200,
  height: 630,
  alt: 'kumo: tmux, if it knew what your agents were doing.',
};
