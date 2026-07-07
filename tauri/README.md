# Vardøger, the app

One crate, two shapes:

- **Desktop (macOS / Windows / Linux)** — the WHOLE twin in one app: the runtime
  (`twin_runtime::server::serve`) starts on a thread inside the process and the
  window is the same thin client a browser gets. If a node is already listening
  (a `twin serve` you started yourself, or another window), the app just attaches
  to it — one node, any number of faces.
- **Phone (iOS / Android)** — a thin host. Raw V8 does not cross-compile to
  phones (and iOS forbids JIT anyway), so the app bundles `web/index.html` and
  connects out to a node you run elsewhere. On first launch, tap the **live**
  chip → **Twin node…** and enter the node's `host:port`; the address is
  remembered.

## Desktop

```bash
cd tauri
cargo run                # dev: builds and opens the app
cargo tauri build        # bundle: .app / .dmg (needs `cargo install tauri-cli`)
```

Where the twin lives (its `data/` and `skills/`), in order of preference:

1. `TWIN_HOME=/path/to/home` — explicit;
2. the working directory, when it holds a `data/` (running from the checkout);
3. the OS app-data dir (`~/Library/Application Support/no.oppeboen.vardoger` on
   macOS), created on first launch — the packaged app's default.

`TWIN_ADDR` overrides the embedded node's address (default `127.0.0.1:8787`).

## Phone

The phone app is built from this same crate with Tauri's mobile targets. This
needs the platform toolchains (neither is on a machine with only Command Line
Tools):

- **iOS**: full Xcode from the App Store, an Apple ID for signing, and
  `rustup target add aarch64-apple-ios`
- **Android**: Android Studio (SDK + NDK), `ANDROID_HOME`/`NDK_HOME` exported,
  and `rustup target add aarch64-linux-android`

Then, from `tauri/`:

```bash
cargo install tauri-cli          # once
cargo tauri ios init && cargo tauri ios dev        # simulator / device
cargo tauri android init && cargo tauri android dev
cargo tauri ios build            # .ipa      (signing set up in Xcode once)
cargo tauri android build        # .apk/.aab (install with adb)
```

Two generated files need one-time touches, because the thin host speaks plain
`ws://`/`http://` to a node on your LAN:

- **iOS** (`gen/apple/…/Info.plist`): allow non-TLS loads and local-network
  access —

  ```xml
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsArbitraryLoads</key><true/></dict>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Vardøger connects to your twin node on the local network.</string>
  ```

- **Android** (`gen/android/…/AndroidManifest.xml`): on the `<application>`
  element, `android:usesCleartextTraffic="true"`.

### Reaching the node from the phone

The node must listen on an address the phone can reach — the desktop app's
default `127.0.0.1` is deliberately loopback-only. On the machine that owns the
twin:

```bash
cargo run --bin serve 0.0.0.0:8787      # or TWIN_ADDR=0.0.0.0:8787 for the app
```

then point the phone at `<that machine's LAN IP>:8787` via **Twin node…**.

**Caveat, worth reading twice:** the node speaks plain HTTP with no
authentication. On `0.0.0.0` anyone on the same network can read and steer the
twin. Keep it loopback-only except on networks you trust (home Wi-Fi, a
WireGuard/Tailscale interface — with Tailscale, bind to the tailnet IP and the
phone reaches it from anywhere, encrypted and member-only). Peer sync does NOT
have this problem — it is end-to-end encrypted and only paired keys connect
(see `src/sync.rs`); giving the phone's UI channel the same story is the
natural next step.

## Icons

`icons/` is generated from the brand mark (the vardøger: a leading circle and
its echo). Regenerate at any size with the CoreGraphics script in the repo
history, or replace with `cargo tauri icon <1024px png>`.
