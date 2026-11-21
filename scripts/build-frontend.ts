import { cpSync, lstatSync, mkdirSync, mkdtempSync, readlinkSync, realpathSync, rmSync, unlinkSync } from 'node:fs'
import { resolve } from 'node:path'

const desktop = resolve(import.meta.dir, '..')
const frontend = resolve(desktop, '../frontend')
const bundle = resolve(desktop, 'src-tauri')

function run(command: string[], cwd: string, env = process.env) {
  const result = Bun.spawnSync(command, { cwd, env, stdout: 'inherit', stderr: 'inherit' })
  if (!result.success) throw new Error(`The frontend command exited with code ${result.exitCode}.`)
}

if (!(await Bun.file(resolve(frontend, 'package.json')).exists())) {
  throw new Error('Initialize the frontend submodule next to desktop before building.')
}

const bun = process.execPath
const output = resolve(bundle, 'frontend-dist')

if (process.argv.includes('--dev')) {
  // Cargo resolves the bundle paths even though Tauri dev uses Vite.
  mkdirSync(output, { recursive: true })
  const { build: { devUrl } } = await Bun.file(resolve(bundle, 'tauri.conf.json')).json()
  const { hostname, port } = new URL(devUrl)
  const buildDir = resolve(frontend, '.data/desktop-nuxt')
  const lock = await Bun.file(resolve(buildDir, 'nuxt.lock')).json().catch(() => null)
  if (
    lock?.command === 'dev' && lock.cwd === realpathSync(frontend) && lock.url === devUrl &&
    Number.isSafeInteger(lock.pid) && lock.pid > 0
  ) {
    try {
      process.kill(lock.pid, 0)
      const response = await fetch(devUrl, { redirect: 'error', signal: AbortSignal.timeout(3000) })
      await response.body?.cancel()
      if (response.ok) {
        console.log(`Using the running IRCord frontend at ${devUrl}.`)
        process.exit(0)
      }
    } catch {
      // Nuxt owns stale lock recovery when there is no running frontend.
    }
  }
  const occupied = await fetch(devUrl, { redirect: 'manual', signal: AbortSignal.timeout(1000) })
    .then(async (response) => {
      await response.body?.cancel()
      return true
    }, () => false)
  if (occupied) throw new Error(`Port ${port} is already in use. Close that server before starting IRCord Desktop.`)

  run([bun, 'x', 'nuxt', 'dev', '--host', hostname, '--port', port], frontend, {
    ...process.env,
    IRCORD_DESKTOP_DEV_DIR: buildDir,
    IRCORD_DESKTOP_SETUP_DIR: resolve(desktop, 'setup'),
    NUXT_DEMO: 'false',
  })
  process.exit(0)
}

// Build from the sibling checkout. Skip the web deployment's local environment file.
const version = Bun.spawnSync(['git', 'rev-parse', '--short', 'HEAD'], { cwd: frontend })
const release = process.env.IRCORD_DESKTOP_FRONTEND_TAG
if (release && !/^v\d+\.\d+\.\d+$/.test(release)) {
  throw new Error('The bundled frontend version must be a stable release tag.')
}
const staging = mkdtempSync(resolve(bundle, 'frontend-build-'))
try {
  run([bun, '--no-env-file', 'x', 'nuxt', 'generate', '--dotenv', '.env.desktop'], frontend, {
    ...process.env,
    IRCORD_DESKTOP_BUILD_DIR: staging,
    NUXT_DEMO: 'false',
    IRCORD_VERSION: release || (version.success ? version.stdout.toString().trim() : ''),
  })
  // Tauri embeds the generated site and the server chooser in the application.
  rmSync(output, { recursive: true, force: true })
  cpSync(resolve(staging, 'public'), output, { recursive: true, dereference: true })
  cpSync(resolve(desktop, 'setup'), resolve(output, 'desktop-setup'), { recursive: true })
} finally {
  const dist = resolve(frontend, 'dist')
  if (
    lstatSync(dist, { throwIfNoEntry: false })?.isSymbolicLink() &&
    resolve(frontend, readlinkSync(dist)) === resolve(staging, 'public')
  ) unlinkSync(dist)
  rmSync(staging, { recursive: true, force: true })
}
