import defaultMdxComponents from 'fumadocs-ui/mdx';
import type { MDXComponents } from 'mdx/types';
import { AgentSkillInstaller } from './agent-skill-installer';
import { ConfigBuilder } from './config-builder';

export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    AgentSkillInstaller,
    ConfigBuilder,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
