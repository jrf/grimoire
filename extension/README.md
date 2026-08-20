# Grimoire browser extension

Send the paper you're looking at — an arXiv abstract, a DOI/publisher landing
page, a PubMed record, or a direct PDF link — straight into your local Grimoire
library with one click or a keyboard shortcut (⌘⇧G / Ctrl+Shift+G).

Works in **Chrome, Edge, Brave, Vivaldi, Firefox, and Safari**. There is no
server and nothing leaves your machine: the extension talks to the `grimoire`
binary on your computer through the browser's [native messaging] protocol.

## How it works

```
browser extension  ──native messaging──▶  grimoire browser-host  ──▶  cmd_add()  ──▶  ~/Papers
   (background.js)      (stdio, JSON)        (src/browser_host.rs)
```

The extension reduces the current tab's URL to the best identifier Grimoire
understands (bare arXiv ID, DOI, PMID/PMCID, or the raw URL) and sends
`{ "action": "add", "input": "..." }`. The host runs the same import path as
`grimoire add`.

## Setup

### 1. Build the extension bundles

```sh
just build-extension        # or: ./scripts/build-extension
```

This writes `dist/chrome/` and `dist/firefox/` from the shared source in
`extension/`.

### 2. Load the extension and note its ID

- **Chrome / Edge / Brave / Vivaldi:** open `chrome://extensions`, enable
  *Developer mode*, click *Load unpacked*, and select `dist/chrome/`. Copy the
  **extension ID** shown on the card.
- **Firefox:** open `about:debugging` → *This Firefox* → *Load Temporary
  Add-on* and pick `dist/firefox/manifest.json`. The ID is the fixed
  `grimoire@jrf.github` from the manifest.

### 3. Install the native-messaging host manifest

The browser only launches `grimoire browser-host` if a manifest names it and
allow-lists your extension ID:

```sh
# Chrome/Edge/Brave: use the ID you copied above
grimoire install-browser-host --extension-id <chrome-extension-id>

# Firefox uses the fixed gecko id
grimoire install-browser-host --extension-id grimoire@jrf.github
```

You can pass `--extension-id` multiple times to allow-list several browsers at
once. The command writes `com.grimoire.host.json` into every installed browser's
native-messaging directory and points it at the current `grimoire` executable
(override with `--binary /path/to/grimoire`). Re-run it whenever the binary
moves or a Chrome extension ID changes.

### 4. Use it

- Click the toolbar icon → **Add this page**, or
- press **⌘⇧G** / **Ctrl+Shift+G**, or
- right-click the page or a link → **Add … to Grimoire**.

A notification reports the new library key (or "already in your library").

## Safari

Safari extensions must be wrapped in an app bundle. On macOS with Xcode:

```sh
just build-extension
xcrun safari-web-extension-converter dist/chrome --project-location dist/safari
```

Open the generated Xcode project, run it once to register the app, then enable
the extension in **Safari → Settings → Extensions**. Safari reaches the host
through the same `com.grimoire.host` manifest, so step 3 applies as well; the
app the converter builds also allows you to sign and distribute it.

## Troubleshooting

- **"Could not reach Grimoire."** — the native host manifest is missing or the
  extension ID isn't allow-listed. Re-run `grimoire install-browser-host
  --extension-id <id>` and reload the extension. Confirm `grimoire` is on your
  `PATH` (the manifest stores an absolute path, so `--binary` may help).
- **Wrong library.** — the host imports into the same library as the CLI. Set
  `GRIM_LIBRARY` or the `library` key in `~/.config/grimoire/config.toml`.
- **Nothing imported (status "skipped").** — a matching DOI/title already
  exists; use the popup's *Import even if it already exists* checkbox to force.

[native messaging]: https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging
