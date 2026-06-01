# minihoard

A Rust CLI **and** MCP server to fetch your [MyMiniFactory](https://www.myminifactory.com/)
library, unpack it, and (soon) restructure + repack it for archival and
cataloging alongside [stl-pack](https://github.com/irongollem/stl-pack).

Browse and download from the terminal, or talk to an assistant (via the MCP
server) — "show me June's Tribe releases", "download these".

## Install

### macOS

```sh
curl -fsSL https://github.com/irongollem/minihoard/releases/latest/download/install.sh | sh
```

### Windows (PowerShell)

```powershell
irm https://github.com/irongollem/minihoard/releases/latest/download/install.ps1 | iex
```

Both scripts download `minihoard` and `minihoard-mcp`, place them in a local
bin folder, and add it to your PATH automatically.

**Or build from source** — needs Rust, plus CMake and (on Windows) NASM for
BoringSSL: `cargo build --release`.

### MCP (Claude Desktop)

After install, one command wires up the MCP server:

```sh
minihoard setup-mcp
```

That's it — it edits `claude_desktop_config.json` automatically. Restart Claude
Desktop, then ask it to browse and download your library.

## Setup

```sh
minihoard configure    # API client id (+ secret) and download/unpack folders
minihoard login        # browser OAuth — opens a tab, stores a refresh token
minihoard set-cookie   # one-time: paste your MMF session cookie (see below)
```

The session cookie is needed only for **library listing** (downloads use OAuth).
Run `minihoard set-cookie` and follow the prompt — it tells you exactly which
header to copy from your browser's DevTools. Re-run when the cookie expires.
Secrets are stored in a `0600` file in the app config dir, never a system
keychain, so headless/automated use never prompts.

## Commands

```sh
minihoard list --month 2026-06            # filter by release month
minihoard list --creator "one page rules" # by creator
minihoard list --search dragon --undownloaded
minihoard download 806054 806051          # download, unpack, clean, reorganize
minihoard download 806054 --keep-archive  # ...but keep the original .zip
minihoard unpack FILE.zip                 # unpack a local archive
minihoard whoami                          # show the logged-in account
minihoard upgrade                         # update to the latest release
```

`download` produces ready-to-use releases: each object is unpacked into
`<unpack_dir>/{creator}-{MM-YYYY}/{release}/`, macOS artifacts (`__MACOSX/`,
`.DS_Store`, `._*`) are stripped, and the `.zip` is deleted (unless
`--keep-archive`). `list` marks items already downloaded; downloads are tracked
in a local manifest.

## How it works

- **Auth**: OAuth2 Authorization Code via a localhost loopback redirect.
- **Discovery**: the website's `data-library/objectPreviews` endpoint lists the
  whole library (id, name, creator, release month, added date).
- **Download**: object files via the OAuth API; the download host is behind
  Cloudflare, so requests use a Chrome-impersonating client to pass the bot
  check, with resumable streaming.

## Roadmap

- Optional repack as tar + zstd (and chunked archives) for off-site backup,
  sharing the format with stl-pack.
- Per-source filters/grouping (Tribes / shared / Kickstarter / store).

## License

MIT — see [LICENSE](LICENSE).
