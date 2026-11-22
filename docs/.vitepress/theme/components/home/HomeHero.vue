<script setup lang="ts">
import { withBase } from 'vitepress'
import { ArrowRight } from '@lucide/vue'
import { release } from '../../../data/release'
</script>

<template>
  <section class="cg-hero" aria-labelledby="cg-hero-title">
    <div class="cg-hero-bg" aria-hidden="true" />
    <div class="cg-hero-inner">
      <a v-if="release.next" class="cg-hero-news" :href="`${release.repo}/blob/dev/CHANGELOG.md`">
        <span class="cg-hero-news-tag">Coming in {{ release.next }}</span>
        <span class="cg-hero-news-long">Python SDK, OpenFeature, Express · Fastify · NestJS, MCP</span>
        <span class="cg-hero-news-short">What's coming</span>
        <ArrowRight :size="14" aria-hidden="true" />
      </a>
      <p class="cg-hero-eyebrow">Open Source · Self-Hosted · Rust-Powered</p>
      <h1 id="cg-hero-title" class="cg-hero-title">
        Ship with confidence.<br /><span>Roll out with control.</span>
      </h1>
      <p class="cg-hero-lead">
        Open-source, self-hosted feature management with local evaluation and real-time synchronization.
      </p>
      <div class="cg-hero-actions">
        <a class="cg-btn brand" :href="withBase('/guide/quickstart.html')">
          Get Started <ArrowRight :size="16" aria-hidden="true" />
        </a>
        <a class="cg-btn alt" :href="release.repo">
          <span class="vpi-social-github cg-gh" aria-hidden="true" /> View on GitHub
        </a>
      </div>
      <p class="cg-hero-tech">
        Feature flags without the round-trip: <code>isEnabled()</code> is answered inside your process.
      </p>
    </div>
  </section>
</template>

<style scoped>
.cg-hero {
  position: relative;
  padding: 72px 24px 40px;
  text-align: center;
  overflow: hidden;
}
.cg-hero-bg {
  position: absolute;
  inset: -1px 0 0;
  background-image:
    linear-gradient(var(--cg-grid) 1px, transparent 1px),
    linear-gradient(90deg, var(--cg-grid) 1px, transparent 1px);
  background-size: 48px 48px;
  background-position: center top;
  -webkit-mask-image: radial-gradient(ellipse 70% 65% at 50% 30%, #000 30%, transparent 75%);
  mask-image: radial-gradient(ellipse 70% 65% at 50% 30%, #000 30%, transparent 75%);
}
.cg-hero-bg::after {
  content: '';
  position: absolute;
  left: 50%;
  top: -180px;
  width: 900px;
  height: 560px;
  transform: translateX(-50%);
  background: radial-gradient(closest-side, rgba(99, 102, 241, 0.16), transparent);
}
.dark .cg-hero-bg::after {
  background: radial-gradient(closest-side, rgba(99, 102, 241, 0.26), transparent);
}
.cg-hero-inner {
  position: relative;
  max-width: 880px;
  margin: 0 auto;
}
.cg-hero-news {
  display: inline-flex;
  align-items: center;
  flex-wrap: wrap;
  justify-content: center;
  gap: 8px;
  margin-bottom: 28px;
  padding: 5px 12px 5px 5px;
  border: 1px solid var(--cg-border);
  border-radius: 999px;
  background: color-mix(in srgb, var(--cg-surface) 85%, transparent);
  font-size: 13px;
  font-weight: 500;
  color: var(--cg-ink-2);
  text-decoration: none;
  transition: border-color 0.2s;
}
.cg-hero-news:hover {
  border-color: var(--cg-accent);
}
.cg-hero-news-tag {
  padding: 2px 9px;
  border-radius: 999px;
  font-weight: 600;
  color: #fff;
  background: var(--cg-indigo-600);
}
.cg-hero-eyebrow {
  margin: 0 0 18px;
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--cg-accent);
}
.cg-hero-title {
  margin: 0;
  font-size: clamp(32px, 6.4vw, 68px);
  line-height: 1.05;
  font-weight: 750;
  letter-spacing: -0.035em;
  color: var(--cg-ink);
}
.cg-hero-title span {
  color: var(--cg-accent);
}
.cg-hero-lead {
  max-width: 640px;
  margin: 24px auto 0;
  font-size: clamp(17px, 2.1vw, 20px);
  line-height: 1.55;
  color: var(--cg-ink-2);
}
.cg-hero-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 12px;
  margin-top: 32px;
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
  transition: background-color 0.2s, border-color 0.2s, transform 0.2s;
}
.cg-btn.brand {
  color: #fff;
  background: var(--cg-indigo-600);
  box-shadow: 0 8px 24px -10px rgba(79, 70, 229, 0.7);
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
.cg-gh {
  display: inline-block;
  width: 18px;
  height: 18px;
}
.cg-hero-news-short {
  display: none;
}
.cg-hero-tech {
  margin: 28px 0 0;
  font-size: 14px;
  color: var(--cg-muted);
}
.cg-hero-tech code {
  padding: 2px 6px;
  border-radius: 6px;
  font-size: 13px;
  color: var(--cg-eval);
  background: var(--cg-accent-soft);
}
@media (max-width: 640px) {
  .cg-hero {
    padding: 48px 16px 28px;
  }
  .cg-hero-title {
    font-size: clamp(32px, 8.4vw, 44px);
  }
  .cg-hero-news-long {
    display: none;
  }
  .cg-hero-news-short {
    display: inline;
  }
  .cg-btn {
    flex: 1 1 180px;
    justify-content: center;
  }
}
</style>
