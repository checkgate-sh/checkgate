<script setup lang="ts">
import { withBase } from 'vitepress'
import { ArrowRight } from '@lucide/vue'
</script>

<template>
  <section class="cg-cta" aria-labelledby="cg-cta-title">
    <div class="cg-cta-inner">
      <h2 id="cg-cta-title">Start controlling your releases with Checkgate.</h2>
      <p>Run it locally in about five minutes with Docker and Node.js, then deploy it on your own infrastructure.</p>
      <div class="cg-cta-actions">
        <a class="cg-btn brand" :href="withBase('/guide/quickstart.html')">Five-minute quickstart <ArrowRight :size="16" aria-hidden="true" /></a>
        <a class="cg-btn alt" :href="withBase('/guide/what-is-checkgate.html')">Read the docs</a>
      </div>
    </div>
  </section>
</template>

<style scoped>
.cg-cta {
  padding: 24px 24px 64px;
}
.cg-cta-inner {
  position: relative;
  max-width: 1120px;
  margin: 0 auto;
  padding: 64px 32px;
  border: 1px solid var(--cg-border);
  border-radius: 24px;
  text-align: center;
  overflow: hidden;
  background:
    radial-gradient(ellipse 60% 80% at 50% 0%, rgba(99, 102, 241, 0.16), transparent 70%),
    var(--cg-surface);
}
h2 {
  margin: 0 auto;
  max-width: 640px;
  padding: 0;
  border: 0;
  font-size: clamp(26px, 3.6vw, 36px);
  line-height: 1.2;
  font-weight: 700;
  letter-spacing: -0.025em;
  color: var(--cg-ink);
}
p {
  margin: 16px auto 0;
  max-width: 560px;
  font-size: 16px;
  line-height: 1.6;
  color: var(--cg-ink-2);
}
.cg-cta-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 12px;
  margin-top: 28px;
}
.cg-btn {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-height: 44px;
  padding: 0 20px;
  border-radius: 12px;
  font-size: 15px;
  font-weight: 600;
  text-decoration: none;
}
.cg-btn.brand {
  color: #fff;
  background: var(--cg-indigo-600);
}
.cg-btn.brand:hover {
  background: var(--cg-indigo-700);
}
.cg-btn.alt {
  color: var(--cg-ink);
  background: var(--cg-bg);
  border: 1px solid var(--cg-border);
}
.cg-btn.alt:hover {
  border-color: var(--cg-accent);
}
@media (max-width: 640px) {
  .cg-cta {
    padding: 8px 16px 64px;
  }
  .cg-cta-inner {
    padding: 44px 20px;
  }
}
</style>
