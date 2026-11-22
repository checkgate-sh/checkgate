<script setup lang="ts">
defineProps<{ id: string; eyebrow?: string; title: string; lead?: string; tint?: boolean }>()
</script>

<template>
  <section :id="id" class="cg-section" :class="{ tint }" :aria-labelledby="`${id}-title`">
    <div class="cg-section-inner">
      <header class="cg-section-head">
        <p v-if="eyebrow" class="cg-eyebrow">{{ eyebrow }}</p>
        <h2 :id="`${id}-title`">{{ title }}</h2>
        <p v-if="lead" class="cg-lead">{{ lead }}</p>
      </header>
      <slot />
    </div>
  </section>
</template>

<style scoped>
.cg-section {
  padding: 88px 24px;
}
.cg-section.tint {
  background: var(--cg-surface);
  border-block: 1px solid var(--cg-border);
}
.cg-section-inner {
  max-width: 1120px;
  margin: 0 auto;
}
.cg-section-head {
  max-width: 720px;
  margin: 0 auto 44px;
  text-align: center;
}
.cg-eyebrow {
  margin: 0 0 10px;
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--cg-accent);
}
h2 {
  margin: 0;
  padding: 0;
  border: 0;
  font-size: clamp(28px, 4vw, 40px);
  line-height: 1.15;
  font-weight: 700;
  letter-spacing: -0.025em;
  color: var(--cg-ink);
}
.cg-lead {
  margin: 16px 0 0;
  font-size: 17px;
  line-height: 1.6;
  color: var(--cg-ink-2);
}
@media (max-width: 640px) {
  .cg-section {
    padding: 64px 16px;
  }
  .cg-section-head {
    margin-bottom: 32px;
  }
}
</style>
