import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { KumoMark } from '@/components/kumo-mark';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <span className="inline-flex items-center gap-2">
          <KumoMark className="size-5" />
          <span className="font-display text-[15px] font-medium tracking-[-0.04em]">kumo</span>
        </span>
      ),
    },
    githubUrl: 'https://github.com/marcrdgz/kumo',
  };
}
