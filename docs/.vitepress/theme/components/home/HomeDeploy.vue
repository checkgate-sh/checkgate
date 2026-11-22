<script setup lang="ts">
import { withBase } from 'vitepress'
import { Server, Database, RadioTower, KeyRound } from '@lucide/vue'
</script>

<template>
  <div class="cg-deploy">
    <div class="cg-deploy-text">
      <p>
        Checkgate is one Rust server — API, SSE streams and the dashboard — with PostgreSQL and Redis.
        Start with the all-in-one image, which runs all of it in a single container, then move to your own
        PostgreSQL and Redis and run several server instances behind a load balancer.
      </p>
      <p>
        No third-party service sees your flags or users. Browser and mobile SDKs download their environment's
        rules, as any local-evaluation SDK does, so keep secrets out of targeting rules.
      </p>
      <ul class="cg-deploy-links">
        <li><a :href="withBase('/self-hosting.html')">Deploy with Docker</a></li>
        <li><a :href="withBase('/self-hosting.html#production-configuration')">Production checklist</a></li>
        <li><a :href="withBase('/operations/upgrading-to-0-2-0.html')">Upgrading to 0.2.0</a></li>
      </ul>
    </div>
    <ul class="cg-stackcard" aria-label="What you run">
      <li>
        <span class="i" aria-hidden="true"><Server :size="18" /></span>
        <div><b>Checkgate server</b><small>Rust · REST API, SSE, dashboard · <code>ghcr.io/checkgate-sh/checkgate</code></small></div>
        <em>required</em>
      </li>
      <li>
        <span class="i" aria-hidden="true"><Database :size="18" /></span>
        <div><b>PostgreSQL</b><small>flags, revisions, audit log, analytics</small></div>
        <em>required</em>
      </li>
      <li>
        <span class="i" aria-hidden="true"><RadioTower :size="18" /></span>
        <div><b>Redis</b><small>fans changes out to every instance; shared ingest quotas</small></div>
        <em>required</em>
      </li>
      <li>
        <span class="i" aria-hidden="true"><KeyRound :size="18" /></span>
        <div><b><code>SESSION_SECRET</code></b><small>the server refuses to start without it; a setup token guards first-run setup</small></div>
        <em>required</em>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.cg-deploy {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 48px;
  align-items: center;
}
.cg-deploy-text p {
  margin: 0 0 16px;
  font-size: 16px;
  line-height: 1.7;
  color: var(--cg-ink-2);
}
.cg-deploy-links {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 20px;
  margin: 8px 0 0;
  padding: 0;
  list-style: none;
}
.cg-deploy-links a {
  font-weight: 600;
  color: var(--cg-accent);
}
.cg-stackcard {
  margin: 0;
  padding: 8px;
  list-style: none;
  border: 1px solid var(--cg-border);
  border-radius: var(--cg-radius-lg);
  background: var(--cg-bg);
  box-shadow: var(--cg-shadow);
}
.cg-stackcard li {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 14px;
  border-radius: 12px;
}
.cg-stackcard li + li {
  border-top: 1px solid var(--cg-border);
  border-radius: 0;
}
.cg-stackcard .i {
  display: grid;
  flex: none;
  place-items: center;
  width: 36px;
  height: 36px;
  border-radius: 10px;
  color: var(--cg-accent);
  background: var(--cg-accent-soft);
}
.cg-stackcard div {
  flex: 1;
  min-width: 0;
}
.cg-stackcard b {
  display: block;
  font-size: 15px;
  color: var(--cg-ink);
}
.cg-stackcard small {
  display: block;
  margin-top: 2px;
  font-size: 13px;
  line-height: 1.5;
  color: var(--cg-muted);
  overflow-wrap: anywhere;
}
.cg-stackcard em {
  flex: none;
  font-style: normal;
  font-size: 12px;
  font-weight: 600;
  color: var(--cg-muted);
}
@media (max-width: 860px) {
  .cg-deploy {
    grid-template-columns: 1fr;
    gap: 32px;
  }
}
</style>
