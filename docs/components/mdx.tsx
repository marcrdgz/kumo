import defaultMdxComponents from 'fumadocs-ui/mdx';
import { Card as BaseCard, type CardProps } from 'fumadocs-ui/components/card';
import type { MDXComponents } from 'mdx/types';
import { AgentSkillInstaller } from './agent-skill-installer';
import { ConfigBuilder } from './config-builder';

/** Fumadocs card with a hook for the Mintlify-style card styling in global.css. */
function Card({ className, ...props }: CardProps) {
  return <BaseCard className={['docs-card', className].filter(Boolean).join(' ')} {...props} />;
}

export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    Card,
    AgentSkillInstaller,
    ConfigBuilder,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
