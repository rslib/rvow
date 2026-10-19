import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import rslibTheme from 'starlight-theme-rslib';

export default defineConfig({
  site: 'https://rslib.github.io',
  base: '/rvow',
  integrations: [
    starlight({
      title: 'rvow',
      description: 'Hold a coding agent to an accepted plan.',
      favicon: '/favicon.svg',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/rslib/rvow' }],
      plugins: [rslibTheme(), starlightLinksValidator()],
      sidebar: [
        { label: 'Start', items: [{ label: 'rvow', slug: '' }] },
        { label: 'Guides', items: [{ autogenerate: { directory: 'guides' } }] },
        {
          label: 'Reference',
          items: [
            { label: 'Commands overview', slug: 'reference/commands' },
            { label: 'Commands', collapsed: true, items: [{ autogenerate: { directory: 'reference/commands' } }] },
          ],
        },
      ],
    }),
  ],
  // npm links the theme from .theme, so Vite may serve its fonts from there.
  vite: { server: { fs: { allow: ['.', './.theme/starlight'] } } },
});
