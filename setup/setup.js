const form = document.querySelector('form')
const address = document.querySelector('#server-address')
const error = document.querySelector('#server-error')
const button = form.querySelector('button')
const status = document.querySelector('#server-status')
const registrationMessage = {
  open: 'You can create an account on this server.',
  keyRequired: 'You’ll need a registration key to create an account.',
  closed: 'Registration is closed. You can sign in with an existing account.',
}
let verifiedAddress = ''

function showError(message) {
  error.textContent = message
  address.setAttribute('aria-invalid', 'true')
  address.focus()
}

address.addEventListener('input', () => {
  verifiedAddress = ''
  status.textContent = ''
  error.textContent = ''
  button.textContent = 'Check server'
  address.removeAttribute('aria-invalid')
})

form.addEventListener('submit', async (event) => {
  event.preventDefault()
  if (button.disabled) return
  error.textContent = ''
  address.removeAttribute('aria-invalid')
  const submittedAddress = address.value.trim()
  const opening = verifiedAddress !== '' && submittedAddress === verifiedAddress
  address.readOnly = true
  button.disabled = true
  status.textContent = button.textContent = opening ? 'Opening IRCord…' : 'Checking server…'
  try {
    if (opening) {
      await window.__TAURI__.core.invoke('connect_server', { address: submittedAddress })
      return
    }
    const server = await window.__TAURI__.core.invoke('check_server', { address: submittedAddress })
    verifiedAddress = address.value = server.origin
    status.textContent = `IRCord server found. ${registrationMessage[server.registration]}`
  } catch (cause) {
    verifiedAddress = ''
    status.textContent = ''
    address.readOnly = false
    showError(typeof cause === 'string' ? cause : 'Could not check the server. Try again.')
  }
  button.textContent = verifiedAddress ? 'Continue' : 'Check server'
  button.disabled = false
  address.readOnly = false
})
