import assert from 'node:assert/strict'
import { mkdir, rename, rm } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'

// This fixture is for the disposable docker-compose.screenshots.yml demo.
const baseURL = process.env.CHECKGATE_SCREENSHOT_BASE_URL ?? 'http://127.0.0.1:3100'
const output = (process.env.CHECKGATE_SCREENSHOT_OUTPUT
  ?? fileURLToPath(new URL('../../assets/screenshots/', import.meta.url))).replace(/\/$/, '')
const staging = `${output}.capture`
const workspaceName = 'Vantage Robotics'
const projectName = 'Vantage Mobile'
const password = 'checkgate-screenshot-demo-password'
const admin = { name: 'Juan Dela Cruz', email: 'juan.delacruz@example.com', password }
const editor = { name: 'Maria Santos', email: 'maria.santos@example.com', password }

const flags = [
  { key: 'ai_recommendations', description: 'Personalized product recommendations on the homepage',
    is_enabled: true, rollout_percentage: 100, tags: ['ai', 'mobile'],
    rules: [{ attribute: 'plan', operator: 'equals', values: ['pro', 'enterprise'] }] },
  { key: 'beta_dashboard_v2', description: 'Next-generation fleet dashboard for internal operations',
    is_enabled: false, rollout_percentage: 100, tags: ['beta', 'internal'] },
  { key: 'checkout_provider', description: 'Payment provider for the checkout experience',
    is_enabled: true, rollout_percentage: 100, flag_type: 'string', default_value: 'stripe',
    disabled_value: 'legacy', tags: ['payments'] },
  { key: 'dark_mode', description: 'Dark theme across the mobile app',
    is_enabled: true, rollout_percentage: 60, tags: ['mobile', 'ui'] },
  { key: 'max_upload_size_mb', description: 'Maximum file upload size in megabytes',
    is_enabled: true, rollout_percentage: 100, flag_type: 'integer', default_value: 50,
    disabled_value: 10, tags: ['platform'] },
  { key: 'new_checkout_flow', description: 'Redesigned single-page checkout',
    is_enabled: true, rollout_percentage: 35, tags: ['checkout', 'mobile'],
    rules: [{ attribute: 'region', operator: 'equals', values: ['PH', 'SG'] }],
    prerequisites: [{ flag_key: 'checkout_provider', required_value: 'stripe' }] },
]

