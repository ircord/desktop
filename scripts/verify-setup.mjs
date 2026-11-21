import assert from 'node:assert/strict'
import { resolve } from 'node:path'
import { webkit } from '../../frontend/node_modules/playwright/index.mjs'

const assets = resolve(import.meta.dirname, '../src-tauri/frontend-dist')
const server = Bun.serve({
  hostname: '127.0.0.1',
  port: 0,
  async fetch(request) {
    const path = new URL(request.url).pathname
    const file = Bun.file(resolve(assets, `.${path}`))
    return await file.exists() ? new Response(file) : new Response(null, { status: 404 })
  },
})
const browser = await webkit.launch()
try {
  const page = await browser.newPage({ viewport: { width: 820, height: 600 }, colorScheme: 'dark' })
  await page.addInitScript(() => {
    window.__TAURI__ = {
      core: {
        invoke: async (command, { address }) => {
          window.connection = { command, address }
          if (address === 'https://unavailable.invalid') throw 'Check the address and try again.'
        },
      },
    }
  })
  await page.goto(new URL('/desktop-setup/index.html', server.url).href)
  assert(await page.locator('img').first().evaluate((image) => image.complete && image.naturalWidth > 0))
  const address = page.getByRole('textbox', { name: 'Server address' })
  const submit = page.getByRole('button', { name: 'Continue' })
  assert.equal(await address.inputValue(), 'https://chat.ircord.com')
  if (process.env.IRCORD_DESKTOP_TEST_SHOTS) {
    await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, 'ircord-setup.png') })
  }

  await address.fill('https://unavailable.invalid')
  await submit.click()
  assert.equal(await page.getByRole('alert').textContent(), 'Check the address and try again.')
  assert.equal(await address.evaluate((input) => input === document.activeElement), true)
  assert.equal(await submit.isEnabled(), true)

  await page.setViewportSize({ width: 340, height: 350 })
  await page.emulateMedia({ colorScheme: 'light' })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth === innerWidth), true)
  await address.fill('http://localhost:8080')
  assert.equal(await page.getByRole('alert').textContent(), '')
  await address.press('Enter')
  assert.deepEqual(await page.evaluate(() => window.connection), {
    command: 'connect_server',
    address: 'http://localhost:8080',
  })
  assert.equal(await page.getByRole('button', { name: 'Connecting…' }).isDisabled(), true)
  console.log('Server setup layout, default address, keyboard submission, and error recovery verified.')
} finally {
  await browser.close()
  server.stop(true)
}
