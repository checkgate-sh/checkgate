<script setup lang="ts">
// How Checkgate works: the sync path (dashboard → server → SDK, when a flag
// changes) drawn apart from the evaluation path (your code → SDK, every flag
// check, in-process). HTML rather than one SVG so it reflows on small screens;
// the moving dot is decorative and stops under prefers-reduced-motion.
import { LayoutDashboard, Server, Database, RadioTower, Cpu, Code2 } from '@lucide/vue'
</script>

<template>
  <figure class="cg-how" aria-labelledby="cg-how-caption">
    <div class="cg-how-row">
      <div class="cg-node">
        <span class="cg-step sync">1</span>
        <p class="cg-node-title"><LayoutDashboard :size="18" aria-hidden="true" /> Dashboard / API</p>
        <p class="cg-node-text">A flag, rule or rollout changes — from the dashboard, the REST API, Terraform or an approved change request.</p>
      </div>

      <div class="cg-wire sync" aria-hidden="true"><span class="cg-dot" /></div>

      <div class="cg-node cg-server">
        <span class="cg-step sync">2</span>
        <p class="cg-node-title"><Server :size="18" aria-hidden="true" /> Checkgate server</p>
        <ul class="cg-parts">
          <li><Database :size="14" aria-hidden="true" /> <b>PostgreSQL</b> stores flags and revisions</li>
          <li><RadioTower :size="14" aria-hidden="true" /> <b>Redis</b> fans the change out to every instance</li>
        </ul>
      </div>

      <div class="cg-wire sync" aria-hidden="true"><span class="cg-dot late" /></div>

      <div class="cg-process">
        <p class="cg-process-label">Your service — one process</p>
        <div class="cg-node cg-sdk">
          <span class="cg-step sync">3</span>
          <p class="cg-node-title"><Cpu :size="18" aria-hidden="true" /> Checkgate SDK</p>
          <p class="cg-node-text">Receives the change over SSE and updates its local flag store. Keeps the last known flags if the stream drops; polls only if a stream cannot be established.</p>
        </div>
        <div class="cg-eval" aria-hidden="true"><span /><span /></div>
        <div class="cg-node cg-code">
          <span class="cg-step eval">4</span>
          <p class="cg-node-title"><Code2 :size="18" aria-hidden="true" /> Your code</p>
          <p class="cg-mono">isEnabled('new-checkout', user)</p>
          <p class="cg-node-text">Answered by the SDK's Rust engine from memory: no network request per check.</p>
        </div>
      </div>
    </div>
    <figcaption id="cg-how-caption" class="cg-legend">
      <span><i class="sync" aria-hidden="true" /> Sync path — runs when configuration changes</span>
      <span><i class="eval" aria-hidden="true" /> Evaluation path — every flag check, inside your process</span>
    </figcaption>
  </figure>
</template>

