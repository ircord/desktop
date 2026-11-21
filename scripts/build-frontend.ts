import { copyFileSync, cpSync, mkdirSync, mkdtempSync, realpathSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'

const desktop = resolve(import.meta.dir, '..')
const frontend = resolve(desktop, '../frontend')
const bundle = resolve(desktop, 'src-tauri')

function run(command: string[], cwd: string, env = process.env) {
  const result = Bun.spawnSync(command, { cwd, env, stdout: 'inherit', stderr: 'inherit' })
  if (!result.success) throw new Error(`The frontend command exited with code ${result.exitCode}.`)
}

const rust = Bun.spawnSync(['rustc', '-vV'])
const host = rust.stdout.toString().match(/^host: (\S+)$/m)?.[1]
if (!rust.success || !host) throw new Error('Could not determine the Rust host target.')
if (process.env.TAURI_ENV_TARGET_TRIPLE && process.env.TAURI_ENV_TARGET_TRIPLE !== host) {
  throw new Error('Build IRCord on the same platform and architecture as the target.')
}
if (!(await Bun.file(resolve(frontend, 'package.json')).exists())) {
  throw new Error('Initialize the frontend submodule next to desktop before building.')
}

const bun = process.execPath
const output = resolve(bundle, 'frontend-dist')
const binaries = resolve(bundle, 'binaries')
mkdirSync(binaries, { recursive: true })
copyFileSync(bun, resolve(binaries, `ircord-bun-${host}${process.platform === 'win32' ? '.exe' : ''}`))

if (process.argv.includes('--dev')) {
  // Cargo resolves the bundle paths even though Tauri dev uses Vite.
  mkdirSync(output, { recursive: true })
  const { build: { devUrl } } = await Bun.file(resolve(bundle, 'tauri.web.conf.json')).json()
  const lock = await Bun.file(resolve(frontend, '.nuxt/nuxt.lock')).json().catch(() => null)
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
  run([bun, 'run', 'dev'], frontend)
  process.exit(0)
}

// Build from the sibling checkout. Skip the web deployment's local environment file.
copyFileSync(resolve(frontend, 'public/icons/icon-192.png'), resolve(desktop, 'setup/icon.png'))
const version = Bun.spawnSync(['git', 'rev-parse', '--short', 'HEAD'], { cwd: frontend })
const staging = mkdtempSync(resolve(bundle, 'frontend-build-'))
try {
  run([bun, '--no-env-file', 'run', 'build', '--dotenv', '.env.desktop'], frontend, {
    ...process.env,
    IRCORD_DESKTOP_BUILD_DIR: staging,
    IRCORD_VERSION: version.success ? version.stdout.toString().trim() : '',
  })
  // Nitro links some dependencies. Tauri's resource bundler copies only files.
  rmSync(output, { recursive: true, force: true })
  cpSync(staging, output, { recursive: true, dereference: true })
} finally {
  rmSync(staging, { recursive: true, force: true })
}
