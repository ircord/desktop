const cli = ['bun', 'tauri', 'icon']
const mac = process.platform === 'darwin'
const appIcon = mac ? 'assets/macos-icon.svg' : '../frontend/public/icons/icon-512.png'

for (const args of [[appIcon], ...(mac ? [['assets/tray-template.svg', '--png', '18']] : [])]) {
  const result = Bun.spawnSync([...cli, ...args], { stdout: 'inherit', stderr: 'inherit' })
  if (!result.success) process.exit(result.exitCode || 1)
}