<style scoped>
.cg-how {
  margin: 0;
}
.cg-how-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 56px minmax(0, 1.1fr) 56px minmax(0, 1.35fr);
  align-items: center;
}
.cg-node {
  position: relative;
  padding: 20px 20px 18px;
  border: 1px solid var(--cg-border);
  border-radius: var(--cg-radius);
  background: var(--cg-surface);
  box-shadow: var(--cg-shadow);
}
.cg-server {
  border-color: color-mix(in srgb, var(--cg-accent) 55%, var(--cg-border));
}
.cg-sdk {
  border-color: color-mix(in srgb, var(--cg-eval) 55%, var(--cg-border));
}
.cg-step {
  position: absolute;
  top: -11px;
  left: 16px;
  display: grid;
  place-items: center;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  font-size: 12px;
  font-weight: 700;
  color: #fff;
}
.cg-step.sync {
  background: var(--cg-indigo-600);
}
.cg-step.eval {
  background: var(--cg-violet-600);
}
.cg-node-title {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0 0 6px;
  font-size: 15px;
  font-weight: 700;
  color: var(--cg-ink);
}
.cg-node-title :deep(svg) {
  color: var(--cg-accent);
  flex: none;
}
.cg-node-text {
  margin: 0;
  font-size: 13.5px;
  line-height: 1.55;
  color: var(--cg-ink-2);
}
.cg-parts {
  margin: 8px 0 0;
  padding: 0;
  list-style: none;
  font-size: 13.5px;
  line-height: 1.5;
  color: var(--cg-ink-2);
}
.cg-parts li {
  display: flex;
  align-items: baseline;
  gap: 6px;
  margin: 4px 0;
}
.cg-parts li :deep(svg) {
  flex: none;
  position: relative;
  top: 2px;
  color: var(--cg-muted);
}
.cg-parts b {
  color: var(--cg-ink);
}
.cg-process {
  position: relative;
  padding: 44px 16px 16px;
  border: 1.5px dashed var(--cg-border);
  border-radius: var(--cg-radius-lg);
}
.cg-process-label {
  position: absolute;
  top: 10px;
  left: 18px;
  margin: 0;
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
  color: var(--cg-muted);
}
.cg-mono {
  margin: 2px 0 6px;
  font-family: var(--vp-font-family-mono);
  font-size: 13px;
  font-weight: 600;
  color: var(--cg-eval);
  overflow-wrap: anywhere;
}
.cg-eval {
  display: flex;
  justify-content: center;
  gap: 22px;
  height: 34px;
}
.cg-eval span {
  width: 2px;
  background: var(--cg-eval);
  position: relative;
}
.cg-eval span::after {
  content: '';
  position: absolute;
  left: -4px;
  border: 5px solid transparent;
}
.cg-eval span:first-child::after {
  top: -2px;
  border-bottom: 7px solid var(--cg-eval);
  border-top: 0;
}
.cg-eval span:last-child::after {
  bottom: -2px;
  border-top: 7px solid var(--cg-eval);
  border-bottom: 0;
}
.cg-wire {
  position: relative;
  height: 2px;
  margin: 0 6px;
  background: var(--cg-accent);
  opacity: 0.85;
}
.cg-wire::after {
  content: '';
  position: absolute;
  right: -2px;
  top: -5px;
  border-left: 8px solid var(--cg-accent);
  border-top: 6px solid transparent;
  border-bottom: 6px solid transparent;
}
.cg-dot {
  position: absolute;
  top: -4px;
  left: 0;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--cg-accent);
  box-shadow: 0 0 0 4px var(--cg-accent-soft);
  animation: cg-travel 3.2s ease-in-out infinite;
}
.cg-dot.late {
  animation-delay: 0.9s;
}
@keyframes cg-travel {
  0% { left: 0; opacity: 0; }
  10% { opacity: 1; }
  45% { left: calc(100% - 10px); opacity: 1; }
  55%, 100% { left: calc(100% - 10px); opacity: 0; }
}
.cg-legend {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 8px 28px;
  margin-top: 24px;
  font-size: 13.5px;
  font-weight: 600;
  color: var(--cg-ink-2);
}
.cg-legend span {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}
.cg-legend i {
  display: inline-block;
  width: 24px;
  height: 3px;
  border-radius: 2px;
}
.cg-legend i.sync {
  background: var(--cg-accent);
}
.cg-legend i.eval {
  background: var(--cg-eval);
}

@media (max-width: 900px) {
  .cg-how-row {
    grid-template-columns: 1fr;
  }
  .cg-wire {
    width: 2px;
    height: 36px;
    margin: 4px auto;
  }
  .cg-wire::after {
    right: -5px;
    top: auto;
    bottom: -2px;
    border-left: 6px solid transparent;
    border-right: 6px solid transparent;
    border-top: 8px solid var(--cg-accent);
    border-bottom: 0;
  }
  .cg-dot {
    left: -4px;
    animation-name: cg-travel-y;
  }
  @keyframes cg-travel-y {
    0% { top: 0; opacity: 0; }
    10% { opacity: 1; }
    45% { top: calc(100% - 10px); opacity: 1; }
    55%, 100% { top: calc(100% - 10px); opacity: 0; }
  }
}
@media (prefers-reduced-motion: reduce) {
  .cg-dot {
    display: none;
  }
}
</style>
