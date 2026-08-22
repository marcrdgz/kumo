import defaultMdxComponents from 'fumadocs-ui/mdx';
import type { MDXComponents } from 'mdx/types';
import { ConfigBuilder } from './config-builder';

export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    ConfigBuilder,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
