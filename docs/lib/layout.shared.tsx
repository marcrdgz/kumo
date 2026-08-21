import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <span className="font-semibold tracking-tight">kumo</span>
      ),
    },
    githubUrl: 'https://github.com/marcrdgz/kumo',
  };
}