const browser = await chromium.launch({
  executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH,
})
try {
  const context = await browser.newContext({
    baseURL, viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1,
    colorScheme: 'light', reducedMotion: 'reduce', locale: 'en-PH', timezoneId: 'Asia/Manila',
    extraHTTPHeaders: { 'X-Checkgate-Request': 'true' },
  })

  async function api(path, method = 'GET', data, request = context.request) {
    // Respect the server's 60-request-per-minute API quota while seeding.
    await new Promise(resolve => setTimeout(resolve, 1050))
    const response = await request.fetch(path, { method, data })
    if (!response.ok()) throw new Error(`${method} ${path}: ${response.status()} ${await response.text()}`)
    return response.status() === 204 ? undefined : response.json()
  }

  const workspace = await api('/api/auth/workspace')
  if (!workspace.is_setup_complete) {
    await api('/api/setup/complete', 'POST', {
      workspace_name: workspaceName, project_name: projectName, ...admin,
    })
  } else {
    assert.equal(workspace.workspace_name, workspaceName, 'Expected the disposable screenshot workspace')
    await api('/api/auth/login', 'POST', admin)
    assert.equal((await api('/api/auth/me')).name, admin.name, 'Expected the Juan Dela Cruz demo account')
  }

  const projects = await api('/api/projects')
  const project = projects.find(p => p.name === projectName)
  assert.ok(project, 'Expected the Vantage Mobile demo project')
  const envPath = `/api/projects/${project.id}/environments`
  let environments = await api(envPath)
  for (const fixture of [
    { name: 'Staging', slug: 'staging', color: '#f59e0b' },
    { name: 'UAT', slug: 'uat', color: '#8b5cf6' },
    { name: 'Development', slug: 'development', color: '#3b82f6' },
  ]) {
    if (!environments.some(e => e.slug === fixture.slug)) await api(envPath, 'POST', fixture)
  }
  environments = await api(envPath)
  const production = environments.find(e => e.slug === 'production')
  const stagingEnv = environments.find(e => e.slug === 'staging')
  assert.equal(environments.find(e => e.slug === 'development')?.color, '#4f46e5',
    'Expected the updated indigo Development environment')
  assert.ok(production && stagingEnv, 'Expected Production and Staging')
  // Disable only this fixture's gate while refreshing demo data; restore it below.
  await api(`${envPath}/${production.id}/require-approval`, 'POST', { require_approval: false })

  for (const env of environments) {
    for (const fixture of flags) {
      const flag = {
        rules: [], flag_type: 'boolean', owner_email: admin.email, ...fixture,
      }
      if (env.slug !== 'production') {
        if (flag.key === 'new_checkout_flow') flag.rollout_percentage = 100
        if (flag.key === 'dark_mode') flag.rollout_percentage = 100
        if (flag.key === 'beta_dashboard_v2') flag.is_enabled = true
      }
      await api(`/api/environments/${env.id}/flags`, 'POST', flag)
    }
  }

  const scheduledPath = `/api/environments/${production.id}/scheduled-changes`
  if (!(await api(scheduledPath)).some(s => !s.executed_at)) {
    await api(`/api/environments/${production.id}/flags/dark_mode/scheduled-changes`, 'POST', {
      scheduled_at: new Date(Date.now() + 7 * 86400000).toISOString(),
      patch: { rollout_percentage: 80 },
    })
  }
  await api(`${envPath}/${production.id}/require-approval`, 'POST', { require_approval: true })

  const users = await api('/api/users')
  const editorUser = users.find(u => u.email === editor.email)
    ?? await api('/api/users', 'POST', { ...editor, role: 'editor' })
  const memberPath = `/api/projects/${project.id}/members`
  if (!(await api(memberPath)).some(m => m.user_id === editorUser.id)) {
    await api(memberPath, 'POST', { user_id: editorUser.id, role: 'editor' })
  }
  const pendingPath = `/api/environments/${production.id}/change-requests?status=pending`
  const pending = await api(pendingPath)
  const editorContext = await browser.newContext({
    baseURL, extraHTTPHeaders: { 'X-Checkgate-Request': 'true' },
  })
  try {
    await api('/api/auth/login', 'POST', editor, editorContext.request)
    for (const [key, patch] of [
      ['new_checkout_flow', { rollout_percentage: 75 }],
      ['beta_dashboard_v2', { is_enabled: true }],
      ['ai_recommendations', { description: 'Expand recommendations to all premium customers' }],
    ]) {
      if (!pending.some(cr => cr.flag_key === key)) {
        await api(`/api/environments/${production.id}/flags/${key}`, 'PATCH', patch, editorContext.request)
      }
    }
  } finally {
    await editorContext.close()
  }

  const tokens = await api('/api/tokens')
  for (const fixture of [
    { name: 'GitHub Actions', scope: 'read_write', expires_in_days: 90 },
    { name: 'Terraform read-only', scope: 'read_only', expires_in_days: 30 },
  ]) {
    if (!tokens.some(t => t.name === fixture.name)) await api('/api/tokens', 'POST', fixture)
  }

  await context.addInitScript(({ projectId, envId }) => {
    localStorage.setItem('lg_active_project', projectId)
    localStorage.setItem('lg_active_env_id', envId)
    localStorage.setItem('lg_sidebar_collapsed', 'false')
    localStorage.removeItem('lg_sidebar_closed_groups')
  }, { projectId: project.id, envId: production.id })

  const page = await context.newPage()
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  page.on('response', response => {
    if (response.url().startsWith(`${baseURL}/api/`) && !response.ok()) {
      errors.push(`HTTP ${response.status()}: ${new URL(response.url()).pathname}`)
    }
  })

  async function visit(path, ready) {
    await page.goto(path, { waitUntil: 'networkidle' })
    await ready()
    await page.evaluate(() => document.fonts.ready)
    await page.locator('aside img[src="/logo.svg"]').waitFor({ state: 'visible' })
    assert.equal(await page.locator('aside img').first().evaluate(img => img.complete && img.naturalWidth > 0), true)
  }

  await rm(staging, { recursive: true, force: true })
  await mkdir(staging, { recursive: true })
  const files = []
  async function capture(file) {
    await page.mouse.move(1439, 999)
    assert.deepEqual(errors, [], 'Fix browser/API errors before publishing screenshots')
    await page.screenshot({ path: `${staging}/${file}`, animations: 'disabled', caret: 'hide' })
    files.push(file)
    console.log(`Captured ${file}`)
  }

  await visit('/', () => page.locator('main tbody tr').first().waitFor())
  assert.equal(await page.locator('aside').getByText(admin.name, { exact: true }).count(), 1)
  const linkColor = await page.locator('main tbody a').first().evaluate(el => getComputedStyle(el).color)
  assert.equal(linkColor, 'rgb(79, 70, 229)', 'Expected the indigo brand color')
  await capture('01-dashboard.png')

  await visit('/flags', () => page.locator('main tbody tr').first().waitFor())
  const toggleColor = await page.getByRole('button', { name: 'Disable flag', exact: true }).first()
    .evaluate(el => getComputedStyle(el).backgroundColor)
  assert.equal(toggleColor, 'rgb(79, 70, 229)', 'Enabled toggles must use indigo')
  await capture('02-feature-flags.png')

  await visit('/flags?edit=new_checkout_flow', async () => {
    await page.getByRole('dialog').waitFor()
    await page.getByRole('dialog').getByPlaceholder('What does this flag control?').waitFor()
  })
  await capture('03-flag-editor.png')

  await visit('/change-requests', () => page.locator('main').getByText('new_checkout_flow', { exact: true }).waitFor())
  await capture('04-change-requests.png')

  await visit('/environments/diff', () => page.locator('main').getByText('new_checkout_flow', { exact: true }).waitFor())
  await capture('05-environment-diff.png')

  await visit('/settings', () => page.locator('main').getByText('GitHub Actions', { exact: true }).waitFor())
  await page.locator('main').getByText('Personal access tokens', { exact: true }).evaluate(el => {
    el.closest('.premium-card').scrollIntoView({ block: 'start' })
  })
  await capture('06-settings-tokens.png')

  await visit('/environments', () => page.locator('main').getByText('UAT', { exact: true }).waitFor())
  await capture('07-environments.png')

  await visit('/flags', () => page.locator('main tbody tr').first().waitFor())
  await page.getByTitle('Collapse sidebar').click()
  await page.waitForFunction(() => document.querySelector('aside').getBoundingClientRect().width === 76)
  await capture('08-sidebar-collapsed.png')

  // Publish only after every page has loaded and every capture has succeeded.
  await mkdir(output, { recursive: true })
  for (const file of files) await rename(`${staging}/${file}`, `${output}/${file}`)
  console.log(`Updated ${files.length} screenshots in ${output}`)
} finally {
  await browser.close()
  await rm(staging, { recursive: true, force: true })
}
