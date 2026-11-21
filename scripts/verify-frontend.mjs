import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { resolve } from 'node:path'
import { webkit } from '../../frontend/node_modules/playwright/index.mjs'
import { compileAppearance, sanitizeAppearance } from '../../frontend/shared/appearance.ts'
import { gifFetchHosts } from '../../frontend/shared/utils/gif-hosts.ts'

const backend = new URL(process.argv[2])
assert.equal(backend.origin, `http://127.0.0.1:${backend.port}`, 'Use a disposable Go backend.')
const assets = resolve(import.meta.dirname, '../src-tauri/frontend-dist')
const { app: { security: { csp } } } = await Bun.file(
  resolve(import.meta.dirname, '../src-tauri/tauri.conf.json'),
).json()
assert(!(await Bun.file(resolve(assets, 'server/index.mjs')).exists()))
const styleNonce = 'ircord-test'
const server = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  async fetch(request) {
    const path = new URL(request.url).pathname
    if (path.startsWith('/api/')) return new Response(null, { status: 404 })
    const file = Bun.file(resolve(assets, `.${path}${path.endsWith('/') ? 'index.html' : ''}`))
    const asset = await file.exists() ? file : Bun.file(resolve(assets, 'index.html'))
    if (!asset.type.startsWith('text/html')) return new Response(asset)
    // Tauri authorizes the styles embedded in the HTML with a nonce.
    const html = (await asset.text()).replace(/<style(?=[\s>])/g, `<style nonce="${styleNonce}"`)
    const scriptHashes = [...html.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/g)]
      .filter(([, code]) => code)
      .map(([, code]) => `'sha256-${createHash('sha256').update(code).digest('base64')}'`)
    const policy = {
      ...csp,
      'script-src': [csp['script-src'], ...scriptHashes].join(' '),
      'style-src': `${csp['style-src']} 'nonce-${styleNonce}'`,
    }
    return new Response(html, {
      headers: {
        'Content-Type': asset.type,
        'Content-Security-Policy': Object.entries(policy).map(([key, value]) => `${key} ${value}`).join('; '),
      },
    })
  },
})

const credentials = { username: `Desk${Date.now().toString(36)}`, password: crypto.randomUUID() }
const signup = await fetch(new URL('/api/v1/auth/signup', backend), {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(credentials),
})
assert.equal(signup.status, 201)
const seedCookie = signup.headers.getSetCookie().find((value) => value.startsWith('ircord_session='))
assert(seedCookie, 'Sign-up created a session to save the account appearance.')
const settings = sanitizeAppearance({
  mode: 'dark', schemeDark: 'hibiki-dark', accent: 'fuchsia', messageFont: 'geist-mono',
})
const compiled = compileAppearance(settings, 'dark')
const saved = await fetch(new URL('/api/v1/app/appearance', backend), {
  method: 'PUT',
  headers: { 'Content-Type': 'application/json', Cookie: seedCookie.split(';')[0] },
  body: JSON.stringify({ settings }),
})
assert.equal(saved.status, 200)
const speech = { baseUrl: 'https://speech.example.test/v1', model: 'test-model', voice: 'test-voice' }
const savedSpeech = await fetch(new URL('/api/v1/app/preferences', backend), {
  method: 'PUT',
  headers: { 'Content-Type': 'application/json', Cookie: seedCookie.split(';')[0] },
  body: JSON.stringify({ preferences: { speech } }),
})
assert.equal(savedSpeech.status, 200)

async function verifyAppearance(page) {
  await page.locator('html[data-scheme="hibiki"]').waitFor({ state: 'attached' })
  const look = await page.evaluate(() => {
    const style = getComputedStyle(document.documentElement)
    return {
      accent: style.getPropertyValue('--accent').trim(),
      background: style.getPropertyValue('--main').trim(),
      messageFont: style.getPropertyValue('--font-message'),
    }
  })
  assert.equal(look.accent, compiled.tokens.accent, 'Use the synced accent under the desktop style policy.')
  assert.equal(look.background, compiled.tokens.main, 'Use the synced desktop palette.')
  assert.match(look.messageFont, /Geist Mono/, 'Use the synced message font.')
}

