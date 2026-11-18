# Website

The documentation site lives in `site/` and is published to
<https://yisier.github.io/rustgrid/> by `.github/workflows/pages.yml`.

## Local preview

```bash
cd site
npm install
npm run dev      # http://localhost:8080/rustgrid/
npm run build    # site/.vuepress/dist
```

VuePress 2 (`2.0.0-rc.32`) with the default theme, English at `/` and Chinese at `/zh/`. On a first
visit the site follows the browser language (English by default, Chinese for a `zh-*` browser) and
remembers a manual choice from the language menu in `localStorage`.

## Layout

| Path | Page |
| --- | --- |
| `site/README.md`, `site/zh/README.md` | Home: hero, benchmark report (`BenchReport.vue`) |
| `site/guide/*.md`, `site/zh/guide/*.md` | Tables, Functions, Queries, Backup, Sync, Settings |
| `site/.vuepress/components/BenchReport.vue` | Size + performance report |
| `site/.vuepress/data/bench.json` | Benchmark results consumed by the report |
| `site/.vuepress/data/releases.json` | Release archive sizes + runtime figures |
| `site/guide/assets/{en,zh}/` | Screenshots (kept beside the pages so Vite fingerprints them) |

Screenshots are referenced relatively (`./assets/en/tables.png`); a bare `assets/...` path
is treated as a package specifier by Vite and fails to resolve.

## Refreshing the benchmark

Data sets and the harness live in `tools/rgbench`; see its README. After a run, copy the
generated JSON over `site/.vuepress/data/bench.json` and rebuild.

## Deploying

Repository **Settings → Pages → Build and deployment → Source** must be **GitHub Actions**.
Pushing a change under `site/` (or manual dispatch) builds and deploys the site. The project
path is configured by `base: '/rustgrid/'` in `site/.vuepress/config.ts`.
