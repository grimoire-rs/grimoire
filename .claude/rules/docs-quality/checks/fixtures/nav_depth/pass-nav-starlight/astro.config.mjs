// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Four groups, one items: level each, no group nested in a group and no bare
// top-level link: depth 2, zero bare entries, nothing expanded at level 3.
export default defineConfig({
  site: 'https://example.test',
  integrations: [
    starlight({
      title: 'Example Docs',
      sidebar: [
        {
          label: 'Start here',
          items: [
            { slug: 'introduction' },
            { slug: 'installation' },
            { slug: 'quickstart' },
          ],
        },
        {
          label: 'Everyday',
          items: [
            { slug: 'commands' },
            { slug: 'configuration' },
            { slug: 'clients' },
          ],
        },
        {
          label: 'Integrate',
          items: [
            { slug: 'ci' },
            { slug: 'publishing' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { slug: 'json-interface' },
            { slug: 'stability' },
          ],
        },
      ],
    }),
  ],
});