let browser
try {
  browser = await webkit.launch()
  const bootPage = await browser.newPage()
  try {
    await bootPage.route('**/_nuxt/*.js', (route) => route.abort())
    await bootPage.addInitScript(({ css, attrs }) => {
      localStorage.setItem('ircord.appearance.compiled', JSON.stringify({ mode: 'dark', dark: { css, attrs } }))
    }, { css: compiled.css, attrs: compiled.attrs })
    await bootPage.goto(server.url.href)
    await verifyAppearance(bootPage)
  } finally {
    await bootPage.close()
  }

  for (const titleBar of ['macos', 'windows', 'linux']) {
    const context = await browser.newContext({ serviceWorkers: 'block', reducedMotion: 'reduce', viewport: { width: 1200, height: 780 } })
    const page = await context.newPage()
    page.setDefaultTimeout(12_000)
    const seen = new Map()
    const sockets = new Map()
    const testNotifications = []
    const speechRequests = []
    let updateChecks = 0
    let cookie = ''
    let frames = 0
    let synced = false
    const errors = []
    page.on('pageerror', (error) => errors.push(error.message))
    const events = (id, event) => page.evaluate(({ id, event }) => window.desktopChannels?.get(id)?.(event), { id, event }).catch(() => {})
    await page.exposeFunction('nativeInvoke', async (command, args) => {
      if (command === 'api_request') {
        const { path, method, headers, body, credentials } = args.request
        const url = new URL(path, backend)
        assert.equal(url.origin, backend.origin)
        const response = await fetch(url, {
          method, redirect: 'manual',
          headers: [...headers, ...(credentials && cookie ? [['Cookie', cookie]] : [])],
          ...(body && { body: Buffer.from(body, 'base64') }),
        })
        seen.set(`${method} ${url.pathname}`, response.status)
        for (const value of response.headers.getSetCookie())
          if (value.startsWith('ircord_session=')) cookie = value.split(';')[0]
        return {
          status: response.status,
          headers: [...response.headers].filter(([key]) => key !== 'set-cookie'),
          body: Buffer.from(await response.arrayBuffer()).toString('base64'),
        }
      }
      if (command === 'open_socket') {
        const url = new URL(args.path, backend)
        assert.equal(url.pathname, '/api/v1/events')
        assert.ok(cookie.startsWith('ircord_session='))
        url.protocol = 'ws:'
        const socket = new WebSocket(url, { headers: { Cookie: cookie, Origin: backend.origin } })
        sockets.set(args.id, socket)
        socket.onopen = () => void events(args.events.id, { type: 'open' })
        socket.onmessage = ({ data }) => {
          frames++
          if (JSON.parse(data).type === 'session.synced') synced = true
          void events(args.events.id, { type: 'message', data })
        }
        socket.onclose = ({ code, reason }) => {
          sockets.delete(args.id)
          void events(args.events.id, { type: 'close', code, reason })
        }
        socket.onerror = () => void events(args.events.id, { type: 'error' })
        return
      }
      if (command === 'send_socket') sockets.get(args.id)?.send(args.data)
      if (command === 'close_socket') sockets.get(args.id)?.close()
      if (command === 'notification_permission' || command === 'request_notification_permission')
        return 'granted'
      if (command === 'show_notification') testNotifications.push(args)
      if (command === 'check_for_updates') updateChecks++
      if (command === 'speech_request') {
        speechRequests.push(args.request)
        return { status: 403, audio: '' }
      }
    })
    await context.addInitScript(({ titleBar, serverOrigin }) => {
      window.__IRCordDesktop = {
        titleBar,
        serverOrigin,
        version: '0.1.2',
        canCheckForUpdates: titleBar !== 'linux',
        supportsSpeechService: true,
      }
      window.cspViolations = []
      document.addEventListener('securitypolicyviolation', ({ effectiveDirective, blockedURI }) => {
        window.cspViolations.push(`${effectiveDirective}: ${blockedURI}`)
      })
      window.windowActions = []
      window.desktopChannels = new Map()
      let maximized = false
      window.__TAURI__ = {
        core: {
          invoke: (command, args) => {
            if (command === 'choose_server') {
              window.windowActions.push('choose')
              return Promise.resolve()
            }
            return window.nativeInvoke(command, args)
          },
          Channel: class {
            constructor(onmessage) {
              this.id = crypto.randomUUID()
              window.desktopChannels.set(this.id, onmessage)
            }
          },
        },
        event: { listen: async () => () => {} },
        window: {
          getCurrentWindow: () => ({
            isMaximized: async () => maximized,
            isFullscreen: async () => false,
            startDragging: async () => window.windowActions.push('drag'),
            toggleMaximize: async () => { maximized = !maximized },
            minimize: async () => window.windowActions.push('minimize'),
            close: async () => window.windowActions.push('close'),
            startResizeDragging: async (edge) => window.windowActions.push(edge),
          }),
        },
      }
    }, { titleBar, serverOrigin: backend.origin })
    try {
      let browserApiRequests = 0
      page.on('request', (request) => {
        if (new URL(request.url()).pathname.startsWith('/api/')) browserApiRequests++
      })
      await page.goto(server.url.href)
      if (titleBar === 'macos') {
        for (const host of gifFetchHosts) {
          const url = `${host}/ircord-csp-fixture.gif`
          await page.route(url, (route) => route.fulfill({ body: 'GIF', headers: { 'Access-Control-Allow-Origin': '*' } }))
          assert.equal(await page.evaluate(async (url) => (await fetch(url)).text(), url), 'GIF')
        }
      }
      await page.getByRole('button', { name: 'Change IRCord server' }).click()
      assert.deepEqual(await page.evaluate(() => window.windowActions), ['choose'])
      await page.getByLabel('Username', { exact: true }).fill(credentials.username)
      await page.getByLabel('Password', { exact: true }).fill(credentials.password)
      await page.getByRole('button', { name: 'Sign in', exact: true }).click()
      await page.locator('.workspace').waitFor()
      await verifyAppearance(page)
      const themeHost = page.locator('[data-ircord-theme-layer]')
      await themeHost.locator('.layer').waitFor({ state: 'attached' })
      const themeLayer = await themeHost.evaluate((host) => {
        const bounds = host.getBoundingClientRect()
        const css = getComputedStyle(host)
        const shadowLayer = host.shadowRoot.querySelector('.layer')
        return {
          coversViewport: bounds.width === innerWidth && bounds.height === innerHeight,
          border: css.borderTopWidth,
          pointerEvents: css.pointerEvents,
          shadowPosition: getComputedStyle(shadowLayer).position,
        }
      })
      assert.deepEqual(themeLayer, {
        coversViewport: true,
        border: '0px',
        pointerEvents: 'none',
        shadowPosition: 'fixed',
      }, 'The theme overlay keeps its styles under the desktop style policy.')
      for (let attempt = 0; !synced && attempt < 50; attempt++) await Bun.sleep(100)
      assert(frames > 0, 'The selected Go server sent authenticated events.')
      assert(synced, 'The client resumed the private event stream.')
      for (const endpoint of ['POST /api/v1/auth/login', 'GET /api/v1/app/preferences'])
        assert.equal(seen.get(endpoint), 200, endpoint)
      assert.equal(browserApiRequests, 0)
      await page.reload()
      const bar = page.locator('.desktop-titlebar')
      const toolbar = page.locator('.workspace-bar')
      await toolbar.waitFor()
      const titleBounds = await bar.boundingBox()
      const toolbarBounds = await toolbar.boundingBox()
      assert.equal(titleBounds?.height, 28)
      assert(titleBounds && toolbarBounds && titleBounds.y + titleBounds.height <= toolbarBounds.y)
      await toolbar.click({ position: { x: 500, y: 12 } })
      assert.deepEqual(await page.evaluate(() => window.windowActions), ['drag'])
      const dialog = page.getByRole('dialog', { name: 'Settings' })
      const privacy = async () => {
        await page.getByRole('button', { name: 'User settings' }).click()
        await dialog.locator('[data-section="privacy"]').click()
        return dialog.getByRole('switch', { name: 'Show NSFW content' })
      }
      const toggle = await privacy()
      await toggle.waitFor()
      await toggle.click()
      for (let attempt = 0; !seen.has('PUT /api/v1/app/preferences') && attempt < 50; attempt++) await Bun.sleep(100)
      assert.equal(seen.get('PUT /api/v1/app/preferences'), 200)
      const checked = titleBar !== 'windows'
      await page.reload()
      await toggle.and(dialog.locator(`[aria-checked="${checked}"]`)).waitFor()
      await dialog.locator('[data-section="account"]').click()
      const sessions = dialog.locator('[data-setting="device-sessions"]')
      const os = { macos: 'macOS', windows: 'Windows', linux: 'Linux' }[titleBar]
      await sessions.getByText(`IRCord Desktop on ${os}`, { exact: false }).first().waitFor()
      const status = dialog.locator('[data-setting="notifications-status"]')
      await dialog.locator('[data-section="notifications"]').click()
      assert.equal(await status.count(), 0)
      const speechTest = dialog.locator('[data-setting="speech-test"]')
      await speechTest.getByRole('button', { name: 'Test', exact: true }).click()
      await speechTest.getByRole('status').getByText(/refused the API key/).waitFor()
      assert.equal(speechRequests.length, 1)
      assert.equal(speechRequests[0].baseUrl, speech.baseUrl)
      assert.equal(speechRequests[0].model, speech.model)
      assert.equal(speechRequests[0].voice, speech.voice)
      await dialog.getByRole('combobox', { name: 'Search settings' }).fill('notification setup')
      await dialog.getByRole('option', { name: /Notification setup on this device.*Device/ }).click()
      await status.waitFor()
      assert.equal(await dialog.locator('[data-section="device"]').getAttribute('aria-current'), 'page')
      assert.equal(await sessions.count(), 0)
      await status.getByRole('button', { name: 'Enable notifications' }).click()
      await dialog.getByRole('button', { name: 'Send test' }).click()
      await dialog.getByText(/^Sent\. It should arrive/).waitFor()
      assert.equal(testNotifications.length, 1)
      assert.equal(testNotifications[0].body, 'This is a test notification.')
      if (process.env.IRCORD_DESKTOP_TEST_SHOTS)
        await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, `ircord-${titleBar}-device.png`), animations: 'disabled' })
      await dialog.locator('[data-section="about"]').click()
      await dialog.getByText('Version 0.1.2', { exact: true }).waitFor()
      await dialog.getByText(backend.origin, { exact: true }).waitFor()
      await dialog.getByText('Bundled frontend', { exact: true }).waitFor()
      const updates = dialog.getByRole('button', { name: 'Check for updates', exact: true })
      if (titleBar === 'linux') {
        assert.equal(await updates.count(), 0)
      } else {
        await updates.click()
        assert.equal(updateChecks, 1)
      }
      if (process.env.IRCORD_DESKTOP_TEST_SHOTS)
        await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, `ircord-${titleBar}-about.png`), animations: 'disabled' })
      await page.keyboard.press('Escape')
      await toolbar.click({ position: { x: 500, y: 12 } })
      assert.deepEqual(await page.evaluate(() => window.windowActions), ['drag'])
      if (titleBar !== 'macos') {
        await bar.getByRole('button', { name: 'Maximize window' }).click()
        await bar.getByRole('button', { name: 'Restore window' }).waitFor()
        await bar.getByRole('button', { name: 'Minimize window' }).click()
        await bar.getByRole('button', { name: 'Close window' }).click()
        assert.deepEqual(await page.evaluate(() => window.windowActions), ['drag', 'minimize', 'close'])
      }
      if (process.env.IRCORD_DESKTOP_TEST_SHOTS)
        await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, `ircord-${titleBar}.png`) })
      if (titleBar === 'macos') {
        await page.setViewportSize({ width: 420, height: 720 })
        await page.getByRole('button', { name: 'User settings' }).click()
        await dialog.locator('[data-section="device"]').click()
        const narrow = page.getByRole('dialog', { name: 'Device', exact: true })
        await narrow.getByRole('button', { name: 'Send test' }).waitFor()
        if (process.env.IRCORD_DESKTOP_TEST_SHOTS)
          await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, 'ircord-narrow-device.png'), animations: 'disabled' })
        await narrow.getByRole('button', { name: 'Back to Settings' }).click()
        await dialog.locator('[data-section="device"]').waitFor()
      }
      assert.deepEqual(errors, [])
      assert.deepEqual(await page.evaluate(() => window.cspViolations), [])
      console.log(`${titleBar} shell, device settings, sign-in, saved privacy settings, and private events verified.`)
    } catch (error) {
      if (process.env.IRCORD_DESKTOP_TEST_SHOTS)
        await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, `ircord-${titleBar}-failure.png`) })
      throw error
    } finally {
      for (const socket of sockets.values()) socket.close()
      await context.close()
    }
  }
} finally {
  await browser?.close()
  server.stop(true)
}
