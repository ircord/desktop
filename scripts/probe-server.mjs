// The selected address has already been validated by the desktop app.
const endpoint = new URL('/api/v1/auth/session', process.env.IRCORD_SETUP_SERVER)
const response = await fetch(endpoint, { redirect: 'error', signal: AbortSignal.timeout(6000) })
if (!response.ok || !response.body) process.exit(1)

const decoder = new TextDecoder()
let body = ''
let bytes = 0
for await (const chunk of response.body) {
  bytes += chunk.byteLength
  if (bytes > 16384) process.exit(1)
  body += decoder.decode(chunk, { stream: true })
}
const session = JSON.parse(body + decoder.decode())
if (session?.account !== null) process.exit(1)
