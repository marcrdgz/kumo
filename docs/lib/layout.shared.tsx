import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <span className="font-display text-[15px] font-medium tracking-[-0.04em]">kumo</span>
      ),
    },
    githubUrl: 'https://github.com/marcrdgz/kumo',
  };
}
