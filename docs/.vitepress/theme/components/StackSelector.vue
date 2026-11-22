<script setup lang="ts">
// Code example per SDK, as accessible tabs (WAI-ARIA tabs pattern: arrow keys,
// Home/End, roving tabindex). The code comes from named slots in the page's
// Markdown, so it is highlighted at build time like every other code block:
//   <StackSelector :tabs="[{ id: 'node', label: 'Node.js', pkg: 'node' }]">
//     <template #node> ```js … ``` </template>
//   </StackSelector>
import { ref, computed, nextTick } from 'vue'
import { withBase } from 'vitepress'
import { pkg, statusLabel } from '../../data/release'

const props = defineProps<{ tabs: { id: string; label: string; pkg: string }[] }>()
const active = ref(props.tabs[0].id)
const tabEls = ref<HTMLElement[]>([])
const current = computed(() => pkg(props.tabs.find((t) => t.id === active.value)!.pkg))
const published = computed(() => ['npm', 'pub', 'pypi'].includes(current.value.registry.kind))
const docHref = computed(() => withBase(current.value.doc.endsWith('/') ? current.value.doc : `${current.value.doc}.html`))

function select(i: number, focus = false) {
  const n = props.tabs.length
  const idx = (i + n) % n
  active.value = props.tabs[idx].id
  if (focus) nextTick(() => tabEls.value[idx]?.focus())
}
function onKey(e: KeyboardEvent, i: number) {
  const k = e.key
  if (k === 'ArrowRight' || k === 'ArrowDown') select(i + 1, true)
  else if (k === 'ArrowLeft' || k === 'ArrowUp') select(i - 1, true)
  else if (k === 'Home') select(0, true)
  else if (k === 'End') select(props.tabs.length - 1, true)
  else return
  e.preventDefault()
}
</script>

<template>
  <div class="cg-stack">
    <div class="cg-tabs" role="tablist" aria-label="SDK">
      <button
        v-for="(t, i) in tabs"
        :id="`cg-tab-${t.id}`"
        :key="t.id"
        ref="tabEls"
        type="button"
        role="tab"
        class="cg-tab"
        :aria-selected="active === t.id"
        :aria-controls="`cg-panel-${t.id}`"
        :tabindex="active === t.id ? 0 : -1"
        @click="select(i)"
        @keydown="onKey($event, i)"
      >
        {{ t.label }}
      </button>
    </div>
    <div
      v-for="t in tabs"
      v-show="active === t.id"
      :id="`cg-panel-${t.id}`"
      :key="t.id"
      class="cg-panel vp-doc"
      role="tabpanel"
      :aria-labelledby="`cg-tab-${t.id}`"
      tabindex="0"
    >
      <slot :name="t.id" />
    </div>
    <div class="cg-stack-meta">
      <span class="cg-pkg"><code>{{ current.name }}</code></span>
      <span class="cg-status" :class="published ? 'ok' : 'pending'">{{ statusLabel(current) }}</span>
      <a class="cg-doc-link" :href="docHref">{{ current.platform }} docs →</a>
    </div>
  </div>
</template>

<style scoped>
.cg-stack {
  border: 1px solid var(--cg-border);
  border-radius: var(--cg-radius-lg);
  background: var(--cg-surface);
  box-shadow: var(--cg-shadow);
  overflow: hidden;
}
.cg-tabs {
  display: flex;
  gap: 2px;
  padding: 8px 8px 0;
  overflow-x: auto;
  border-bottom: 1px solid var(--cg-border);
  scrollbar-width: none;
}
.cg-tab {
  flex: none;
  min-height: 40px;
  padding: 0 14px;
  border: 0;
  border-bottom: 2px solid transparent;
  background: none;
  font-size: 14px;
  font-weight: 600;
  color: var(--cg-muted);
  cursor: pointer;
}
.cg-tab:hover {
  color: var(--cg-ink);
}
.cg-tab[aria-selected='true'] {
  color: var(--cg-ink);
  border-bottom-color: var(--cg-accent);
}
.cg-panel {
  padding: 4px 16px 0;
}
.cg-panel :deep(div[class*='language-']) {
  margin: 12px 0 !important;
  border-radius: 10px;
}
.cg-stack-meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  padding: 12px 18px 16px;
  font-size: 13px;
}
.cg-pkg code {
  font-size: 13px;
  color: var(--cg-ink);
}
.cg-status {
  padding: 1px 8px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  border: 1px solid var(--cg-border);
  color: var(--cg-ink-2);
  background: var(--cg-bg);
}
.cg-status.pending {
  color: var(--cg-indigo-700);
  background: var(--cg-indigo-50);
  border-color: var(--cg-indigo-200);
}
.dark .cg-status.pending {
  color: var(--cg-indigo-200);
  background: rgba(99, 102, 241, 0.16);
  border-color: rgba(129, 140, 248, 0.35);
}
.cg-doc-link {
  margin-left: auto;
  font-weight: 600;
  color: var(--cg-accent);
  text-decoration: none;
}
.cg-doc-link:hover {
  text-decoration: underline;
}
</style>
