import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

const raw = process.env.DOCS_BASE_PATH;
const basePath =
  raw !== undefined
    ? raw === '/'
      ? ''
      : raw
    : process.env.NODE_ENV === 'production'
      ? '/kumo'
      : '';

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  output: 'export',
  trailingSlash: true,
  images: { unoptimized: true },
  basePath: basePath || undefined,
  env: {
    NEXT_PUBLIC_BASE_PATH: basePath,
  },
};

export default withMDX(config);
