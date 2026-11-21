# IRCord desktop

The Tauri app packages the Nuxt frontend from the sibling `frontend/` checkout.
At launch, the app serves that build over a private loopback address. The app connects to the IRCord backend for accounts, messages, files, and live events.

## Prerequisites

- A recursive checkout of the [IRCord development workspace](https://github.com/ircord/dev-environment).
- Bun 1.4.4, Rust, and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your platform.

## Procedure

1. Run `bun install --frozen-lockfile` in `frontend/`.
2. Run `bun install --frozen-lockfile` in `desktop/`.
3. Run `bun run build` in `desktop/`.
4. Open the bundle from `desktop/src-tauri/target/release/bundle/`.

Build on the platform and architecture that will run the bundle. The build generates the app icon from the frontend's artwork.
On macOS, use `bun run build --bundles app` to build the application without a disk image.

## Verification

On first launch, enter the address of your IRCord server. The app suggests `https://chat.ircord.com`.
The app uses the saved address on later launches.
Set `IRCORD_DESKTOP_BACKEND_URL` before launching the executable to supply the address ahead of time.
The app accepts HTTPS server addresses and local HTTP addresses on `127.0.0.1`.
Each server has its own browser data. On macOS versions before 14, the app uses temporary browser data.

Run `bun tauri dev` in `desktop/` to test the desktop flow from a local frontend build.
For frontend hot reload, start the Go backend with `make backend` in the workspace root.
Run `bun run dev:web` in `desktop/` to open the Nuxt development frontend directly.
If this checkout's Nuxt server already runs on port 3000, Tauri uses it.

To check a macOS bundle in WebKit, start a disposable Go backend on `127.0.0.1:41987` with registration enabled.
From `desktop/`, run the following command after installing the frontend's Playwright WebKit browser.

```sh
bun scripts/verify-frontend.mjs http://127.0.0.1:41987 \
  src-tauri/target/release/bundle/macos/IRCord.app/Contents/Resources/frontend/server/index.mjs \
  src-tauri/target/release/bundle/macos/IRCord.app/Contents/MacOS/ircord-bun
```

## Recovery

If the app cannot start its local frontend, close other IRCord desktop windows and check whether another process uses TCP port 47832.
To choose a different server, close IRCord and remove the `server` file from the app's configuration directory.
On macOS, the directory is `~/Library/Application Support/com.ircord.desktop/`.
