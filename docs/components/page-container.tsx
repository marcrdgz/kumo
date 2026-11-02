'use client';

import type { ComponentProps } from 'react';
import { useDocsPage } from 'fumadocs-ui/layouts/notebook/page';

/**
 * Page container slot for `DocsPage`. The stock slot wraps the article in a
 * `display: contents` `<main>`, and the page's other top-level children (the
 * TOC and its popover) are sticky. Next.js skips both kinds of element when it
 * looks for the segment to scroll into view, so navigating between pages kept
 * the previous scroll offset. A real `<main>` gives it a box to measure.
 */
export function PageContainer(props: ComponentProps<'main'>) {
  const { full } = useDocsPage();
  return (
    <main
      id="nd-page"
      data-full={full}
      {...props}
      className={[
        'flex flex-col [grid-area:main] px-4 py-6 gap-4 md:px-6 md:pt-8 xl:px-8 xl:pt-14',
        full ? '*:max-w-[1285px]' : '*:max-w-[900px]',
        props.className,
      ]
        .filter(Boolean)
        .join(' ')}
    />
  );
}
