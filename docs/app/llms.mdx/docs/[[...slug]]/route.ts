import { notFound } from 'next/navigation';
import { getMarkdownParams, getPageFromMarkdownSlug, getPageMarkdown } from '@/lib/source';

export const dynamic = 'force-static';

export async function GET(_req: Request, { params }: RouteContext<'/llms.mdx/docs/[[...slug]]'>) {
  const { slug = [] } = await params;
  const page = getPageFromMarkdownSlug(slug);
  if (!page) notFound();

  return new Response(await getPageMarkdown(page), {
    headers: { 'Content-Type': 'text/markdown; charset=utf-8' },
  });
}

export function generateStaticParams() {
  return getMarkdownParams();
}
