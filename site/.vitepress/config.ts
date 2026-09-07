import { defineConfig } from 'vitepress'

// concord-bots documentation site
// Build:  npm run docs:build   → static output in site/dist/
// Serve:  npm run docs:preview
// Publish: see guide/deployment.md → "Publishing to nsite"
export default defineConfig({
  title: 'concord-bots',
  description:
    'A Vector/Concord Protocol bot template for AI agents. Build custom Nostr bots without Rust expertise.',
  lang: 'en-US',
  outDir: 'dist',
  cleanUrls: false,

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }],
  ],

  themeConfig: {
    // Purple accent to match the project's branding
    logo: '/favicon.svg',

    nav: [
      { text: 'Guide', link: '/guide/getting-started' },
      { text: 'Commands', link: '/guide/commands' },
      { text: 'Configuration', link: '/guide/configuration' },
      { text: 'Examples', link: '/guide/examples' },
      {
        text: 'GitHub',
        link: 'https://github.com/CentauriAgent/concord-bots',
      },
    ],

    sidebar: [
      {
        text: 'Introduction',
        items: [{ text: 'What is concord-bots?', link: '/' }],
      },
      {
        text: 'Guide',
        items: [
          { text: 'Getting Started', link: '/guide/getting-started' },
          { text: 'Command Reference', link: '/guide/commands' },
          { text: 'Configuration', link: '/guide/configuration' },
          { text: 'Custom Handler Tutorial', link: '/guide/custom-handlers' },
          { text: 'Example Bots', link: '/guide/examples' },
          { text: 'Deployment', link: '/guide/deployment' },
        ],
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/CentauriAgent/concord-bots' },
    ],

    outline: {
      level: [2, 3],
    },

    search: {
      provider: 'local',
    },

    footer: {
      message: 'Released under the MIT License.',
      copyright: 'concord-bots — Vector/Concord Protocol bot template',
    },
  },
})
