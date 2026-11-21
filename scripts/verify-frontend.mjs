import assert from 'node:assert/strict'
import { resolve } from 'node:path'
import { webkit } from '../../frontend/node_modules/playwright/index.mjs'

const backend = new URL(process.argv[2])
assert.equal(backend.origin, `http://127.0.0.1:${backend.port}`, 'Use a disposable Go backend.')
const assets = resolve(import.meta.dirname, '../src-tauri/frontend-dist')
assert(!(await Bun.file(resolve(assets, 'server/index.mjs')).exists()))
const server = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  async fetch(request) {
    const path = new URL(request.url).pathname
    if (path.startsWith('/api/')) return new Response(null, { status: 404 })
    const file = Bun.file(resolve(assets, `.${path}${path.endsWith('/') ? 'index.html' : ''}`))
    if (await file.exists()) return new Response(file)
    return new Response(Bun.file(resolve(assets, 'index.html')))
  },
})

const credentials = { username: `Desk${Date.now().toString(36)}`, password: crypto.randomUUID() }
const signup = await fetch(new URL('/api/v1/auth/signup', backend), {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify(credentials),
})
assert.equal(signup.status, 201)
let browser
try {
  browser = await webkit.launch()
  for (const titleBar of ['macos', 'windows', 'linux']) {
    const context = await browser.newContext({ serviceWorkers: 'block', viewport: { width: 1200, height: 780 } })
    const page = await context.newPage()
    page.setDefaultTimeout(12_000)
    const seen = new Map()
    const sockets = new Map()
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
      if (command === 'notification_permission') return 'granted'
    })
    await context.addInitScript(({ titleBar, serverOrigin }) => {
      window.__IRCordDesktop = { titleBar, serverOrigin }
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
      await page.getByRole('button', { name: 'Change IRCord server' }).click()
      assert.deepEqual(await page.evaluate(() => window.windowActions), ['choose'])
      await page.getByLabel('Username', { exact: true }).fill(credentials.username)
      await page.getByLabel('Password', { exact: true }).fill(credentials.password)
      await page.getByRole('button', { name: 'Sign in', exact: true }).click()
      await page.locator('.workspace').waitFor()
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
      assert.deepEqual(errors, [])
      console.log(`${titleBar} shell, server chooser, sign-in, saved privacy settings, and private events verified.`)
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
