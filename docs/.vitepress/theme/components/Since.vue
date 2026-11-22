<script setup lang="ts">
// Inline version marker: <Since v="0.2.0" /> renders "0.2.0 · unreleased"
// until that version is published (docs/.vitepress/data/release.ts), then
// "since 0.2.0".
import { computed } from 'vue'
import { isUnreleased } from '../../data/release'

const props = defineProps<{ v: string }>()
const pending = computed(() => isUnreleased(props.v))
</script>

<template>
  <span class="cg-since" :class="{ pending }" :title="pending ? `Not in a published release yet; ships in ${v}` : `Available since ${v}`">
    {{ pending ? `${v} · unreleased` : `since ${v}` }}
  </span>
</template>

<style scoped>
.cg-since {
  display: inline-block;
  margin-left: 6px;
  padding: 0 8px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  line-height: 20px;
  vertical-align: middle;
  white-space: nowrap;
  color: var(--cg-ink-2);
  background: var(--cg-surface-2);
  border: 1px solid var(--cg-border);
}
.cg-since.pending {
  color: var(--cg-indigo-700);
  background: var(--cg-indigo-50);
  border-color: var(--cg-indigo-200);
}
.dark .cg-since.pending {
  color: var(--cg-indigo-200);
  background: rgba(99, 102, 241, 0.16);
  border-color: rgba(129, 140, 248, 0.35);
}
h1 .cg-since,
h2 .cg-since,
h3 .cg-since {
  font-size: 12px;
  letter-spacing: 0;
}
</style>
