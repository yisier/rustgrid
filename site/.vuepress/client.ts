import { defineClientConfig } from 'vuepress/client'
import BenchReport from './components/BenchReport.vue'
import './styles/index.scss'

export default defineClientConfig({
  enhance({ app }) {
    app.component('BenchReport', BenchReport)
  },
})
