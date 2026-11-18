import { defineUserConfig } from 'vuepress'
import { defaultTheme } from '@vuepress/theme-default'
import { viteBundler } from '@vuepress/bundler-vite'

export default defineUserConfig({
  base: '/rustgrid/',
  lang: 'en-US',
  title: 'RustGrid',
  description: 'A lightweight, native, Navicat-style database GUI written in Rust.',
  head: [
    ['link', { rel: 'icon', href: '/rustgrid/images/logo.png' }],
    ['meta', { name: 'theme-color', content: '#3b82f6' }],
    [
      'script',
      {},
      `(function () {
  try {
    var base = '/rustgrid/';
    var pages = [
      '',
      'index.html',
      'guide/install.html',
      'guide/connections.html',
      'guide/settings.html',
      'guide/tables.html',
      'guide/functions.html',
      'guide/queries.html',
      'guide/backup.html',
      'guide/sync.html',
      'guide/engines.html',
      'guide/shortcuts.html'
    ];
    var path = window.location.pathname;
    if (path.indexOf(base) !== 0) return;
    var rel = path.slice(base.length);
    if (rel.indexOf('zh/') === 0 || pages.indexOf(rel) === -1) return;
    var saved = window.localStorage.getItem('rg-locale');
    if (saved === 'en') return;
    var prefersChinese = /^zh/i.test(window.navigator.language || '');
    if (saved === 'zh' || (!saved && prefersChinese)) {
      window.location.replace(base + 'zh/' + rel + window.location.search + window.location.hash);
    }
  } catch (error) {
    /* storage unavailable: keep the page as requested */
  }
})();`,
    ],
  ],

  locales: {
    '/': {
      lang: 'en-US',
      title: 'RustGrid',
      description: 'A lightweight, native, Navicat-style database GUI written in Rust.',
    },
    '/zh/': {
      lang: 'zh-CN',
      title: 'RustGrid',
      description: '用 Rust 编写的轻量原生数据库管理工具，操作习惯对齐 Navicat。',
    },
  },

  bundler: viteBundler(),

  theme: defaultTheme({
    logo: '/images/logo.png',
    logoDark: '/images/logo.png',
    repo: 'yisier/rustgrid',
    docsDir: '.',
    sidebarDepth: 0,
    editLink: false,
    contributors: false,
    lastUpdated: false,
    navbar: [
      { text: 'Guide', link: '/guide/install.html' },
      { text: 'Benchmark', link: '/#benchmark' },
      { text: 'Download', link: 'https://github.com/yisier/rustgrid/releases/latest' },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Getting Started',
          children: [
            '/guide/install.html',
            '/guide/connections.html',
            '/guide/settings.html',
          ],
        },
        {
          text: 'Modules',
          children: [
            '/guide/tables.html',
            '/guide/functions.html',
            '/guide/queries.html',
            '/guide/backup.html',
            '/guide/sync.html',
          ],
        },
        {
          text: 'Reference',
          children: ['/guide/engines.html', '/guide/shortcuts.html'],
        },
      ],
    },
    locales: {
      '/': {
        sidebarDepth: 0,
        selectLanguageName: 'English',
        selectLanguageText: 'Languages',
        navbar: [
          { text: 'Guide', link: '/guide/install.html' },
          { text: 'Benchmark', link: '/#benchmark' },
          { text: 'Download', link: 'https://github.com/yisier/rustgrid/releases/latest' },
        ],
        sidebar: {
          '/guide/': [
            {
              text: 'Getting Started',
              children: [
                '/guide/install.html',
                '/guide/connections.html',
                '/guide/settings.html',
              ],
            },
            {
              text: 'Modules',
              children: [
                '/guide/tables.html',
                '/guide/functions.html',
                '/guide/queries.html',
                '/guide/backup.html',
                '/guide/sync.html',
              ],
            },
            {
              text: 'Reference',
              children: ['/guide/engines.html', '/guide/shortcuts.html'],
            },
          ],
        },
      },
      '/zh/': {
        sidebarDepth: 0,
        selectLanguageName: '简体中文',
        selectLanguageText: '选择语言',
        navbar: [
          { text: '指南', link: '/zh/guide/install.html' },
          { text: '性能报告', link: '/zh/#benchmark' },
          { text: '下载', link: 'https://github.com/yisier/rustgrid/releases/latest' },
        ],
        sidebar: {
          '/zh/guide/': [
            {
              text: '开始使用',
              children: [
                '/zh/guide/install.html',
                '/zh/guide/connections.html',
                '/zh/guide/settings.html',
              ],
            },
            {
              text: '功能模块',
              children: [
                '/zh/guide/tables.html',
                '/zh/guide/functions.html',
                '/zh/guide/queries.html',
                '/zh/guide/backup.html',
                '/zh/guide/sync.html',
              ],
            },
            {
              text: '参考',
              children: ['/zh/guide/engines.html', '/zh/guide/shortcuts.html'],
            },
          ],
        },
      },
    },
  }),
})
