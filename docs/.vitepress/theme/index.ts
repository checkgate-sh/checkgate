import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import './styles/tokens.css'
import './custom.css'

import HowItWorks from './components/HowItWorks.vue'
import FeatureCard from './components/FeatureCard.vue'
import StackSelector from './components/StackSelector.vue'
import BenchmarkStats from './components/BenchmarkStats.vue'
import SdkMatrix from './components/SdkMatrix.vue'
import Since from './components/Since.vue'
import ReleaseNotice from './components/ReleaseNotice.vue'
import NextSteps from './components/NextSteps.vue'
import HomeHero from './components/home/HomeHero.vue'
import HomePreview from './components/home/HomePreview.vue'
import HomeSection from './components/home/HomeSection.vue'
import HomeDeploy from './components/home/HomeDeploy.vue'
import HomeCta from './components/home/HomeCta.vue'

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    // Available in every Markdown page.
    app.component('HowItWorks', HowItWorks)
    app.component('FlowDiagram', HowItWorks) // older name, kept for existing pages
    app.component('FeatureCard', FeatureCard)
    app.component('StackSelector', StackSelector)
    app.component('BenchmarkStats', BenchmarkStats)
    app.component('SdkMatrix', SdkMatrix)
    app.component('Since', Since)
    app.component('ReleaseNotice', ReleaseNotice)
    app.component('NextSteps', NextSteps)
    app.component('HomeHero', HomeHero)
    app.component('HomePreview', HomePreview)
    app.component('HomeSection', HomeSection)
    app.component('HomeDeploy', HomeDeploy)
    app.component('HomeCta', HomeCta)
  },
} satisfies Theme
