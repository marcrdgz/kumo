import { getMarkdownUrl, getPageSection, source } from '@/lib/source';
import {
  DocsBody,
  DocsDescription,
  DocsPage,
  DocsTitle,
  MarkdownCopyButton,
  ViewOptionsPopover,
} from 'fumadocs-ui/layouts/notebook/page';
import { notFound } from 'next/navigation';
import { getMDXComponents } from '@/components/mdx';
import type { Metadata } from 'next';
import { createRelativeLink } from 'fumadocs-ui/mdx';

const GITHUB_CONTENT = 'https://github.com/marcrdgz/kumo/blob/main/docs/content/docs';

export default async function Page(props: PageProps<'/docs/[[...slug]]'>) {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  const MDX = page.data.body;
  const section = getPageSection(page);
  const markdownUrl = getMarkdownUrl(page);

  return (
    <DocsPage
      toc={page.data.toc}
      full={page.data.full}
      breadcrumb={{ enabled: false }}
    >
      <header className="flex flex-col gap-2 pb-6">
        {section && <p className="text-sm font-semibold text-fd-primary">{section}</p>}
        <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
          <DocsTitle className="text-[2rem] leading-tight font-semibold">
            {page.data.title}
          </DocsTitle>
          <div className="flex shrink-0 items-center gap-2 pt-1.5">
            <MarkdownCopyButton markdownUrl={markdownUrl}>Copy page</MarkdownCopyButton>
            <ViewOptionsPopover
              markdownUrl={markdownUrl}
              githubUrl={`${GITHUB_CONTENT}/${page.path}`}
            />
          </div>
        </div>
        <DocsDescription className="mb-0 text-lg">{page.data.description}</DocsDescription>
      </header>
      <DocsBody>
        <MDX
          components={getMDXComponents({
            a: createRelativeLink(source, page),
          })}
        />
      </DocsBody>
    </DocsPage>
  );
}

export async function generateStaticParams() {
  return source.generateParams();
}

export async function generateMetadata(props: PageProps<'/docs/[[...slug]]'>): Promise<Metadata> {
  const params = await props.params;
  const page = source.getPage(params.slug);
  if (!page) notFound();

  return {
    title: page.data.title,
    description: page.data.description,
  };
}
