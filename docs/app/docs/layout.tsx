import Link from 'next/link';
import { source } from '@/lib/source';
import { DocsLayout } from 'fumadocs-ui/layouts/notebook';
import { baseOptions } from '@/lib/layout.shared';

// Notebook layout with a full-width top bar: header spans the page, sidebar
// and content sit below it, like Mintlify.
export default function Layout({ children }: LayoutProps<'/docs'>) {
  const base = baseOptions();
  return (
    <DocsLayout
      {...base}
      tree={source.getPageTree()}
      nav={{ ...base.nav, mode: 'top' }}
      links={[
        { text: 'Releases', url: 'https://github.com/marcrdgz/kumo/releases', external: true },
        {
          type: 'custom',
          on: 'nav',
          secondary: true,
          children: (
            <Link
              href="/docs/getting-started/installation"
              className="ms-1 inline-flex h-8 items-center rounded-full bg-fd-primary px-3.5 text-sm font-medium text-fd-primary-foreground transition-opacity hover:opacity-90"
            >
              Install
            </Link>
          ),
        },
      ]}
    >
      {children}
    </DocsLayout>
  );
}
