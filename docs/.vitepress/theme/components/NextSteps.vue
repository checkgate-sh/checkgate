<script setup lang="ts">
// Next-step links at the end of a page:
//   <NextSteps :items="[{ title: 'Install an SDK', link: '/sdks/', text: '…' }]" />
import { withBase } from 'vitepress'
import { ArrowRight } from '@lucide/vue'

defineProps<{ items: { title: string; link: string; text?: string }[] }>()
const href = (l: string) => (/^https?:/.test(l) ? l : withBase(l.endsWith('/') || l.includes('#') || l.endsWith('.html') ? l : `${l}.html`))
</script>

<template>
  <nav class="cg-next" aria-label="Next steps">
    <a v-for="i in items" :key="i.link" class="cg-next-item" :href="href(i.link)">
      <span class="cg-next-title">{{ i.title }} <ArrowRight :size="16" aria-hidden="true" /></span>
      <span v-if="i.text" class="cg-next-text">{{ i.text }}</span>
    </a>
  </nav>
</template>

<style scoped>
.cg-next {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: 12px;
  margin: 24px 0;
}
.cg-next-item {
  display: block;
  padding: 16px 18px;
  border: 1px solid var(--cg-border);
  border-radius: 12px;
  background: var(--cg-surface);
  text-decoration: none !important;
  transition: border-color 0.2s, transform 0.2s;
}
.cg-next-item:hover {
  border-color: var(--cg-accent);
  transform: translateY(-1px);
}
.cg-next-title {
  display: flex;
  align-items: center;
  gap: 6px;
  font-weight: 600;
  color: var(--cg-accent);
}
.cg-next-text {
  display: block;
  margin-top: 4px;
  font-size: 14px;
  color: var(--cg-ink-2);
}
</style>
