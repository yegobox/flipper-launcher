# Yegobox Launcher

A small desktop app (Tauri 2) that installs Yegobox apps (Flipper POS for now)
and keeps them up to date. It also keeps itself up to date.

```
yegobox/flipper  desktop_launcher_release.yml
  └─ release desktop-v<ver>     flipper-{windows-x86_64.zip, darwin.tar.gz, linux-x86_64.tar.gz} + .sig
  └─ release launcher-manifest  apps.json   ← the catalog this launcher reads
yegobox/flipper-launcher  release.yml (tag launcher-v*)
  └─ release launcher-v<ver>    installers + latest.json   ← the launcher's own updates
```

## How it works

1. It fetches `apps.json` at startup and then every 6 hours. A dev build can
   use a different catalog by setting `LAUNCHER_MANIFEST_URL`.
2. It picks a release for this computer's platform (`windows-x86_64`,
   `darwin-aarch64`, `darwin-x86_64`, `linux-x86_64`) on the user's channel.
   Users on the beta channel get the newer of beta and stable.
3. It downloads the archive and checks its **sha256** and its **minisign
   signature** against the public key in `tauri.conf.json`
   (`plugins.updater.pubkey`). The launcher's own updates use the same key.
4. It extracts the archive to `versions/<ver>.partial`, confirms the entry
   executable exists, and only then renames the folder to `versions/<ver>`.
   If the app is running, the new version is staged and becomes current the
   next time the app is opened.
5. It keeps the current version and the previous one, so a rollback is
   available.
6. If a freshly updated version exits within 10 seconds of launch, the window
   offers to roll back. The abandoned version is skipped until a newer one
   ships.

Install root: `<app_local_data_dir>/apps/<id>/`. On Windows that is
`%LOCALAPPDATA%\com.yegobox.launcher\apps\flipper`, and on macOS it is
`~/Library/Application Support/com.yegobox.launcher/apps/flipper`. Apps keep
their own data (Ditto, the DB, preferences) in their own data directories, so
swapping versions leaves it alone.

**Shortcuts:**
- On first install, Windows gets a Start-menu and desktop shortcut, and Linux
  gets a `.desktop` entry. Both run `launcher --launch flipper`, which applies
  any staged update, opens Flipper and exits.
- If the launcher is already in the tray, the single-instance plugin passes the
  click to it instead.
- macOS users open Flipper from the launcher window.

**Background updates:** the launcher starts at login with `--background` (tray
only). This can be turned off in Settings.

## Develop

```bash
npm install
npm run tauri dev                                   # live catalog
LAUNCHER_MANIFEST_URL=http://127.0.0.1:8000/apps.json npm run tauri dev
cargo test --manifest-path src-tauri/Cargo.toml     # manifest, verify, extract, stage, rollback
```

To test against a local catalog:

```bash
npx tauri signer generate -w ~/.tauri/launcher-dev.key   # throwaway key
# put the printed public key in tauri.conf.json (dev only), then:
npx tauri signer sign -f ~/.tauri/launcher-dev.key flipper-darwin.tar.gz
python3 ../flipper/scripts/ci/update_launcher_manifest.py --manifest apps.json \
  --app flipper --name "Flipper POS" --channel stable --version 1.0.0 \
  --base-url http://127.0.0.1:8000 --platform darwin-aarch64=flipper-darwin.tar.gz:flipper.app
python3 -m http.server 8000
```

## One-time release setup

1. Run `npx tauri signer generate -w ~/.tauri/yegobox-launcher.key`. Put the
   public key in `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`.
   Store the private key and its password as `TAURI_SIGNING_PRIVATE_KEY` and
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in **both** `yegobox/flipper-launcher`
   and `yegobox/flipper`. Never commit the key.
2. **macOS:** you need a Developer ID Application certificate and
   notarization credentials. The secret names are listed in
   `.github/workflows/release.yml`.
3. **Windows:** builds are unsigned for now, so SmartScreen shows "unknown
   publisher" until a code-signing certificate is added.
4. To release, bump the version in `tauri.conf.json`, `Cargo.toml` and
   `package.json`, then push the tag `launcher-v<version>`.
