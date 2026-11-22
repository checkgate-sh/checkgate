<script setup lang="ts">
// Distribution status of one package, from docs/.vitepress/data/release.ts:
//   <ReleaseNotice pkg="python" />
// Renders nothing for a package that is on its registry and released.
import { computed } from 'vue'
import { pkg, isUnreleased, release } from '../../data/release'

const props = defineProps<{ pkg: string }>()
const p = computed(() => pkg(props.pkg))
const show = computed(() => p.value.registry.kind === 'unpublished' || p.value.registry.kind === 'source' || isUnreleased(p.value.since))
const note = computed(() => ('note' in p.value.registry ? p.value.registry.note : ''))
</script>

<template>
  <div v-if="show" class="custom-block warning cg-release-notice" role="note">
    <p class="custom-block-title">
      <template v-if="isUnreleased(p.since)">Coming in {{ p.since }} — not released yet</template>
      <template v-else>How to get it</template>
    </p>
    <p>
      <code>{{ p.name }}</code>
      <template v-if="isUnreleased(p.since)">
        is in the {{ p.since }} source tree; the latest release is {{ release.latest }}.
      </template>
      {{ note }}
    </p>
  </div>
</template>
