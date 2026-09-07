import { defineCollection, z } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    schema: docsSchema({
      extend: z.object({
        // C-005: description is mandatory — it replaces seo.py's scraped
        // first paragraph and drives the meta description and OG pair.
        // Required, never .optional(): a page without one must fail the build.
        description: z.string(),
      }),
    }),
  }),
};
