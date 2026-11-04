import { loader, type InferPageType } from 'fumadocs-core/source';
import { lucideIconsPlugin } from 'fumadocs-core/source/plugins/lucide-icons';
import { defineDocs } from 'fumadocs-mdx/macro';

const docs = defineDocs({
  dir: 'content/docs',
  // Processed Markdown backs the "Copy page" action and its `.md` endpoint.
  docs: { postprocess: { includeProcessedMarkdown: true } },
});

export const source = loader({
  baseUrl: '/docs',
  source: docs.toFumadocsSource(),
  plugins: [lucideIconsPlugin()],
});

export type DocPage = InferPageType<typeof source>;

const MARKDOWN_FILE = 'content.md';

/** Static Markdown export of a page, served by `app/llms.mdx`. */
export function getMarkdownUrl(page: DocPage): string {
  return ['/llms.mdx/docs', ...page.slugs, MARKDOWN_FILE].join('/');
}

export function getMarkdownParams(): { slug: string[] }[] {
  return source.getPages().map((page) => ({ slug: [...page.slugs, MARKDOWN_FILE] }));
}

export function getPageFromMarkdownSlug(slug: string[]): DocPage | undefined {
  if (slug.at(-1) !== MARKDOWN_FILE) return undefined;
  return source.getPage(slug.slice(0, -1));
}

export async function getPageMarkdown(page: DocPage): Promise<string> {
  const body = await page.data.getText('processed');
  return `# ${page.data.title}\n\n${page.data.description ?? ''}\n\n${body}`;
}

/**
 * The sidebar section a page sits under: the nearest separator above it in
 * the page tree. Sections are flat separators (not folders) so the sidebar
 * reads as labelled groups, which leaves breadcrumbs with nothing to show.
 */
export function getPageSection(page: DocPage): string | undefined {
  let section: string | undefined;
  for (const node of source.getPageTree().children) {
    if (node.type === 'separator') section = typeof node.name === 'string' ? node.name : undefined;
    else if (node.type === 'page' && node.url === page.url) return section;
  }
  return undefined;
}
