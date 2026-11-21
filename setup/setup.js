const form = document.querySelector('form')
const address = document.querySelector('#server-address')
const error = document.querySelector('#server-error')
const button = form.querySelector('button')
const progress = document.querySelector('#progress')

function showError(message) {
  error.textContent = message
  address.setAttribute('aria-invalid', 'true')
  address.focus()
}

address.addEventListener('input', () => {
  error.textContent = ''
  address.removeAttribute('aria-invalid')
})

form.addEventListener('submit', async (event) => {
  event.preventDefault()
  if (button.disabled) return
  error.textContent = ''
  address.removeAttribute('aria-invalid')
  address.readOnly = true
  button.disabled = true
  progress.textContent = button.textContent = 'Connecting…'
  try {
    await window.__TAURI__.core.invoke('connect_server', { address: address.value.trim() })
  } catch (cause) {
    progress.textContent = ''
    button.textContent = 'Continue'
    button.disabled = false
    address.readOnly = false
    showError(typeof cause === 'string' ? cause : 'Could not open IRCord. Try again.')
  }
})
