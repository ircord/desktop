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
          (window.connections ??= []).push({ command, address })
          if (address === 'google.com') throw 'Could not verify this address as an IRCord server.'
          if (command === 'check_server') {
            if (address === 'http://localhost:8080') {
              return { origin: 'http://127.0.0.1:8080', registration: 'closed' }
            }
            if (address === 'https://chat.ircord.com') {
              return { origin: address, registration: 'keyRequired' }
            }
            return { origin: `https://${address}`, registration: 'open' }
          }
        },
      },
    }
  })
  await page.goto(new URL('/desktop-setup/index.html', server.url).href)
  assert(await page.locator('img').first().evaluate((image) => image.complete && image.naturalWidth > 0))
  const address = page.getByRole('textbox', { name: 'Server address' })
  const check = page.getByRole('button', { name: 'Check server' })
  const continueButton = page.getByRole('button', { name: 'Continue' })
  assert.equal(await address.inputValue(), 'https://chat.ircord.com')
  if (process.env.IRCORD_DESKTOP_TEST_SHOTS) {
    await page.screenshot({ path: resolve(process.env.IRCORD_DESKTOP_TEST_SHOTS, 'ircord-setup.png') })
  }

  await address.fill('google.com')
  await check.click()
  assert.equal(await page.getByRole('alert').textContent(), 'Could not verify this address as an IRCord server.')
  assert.equal(await address.evaluate((input) => input === document.activeElement), true)
  assert.equal(await check.isEnabled(), true)
  assert.equal(await continueButton.count(), 0)

  await address.fill('https://chat.ircord.com')
  await check.click()
  await page.getByText(/need a registration key/).waitFor()
  await continueButton.waitFor()
  await address.fill('irc.example.com')
  assert.equal(await continueButton.count(), 0)
  assert.equal(await page.getByRole('status').textContent(), '')
  await check.click()
  await page.getByText(/You can create an account/).waitFor()
  assert.equal(await address.inputValue(), 'https://irc.example.com')

  await page.setViewportSize({ width: 340, height: 350 })
  await page.emulateMedia({ colorScheme: 'light' })
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth === innerWidth), true)
  await address.fill('http://localhost:8080')
  assert.equal(await page.getByRole('alert').textContent(), '')
  await address.press('Enter')
  await page.getByText(/Registration is closed/).waitFor()
  assert.equal(await address.inputValue(), 'http://127.0.0.1:8080')
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth === innerWidth), true)
  const calls = await page.evaluate(() => window.connections)
  assert.ok(calls.every(({ command }) => command === 'check_server'))
  await continueButton.click()
  assert.deepEqual(await page.evaluate(() => window.connections.at(-1)), {
    command: 'connect_server',
    address: 'http://127.0.0.1:8080',
  })
  assert.equal(await page.getByRole('button', { name: 'Opening IRCord…' }).isDisabled(), true)
  console.log('Server verification, all registration states, keyboard submission, and error recovery verified.')
} finally {
  await browser.close()
  server.stop(true)
}
