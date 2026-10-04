# TrusCo Tracker

TrusCo Tracker runs quietly on a practitioner's Mac or Windows PC. It records which document or app they are working in, links that time to a matter, and sends it to **TrusCo**. In TrusCo the practitioner reviews it and turns it into ordinary unbilled time entries. The existing fee-note flow then invoices those entries.

```
 Mac / PC                                  TrusCo (Lovable · trusco.app)
 ┌──────────────────────────┐   HTTPS    ┌─────────────────────────────────────────────┐
 │ tracker thread (5 s)     │  bearer    │ /api/public/tracker/pair  (one-time code)   │
 │  foreground app, title,  │  ttk_…     │ /api/public/tracker/me                      │
 │  document path, pages,   │ ─────────▶ │ /api/public/tracker/sync → tracker_ingest_  │
 │  idle → work blocks      │            │   blocks(): match matter + suggest tariff   │
 │ sync thread (60 s)       │ ◀───────── │                                             │
 │ tray + small window      │  matter    │ activity_blocks (private to the user)       │
 └──────────────────────────┘  label     │   └─ Captured Time page → approve_activity_ │
                                         │      blocks() → time_entries (unbilled)     │
                                         │        └─ existing fee note / invoice       │
                                         └─────────────────────────────────────────────┘
```

## What gets recorded

| Captured | Not captured |
|---|---|
| App name, window title, document path, Word page count (from the file's metadata) | Keystrokes, screenshots, document or email contents |
| Start/end time and *active* time (idle over 5 min is excluded; calls and meetings are exempt) | Anything while paused, in excluded apps (password managers by default) or in private browser windows |

- Blocks shorter than 1 minute stay on the computer.
- In TrusCo, captured activity is visible only to the user who recorded it until they approve it.
- Nothing becomes billable without that approval.
- The device key is kept in the macOS Keychain or Windows Credential Manager. Local data files are owner-only.

## Matching (server side, `tracker_match_matter`)

Rules are tried in this order:

1. **Rule** (100): a saved folder or keyword rule, for example "anything under `…/Clients/Smit/` is M-1004". Rules are created from the approval dialog or in the Rules section.
2. **Matter number** (95): for example `M-1004` or `m1004` in the file name or path.
3. **Client full name** (80): for example "Jane Smit".
4. **Client surname** (60).

A word from the matter title adds 5 points, which separates matters belonging to the same client. A tie between matters costs 10 points and is explained in the match reason.

The tariff suggestion (`tracker_suggest_billing_item`) uses the firm's own Rate Catalogue:
- Word: the closest "Drafting of …" item, for example a settlement document becomes "Drafting of settlement agreement".
- Email: "Perusal of correspondence".
- PDFs: "Perusal of pleadings".
- Zoom and Teams calls: "Meeting attendance".
- Anything else: "General attendance".

Units are calculated as follows:
- Per 10 min: ⌈seconds ÷ 600⌉
- Per hour: 6-minute steps
- Per page: the Word page count

## Layout

```
trusco/001_activity_tracker.sql   Database objects (applied in TrusCo as drizzle migration 0037)
desktop/core/                     Platform-independent engine (Rust): sampling, blocks, sync, storage
  src/platform/macos.rs           NSWorkspace + Accessibility API + AppleScript for Office paths
  src/platform/windows.rs         Win32 foreground window + GetLastInputInfo + Office COM paths
desktop/src-tauri/                Tauri 2 app: tray menu, threads, commands
desktop/src/                      The small window (pairing, today's activity, settings)
```

## Develop

Requirements: Rust (rustup), Node 20+. On Windows you also need the WebView2 runtime, which ships with Windows 11.

```bash
cd desktop
npm install
npx tauri dev                          # run with hot reload
cd core && cargo test                  # engine, docx, URL and API-contract tests
# type-check the Windows code from a Mac (aws-lc can't be cross-compiled, so use native-tls here)
cargo check --target x86_64-pc-windows-msvc --no-default-features --features native-tls
TRUSCO_URL=https://trusco.app cargo run --example probe   # can this machine reach TrusCo?
```

Networking uses rustls with the operating system's certificate store. This means firms whose networks inspect HTTPS still work. The macOS system TLS stack took 5–30 s to connect to trusco.app, which is why it isn't used.

## Build installers

**Windows: GitHub Actions** (`.github/workflows/build.yml`)
- Actions tab → **build** → **Run workflow**, or push a tag (`git tag v0.1.1 && git push --tags`).
- Each run uploads `TrusCo-Tracker-Windows`, containing the `.msi` and the setup `.exe`, as an artifact. Tag runs also attach both to a draft release.
- The installers are unsigned for now, so Windows SmartScreen shows "More info → Run anyway". Code signing (for example Azure Trusted Signing) removes that warning.

**macOS: signed and notarised locally**, so it opens on any Mac without warnings:

```bash
cd desktop
./scripts/release-mac.sh               # writes ../releases/TrusCo Tracker_<version>_aarch64.dmg
```

One-time setup:
1. Create a **Developer ID Application** certificate in Xcode → Settings → Accounts → Manage Certificates. You need the Account Holder role, or Admin with access to Developer ID.
2. Store notarisation credentials. This prompts for an app-specific password from appleid.apple.com:
   `xcrun notarytool store-credentials trusco-notary --apple-id <Apple ID> --team-id <your Team ID>`

## First run

1. In TrusCo go to **Legal / Trust → Captured Time → Connect a device**. TrusCo shows a code that is valid for 10 minutes.
2. Open TrusCo Tracker, enter the code and choose **Connect**. The app then starts at login and lives in the menu bar or tray.
3. **macOS only:** grant **Accessibility** when prompted, under System Settings → Privacy & Security. Without it the tracker sees only the app name and not the document. The first time Word is in front, macOS also asks whether TrusCo Tracker may "control Microsoft Word". This is only used to read the open document's path.

## Roadmap

- **Phase 2:** Microsoft 365 / Gmail connection, which records sender, recipients and subject (no bodies) and matches by client email. Also Teams and Zoom call records.
- **Phase 3:** WhatsApp Business Platform in coexistence mode, matching messages by client phone number. Also call records from office phone systems (VoIP/PBX).
- **Phase 4:** mobile. Android first, using the call log. iPhone gets a "log that call" prompt, because iOS does not expose call history.
