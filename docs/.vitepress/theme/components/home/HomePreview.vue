<script setup lang="ts">
// A real screenshot of the dashboard (assets/screenshots, captured from a
// running server with example data by dashboard/scripts/capture-screenshots.mjs).
// The WebP files are resized copies of 01-dashboard.png (ImageMagick, quality 82),
// so phones do not download the full 1440 px PNG.
import { withBase } from 'vitepress'
import shot1440 from '../../../../../assets/screenshots/web/01-dashboard-1440.webp'
import shot720 from '../../../../../assets/screenshots/web/01-dashboard-720.webp'
</script>

<template>
  <section class="cg-preview" aria-label="Dashboard preview">
    <figure class="cg-frame">
      <div class="cg-frame-bar" aria-hidden="true">
        <span /><span /><span />
        <p>localhost:3000</p>
      </div>
      <img
        :src="shot1440"
        :srcset="`${shot720} 720w, ${shot1440} 1440w`"
        sizes="(max-width: 1120px) calc(100vw - 32px), 1070px"
        width="1440"
        height="1000"
        alt="The Checkgate dashboard overview: flag counts, rollout status by environment and recent activity, with example data"
        decoding="async"
      />
    </figure>
    <p class="cg-preview-caption">
      The v0.1.31 dashboard with example data.
      <a :href="withBase('/guide/dashboard.html')">Tour the dashboard</a>
    </p>
  </section>
</template>

<style scoped>
.cg-preview {
  max-width: 1120px;
  margin: 0 auto;
  padding: 8px 24px 0;
}
.cg-frame {
  margin: 0;
  border: 1px solid var(--cg-border);
  border-radius: var(--cg-radius-lg);
  background: var(--cg-surface);
  box-shadow: var(--cg-shadow-lg);
  overflow: hidden;
}
.cg-frame-bar {
  display: flex;
  align-items: center;
  gap: 7px;
  height: 38px;
  padding: 0 14px;
  border-bottom: 1px solid var(--cg-border);
}
.cg-frame-bar span {
  width: 11px;
  height: 11px;
  border-radius: 50%;
  background: var(--cg-border);
}
.cg-frame-bar p {
  margin: 0 auto;
  padding: 2px 14px;
  border-radius: 7px;
  font-size: 12px;
  color: var(--cg-muted);
  background: var(--cg-bg);
  transform: translateX(-24px);
}
.cg-frame img {
  display: block;
  width: 100%;
  height: auto;
}
.cg-preview-caption {
  margin: 14px 0 0;
  text-align: center;
  font-size: 13px;
  color: var(--cg-muted);
}
.cg-preview-caption a {
  margin-left: 6px;
  font-weight: 600;
  color: var(--cg-accent);
}
@media (max-width: 640px) {
  .cg-preview {
    padding: 0 16px;
  }
  .cg-frame-bar p {
    display: none;
  }
}
</style>
