<script setup lang="ts">
// SDK and integration table with real distribution status, from
// docs/.vitepress/data/release.ts. <SdkMatrix group="sdk" /> or all groups.
import { computed } from 'vue'
import { withBase } from 'vitepress'
import { packages, statusLabel, type Package } from '../../data/release'

const props = defineProps<{ group?: Package['group'] }>()
const rows = computed(() => packages.filter((p) => !props.group || p.group === props.group))
const published = (p: Package) => ['npm', 'pub', 'pypi'].includes(p.registry.kind)
const notes = (p: Package) => [('note' in p.registry ? p.registry.note : ''), p.limits].filter(Boolean).join(' ')
</script>

<template>
  <div class="cg-matrix">
    <table>
      <thead>
        <tr>
          <th scope="col">For</th>
          <th scope="col">Package</th>
          <th scope="col">Status</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="p in rows" :key="p.id">
          <th scope="row"><a :href="withBase(p.doc + (p.doc.endsWith('/') ? '' : '.html'))">{{ p.platform }}</a></th>
          <td>
            <code>{{ p.name }}</code><span class="cg-binding">{{ p.binding }}</span>
            <span v-if="notes(p)" class="cg-notes">{{ notes(p) }}</span>
          </td>
          <td><span class="cg-status" :class="published(p) ? 'ok' : 'pending'">{{ statusLabel(p) }}</span></td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
.cg-matrix {
  margin: 16px 0;
  overflow-x: auto;
  border: 1px solid var(--cg-border);
  border-radius: 12px;
}
table {
  display: table !important;
  width: 100%;
  margin: 0 !important;
  border-collapse: collapse;
  font-size: 14px;
}
th,
td {
  border: 0 !important;
  border-bottom: 1px solid var(--cg-border) !important;
  padding: 10px 14px !important;
  text-align: left;
  vertical-align: top;
}
tr:last-child > * {
  border-bottom: 0 !important;
}
tr,
tr:nth-child(2n) {
  background: transparent !important;
}
thead th {
  background: var(--cg-surface);
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--cg-muted);
}
tbody th {
  font-weight: 600;
  min-width: 120px;
  background: transparent !important;
}
code {
  overflow-wrap: anywhere;
}
td:last-child {
  white-space: nowrap;
}
.cg-binding {
  display: block;
  margin-top: 2px;
  font-size: 12px;
  color: var(--cg-muted);
}
.cg-status {
  display: inline-block;
  padding: 1px 8px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
  border: 1px solid var(--cg-border);
  color: var(--cg-ink-2);
  background: var(--cg-surface);
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
.cg-notes {
  display: block;
  margin-top: 6px;
  max-width: 52ch;
  color: var(--cg-ink-2);
  font-size: 13px;
  line-height: 1.5;
}
@media (max-width: 640px) {
  th,
  td {
    padding: 10px !important;
  }
  tbody th {
    min-width: 0;
  }
  td:last-child,
  .cg-status {
    white-space: normal;
  }
}
</style>
