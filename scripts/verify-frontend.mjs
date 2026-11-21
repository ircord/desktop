import assert from 'node:assert/strict'
import { once } from 'node:events'
import { createServer } from 'node:net'
import { resolve } from 'node:path'
import { webkit } from '../../frontend/node_modules/playwright/index.mjs'

const [backend, entry, bun = process.execPath] = process.argv.slice(2)
const upstream = new URL(backend)
assert.equal(upstream.protocol, 'http:')
assert.equal(upstream.hostname, '127.0.0.1', 'Use a disposable loopback backend.')

const listener = createServer().listen(0, '127.0.0.1')
await once(listener, 'listening')
const { port } = listener.address()
await new Promise((resolve) => listener.close(resolve))
const origin = `http://127.0.0.1:${port}`
const key = crypto.randomUUID()
const server = Bun.spawn([resolve(bun), '--no-env-file', resolve(entry)], {
  env: {
    NITRO_HOST: '127.0.0.1',
    NITRO_PORT: String(port),
    NUXT_BACKEND_URL: upstream.origin,
    NUXT_SITE_URL: origin,
    NUXT_PUBLIC_INSTANCE_URL: upstream.origin,
    IRCORD_DESKTOP_KEY: key,
  },
  stdin: 'pipe',
  stderr: 'inherit',
  stdout: 'inherit',
})

let browser
async function ready() {
  for (let attempt = 0; attempt < 80; attempt++) {
    try {
      return await fetch(origin, { signal: AbortSignal.timeout(200) })
    } catch {
      await Bun.sleep(100)
    }
  }
  throw new Error('The bundled frontend failed to start.')
}

try {
  assert.equal((await ready()).status, 403, 'Access requires the native window cookie.')
  const cookie = `ircord_desktop=${key}`
  for (const headers of [
    { cookie, origin: 'http://127.0.0.1:1' },
    { cookie, 'sec-fetch-site': 'same-site' },
  ]) {
    assert.equal((await fetch(origin, { headers })).status, 403)
  }

  browser = await webkit.launch()
  const context = await browser.newContext({ serviceWorkers: 'block' })
  const titleBar = process.env.IRCORD_TEST_TITLE_BAR
  if (titleBar) {
    await context.addInitScript((titleBar) => {
      window.__IRCordDesktop = { titleBar }
      window.windowActions = []
      let maximized = false
      window.__TAURI__ = {
        core: { invoke: async () => 'granted' },
        event: { listen: async () => () => {} },
        window: {
          getCurrentWindow: () => ({
            isMaximized: async () => maximized,
            isFullscreen: async () => false,
            toggleMaximize: async () => { maximized = !maximized },
            minimize: async () => window.windowActions.push('minimize'),
            close: async () => window.windowActions.push('close'),
            startResizeDragging: async (edge) => window.windowActions.push(edge),
          }),
        },
      }
    }, titleBar)
  }
  await context.addCookies([
    { name: 'ircord_desktop', value: key, url: origin, httpOnly: true, sameSite: 'Strict' },
  ])
  // The packaged client must load with the local server as its only network origin.
  await context.route('**/*', (route) => (
    new URL(route.request().url()).origin === origin ? route.continue() : route.abort()
  ))
  const page = await context.newPage()
  assert.equal((await page.goto(origin)).status(), 200)
  await page.getByRole('button', { name: 'Sign in', exact: true }).first().waitFor()

  const credentials = { username: `Desktop${Date.now()}`, password: 'disposable-test-password' }
  async function authenticate(path, body, status) {
    assert.equal(await page.evaluate(async ({ path, body }) => {
      const response = await fetch(`/api/v1/auth/${path}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      return response.status
    }, { path, body }), status)
  }
  await authenticate('signup', credentials, 201)
  await authenticate('logout', undefined, 204)
  await authenticate('login', credentials, 200)
  const session = await page.evaluate(async () => (await fetch('/api/v1/auth/session')).json())
  assert.equal(session.account?.username, credentials.username)
  const sessionCookie = (await context.cookies(origin)).find(({ name }) => name === 'ircord_session')
  assert.equal(sessionCookie?.httpOnly, true)
  assert.equal((await page.request.get(`${origin}/api/app/preferences`)).status(), 200)
  await page.reload()
  await page.locator('.workspace').waitFor()
  if (titleBar) {
    const bar = page.locator('.desktop-titlebar')
    const bounds = await bar.boundingBox()
    const workspace = await page.locator('.workspace-bar').boundingBox()
    assert.ok(bounds && workspace && bounds.y + bounds.height <= workspace.y)
    assert.ok(await bar.getAttribute('data-tauri-drag-region') !== null)
    if (titleBar !== 'macos') {
      await bar.getByRole('button', { name: 'Maximize window' }).click()
      await bar.getByRole('button', { name: 'Restore window' }).waitFor()
      await bar.getByRole('button', { name: 'Restore window' }).click()
      await bar.getByRole('button', { name: 'Maximize window' }).waitFor()
      await bar.getByRole('button', { name: 'Minimize window' }).click()
      await bar.getByRole('button', { name: 'Close window' }).click()
      assert.deepEqual(await page.evaluate(() => window.windowActions), ['minimize', 'close'])
    }
    console.log(`The ${titleBar} layout keeps window controls above the workspace toolbar.`)
  } else assert.equal(await page.locator('.desktop-titlebar').count(), 0)
  const event = await page.evaluate(() => new Promise((resolve, reject) => {
    const socket = new WebSocket('/api/v1/events')
    const timeout = setTimeout(() => {
      socket.close()
      reject(new Error('No live event received.'))
    }, 5000)
    socket.addEventListener('message', (event) => {
      clearTimeout(timeout)
      socket.close()
      resolve(JSON.parse(event.data).type)
    }, { once: true })
    socket.addEventListener('error', () => {
      clearTimeout(timeout)
      reject(new Error('Live connection failed.'))
    }, { once: true })
  }))
  assert.equal(event, 'session.ready')
  server.stdin.end()
  assert.equal(await Promise.race([server.exited, Bun.sleep(3000).then(() => 'timeout')]), 0)
  console.log('Packaged frontend, sign-in, session, preferences, workspace, and WebSocket verified.')
  console.log('Frontend exits when the desktop parent closes its pipe.')
} finally {
  await browser?.close()
  server.kill()
  await server.exited
}
