import { defineClientConfig } from 'vuepress/client'
import BenchReport from './components/BenchReport.vue'
import './styles/index.scss'

export default defineClientConfig({
  enhance({ app, router }) {
    app.component('BenchReport', BenchReport)

    // Remember the language the visitor is actually on. The inline script in the site head only
    // auto-redirects the English home page, so a manual switch in the navbar is never overridden.
    if (typeof window !== 'undefined') {
      router.afterEach((to) => {
        try {
          const chinese = to.path === '/zh' || to.path.startsWith('/zh/')
          window.localStorage.setItem('rg-locale', chinese ? 'zh' : 'en')
        } catch {
          /* storage unavailable: keep the browser default */
        }
      })
    }
  },
})
