# minihoard

A Rust CLI **and** MCP server to fetch your [MyMiniFactory](https://www.myminifactory.com/)
library, unpack it, and (soon) restructure + repack it for archival and
cataloging alongside [stl-pack](https://github.com/irongollem/stl-pack).

Browse and download from the terminal, or talk to an assistant (via the MCP
server) — "show me June's Tribe releases", "download these".

## Install

**Download a prebuilt binary** (no toolchain needed) from the
[latest release](https://github.com/irongollem/minihoard/releases/latest):

- Windows: `minihoard-x86_64-pc-windows-msvc.exe` (+ `minihoard-mcp-…exe`)
- macOS (Apple Silicon): `minihoard-aarch64-apple-darwin` (+ `minihoard-mcp-…`)

Rename them to `minihoard`/`minihoard-mcp` (or `.exe`) and put them on your PATH.

**Or build from source** — needs Rust, plus CMake and (on Windows) NASM for
BoringSSL: `cargo build --release`.

## Setup

```sh
minihoard configure    # API client id (+ secret) and download/unpack folders
minihoard login        # one-time browser OAuth (stores a refresh token)
minihoard set-cookie   # paste your MMF website Cookie header (for library listing)
```

Why a cookie? Downloads use OAuth, but the full **library listing** endpoint is
gated on the website session cookie, so you paste it once (re-paste when it
expires). Secrets are stored in a `0600` file in the app config dir — **not** a
system keychain — so headless/automated use never prompts.

## Commands

```sh
minihoard list --month 2026-06            # filter by release month
minihoard list --creator "one page rules" # by creator
minihoard list --search dragon --undownloaded
minihoard download 806054 806051          # download (auto-unpacks zips)
minihoard unpack FILE.zip                 # unpack a local archive
minihoard whoami                          # show the logged-in account
```

`list` marks items you've already downloaded; downloads are tracked in a local
manifest.

## MCP server (talk to an assistant)

`minihoard-mcp` is a stdio MCP server exposing `status`, `list_library`,
`preview_download`, and `download_objects`. Point your MCP client at it, e.g.
Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "minihoard": { "command": "/path/to/minihoard-mcp" }
  }
}
```

Then ask it to browse and download; it previews sizes before fetching.

## How it works

- **Auth**: OAuth2 Authorization Code via a localhost loopback redirect.
- **Discovery**: the website's `data-library/objectPreviews` endpoint lists the
  whole library (id, name, creator, release month, added date).
- **Download**: object files via the OAuth API; the download host is behind
  Cloudflare, so requests use a Chrome-impersonating client to pass the bot
  check, with resumable streaming.

## Roadmap

- Restructure downloads into stl-pack's `{designer}-{MM-YYYY}` layout and repack
  as tar + zstd (with a crate shared with stl-pack).
- Per-source filters/grouping (Tribes / shared / Kickstarter / store).

## License

MIT — see [LICENSE](LICENSE).
