<script setup lang="ts">
// A feature card: icon, headline, one or two sentences, optional docs link.
//   <FeatureCard icon="Target" title="…" link="/guide/segments">text</FeatureCard>
import { computed } from 'vue'
import { withBase } from 'vitepress'
import { ArrowRight, Gauge, RadioTower, Target, SlidersHorizontal, Server, ShieldCheck } from '@lucide/vue'

const icons = { Gauge, RadioTower, Target, SlidersHorizontal, Server, ShieldCheck }
const props = defineProps<{ icon: keyof typeof icons; title: string; link?: string; linkText?: string }>()
const Icon = computed(() => icons[props.icon])
const href = computed(() => (props.link ? withBase(props.link.includes('#') || props.link.endsWith('/') ? props.link : `${props.link}.html`) : ''))
</script>

<template>
  <article class="cg-card">
    <span class="cg-card-icon" aria-hidden="true"><component :is="Icon" :size="20" :stroke-width="2" /></span>
    <h3>{{ title }}</h3>
    <p><slot /></p>
    <a v-if="link" :href="href" class="cg-card-link">
      {{ linkText || 'Learn more' }}<span class="sr-only"> about {{ title.toLowerCase() }}</span>
      <ArrowRight :size="14" aria-hidden="true" />
    </a>
  </article>
</template>

<style scoped>
.cg-card {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 24px;
  border: 1px solid var(--cg-border);
  border-radius: var(--cg-radius);
  background: var(--cg-bg);
  transition: border-color 0.2s, box-shadow 0.2s;
}
.cg-card:hover {
  border-color: color-mix(in srgb, var(--cg-accent) 50%, var(--cg-border));
  box-shadow: var(--cg-shadow);
}
.cg-card-icon {
  display: grid;
  place-items: center;
  width: 40px;
  height: 40px;
  margin-bottom: 16px;
  border-radius: 10px;
  color: var(--cg-accent);
  background: var(--cg-accent-soft);
}
h3 {
  margin: 0 0 8px;
  font-size: 17px;
  font-weight: 650;
  letter-spacing: -0.01em;
  color: var(--cg-ink);
}
p {
  flex: 1;
  margin: 0;
  font-size: 14.5px;
  line-height: 1.6;
  color: var(--cg-ink-2);
}
.cg-card-link {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  margin-top: 16px;
  font-size: 14px;
  font-weight: 600;
  color: var(--cg-accent);
  text-decoration: none;
}
.cg-card-link:hover {
  text-decoration: underline;
}
.sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}
</style>
