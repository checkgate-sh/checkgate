<script setup>
// Clients shown fanning out from the server. `cy` is the vertical center of
// each client node in the 820x380 SVG viewBox.
const clients = [
  { label: 'Node.js', sub: 'NAPI', cy: 60 },
  { label: 'Web', sub: 'WASM', cy: 140 },
  { label: 'React Native', sub: 'JSI', cy: 220 },
  { label: 'Flutter', sub: 'FFI', cy: 300 },
]
const pushPath = (cy) => `M500,190 Q590,${cy} 654,${cy}`
</script>

<template>
  <figure class="cg-flow">
    <svg viewBox="0 0 820 380" role="img"
         aria-label="A flag change flows from the dashboard to the Checkgate server, which pushes it over SSE to every SDK, where flags evaluate locally in-process.">
      <!-- connection paths -->
      <path class="cg-wire" d="M172,190 L300,190" />
      <path v-for="c in clients" :key="'w' + c.label" class="cg-wire" :d="pushPath(c.cy)" />

      <!-- Dashboard -->
      <g class="cg-node">
        <rect x="20" y="150" width="152" height="80" rx="14" />
        <text x="96" y="184" class="cg-title">Dashboard / API</text>
        <text x="96" y="205" class="cg-sub">toggle a flag</text>
      </g>

      <!-- Server -->
      <g class="cg-node cg-server">
        <rect x="300" y="104" width="200" height="172" rx="16" />
        <text x="400" y="130" class="cg-title">Checkgate Server</text>
        <rect x="318" y="146" width="164" height="30" rx="8" class="cg-pill cg-pill-accent" />
        <text x="400" y="166" class="cg-pill-text">per-environment SSE</text>
        <rect x="318" y="186" width="78" height="28" rx="8" class="cg-pill" />
        <text x="357" y="205" class="cg-pill-text">PostgreSQL</text>
        <rect x="404" y="186" width="78" height="28" rx="8" class="cg-pill" />
        <text x="443" y="205" class="cg-pill-text">Redis</text>
        <text x="400" y="248" class="cg-sub">pushes deltas over SSE</text>
      </g>

      <!-- SSE label -->
      <text x="578" y="24" class="cg-edge-label">SSE · &lt;50 ms</text>

      <!-- Clients -->
      <g v-for="c in clients" :key="c.label" class="cg-node cg-client">
        <rect :x="654" :y="c.cy - 27" width="146" height="54" rx="12" />
        <rect class="cg-flash" :x="654" :y="c.cy - 27" width="146" height="54" rx="12" />
        <text x="727" :y="c.cy - 3" class="cg-title">{{ c.label }}</text>
        <text x="727" :y="c.cy + 15" class="cg-sub">{{ c.sub }} · local eval</text>
      </g>

      <!-- Right-anchored: centred on the client column this label runs past the
           820-wide viewBox and gets clipped. -->
      <text x="800" y="352" class="cg-edge-label cg-edge-end">isEnabled() · ~100 ns, in-process</text>

      <!-- Animated packets (SMIL). All share dur=3.2s so the write→push cycle stays in sync. -->
      <circle class="cg-packet cg-write" r="5">
        <animateMotion dur="3.2s" repeatCount="indefinite"
          keyTimes="0;0.35;1" keyPoints="0;1;1" calcMode="linear"
          path="M172,190 L300,190" />
        <animate attributeName="opacity" dur="3.2s" repeatCount="indefinite"
          keyTimes="0;0.02;0.33;0.36;1" values="0;1;1;0;0" />
      </circle>

      <template v-for="c in clients" :key="'p' + c.label">
        <circle class="cg-packet cg-push" r="5">
          <animateMotion dur="3.2s" repeatCount="indefinite"
            keyTimes="0;0.4;0.75;1" keyPoints="0;0;1;1" calcMode="linear"
            :path="pushPath(c.cy)" />
          <animate attributeName="opacity" dur="3.2s" repeatCount="indefinite"
            keyTimes="0;0.4;0.42;0.73;0.76;1" values="0;0;1;1;0;0" />
        </circle>
      </template>
    </svg>
    <figcaption>A flag change is written once, then pushed to every connected SDK in milliseconds — after which each <code>isEnabled()</code> is a local, in-process lookup.</figcaption>
  </figure>
</template>

<style scoped>
.cg-flow {
  margin: 1.5rem 0;
}
.cg-flow svg {
  width: 100%;
  height: auto;
  max-width: 820px;
  display: block;
  margin: 0 auto;
}
.cg-wire {
  fill: none;
  stroke: var(--vp-c-divider);
  stroke-width: 2;
  stroke-dasharray: 5 6;
}
.cg-node rect {
  fill: var(--vp-c-bg-soft);
  stroke: var(--vp-c-divider);
  stroke-width: 1.5;
}
.cg-server > rect {
  stroke: var(--vp-c-brand-1);
}
.cg-pill {
  fill: var(--vp-c-bg);
  stroke: var(--vp-c-divider);
  stroke-width: 1;
}
.cg-pill-accent {
  fill: color-mix(in srgb, var(--vp-c-brand-1) 14%, var(--vp-c-bg));
  stroke: color-mix(in srgb, var(--vp-c-brand-1) 40%, transparent);
}
.cg-title {
  fill: var(--vp-c-text-1);
  font-size: 14px;
  font-weight: 700;
  text-anchor: middle;
  font-family: var(--vp-font-family-base);
}
.cg-sub {
  fill: var(--vp-c-text-2);
  font-size: 11px;
  text-anchor: middle;
  font-family: var(--vp-font-family-base);
}
.cg-pill-text {
  fill: var(--vp-c-text-1);
  font-size: 11px;
  font-weight: 600;
  text-anchor: middle;
  font-family: var(--vp-font-family-base);
}
.cg-edge-label {
  fill: var(--vp-c-text-3);
  font-size: 11px;
  font-weight: 600;
  text-anchor: middle;
  font-family: var(--vp-font-family-mono, monospace);
}
.cg-edge-end {
  text-anchor: end;
}
.cg-packet {
  fill: var(--vp-c-brand-1);
}
.cg-write {
  fill: #3b82f6;
}
.cg-push {
  fill: #10b981;
}
/* Each client briefly "lights up" as the pushed delta arrives (~0.72 of cycle). */
.cg-flash {
  fill: #10b981;
  opacity: 0;
  animation: cg-arrive 3.2s linear infinite;
}
@keyframes cg-arrive {
  0%, 70% { opacity: 0; }
  74% { opacity: 0.22; }
  82%, 100% { opacity: 0; }
}
figcaption {
  text-align: center;
  color: var(--vp-c-text-2);
  font-size: 0.85rem;
  margin-top: 0.75rem;
}
/* Respect users who prefer reduced motion: keep the static diagram, drop the movement. */
@media (prefers-reduced-motion: reduce) {
  .cg-packet { display: none; }
  .cg-flash { animation: none; }
}
</style>
