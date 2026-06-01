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

The MCP tools are `status`, `list_library`, `preview_download`, `download_objects`,
and `job_status`. Downloads run in the **background**: `download_objects` returns
a job id right away and the assistant polls `job_status` to report live progress
("3 of 5 done, downloading Behir…") instead of blocking silently on a big batch.

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
minihoard get 806054 806051               # by id: download, unpack, clean, reorganize
minihoard get "dragon knight"             # by name: search, pick, then the same
minihoard get --month 2026-06             # batch: this month's whole drop (asks first)
minihoard get --creator "one page rules" --undownloaded   # batch: only what's new
minihoard get 806054 --keep-archive       # ...but keep the original .zip
minihoard pack ~/mmf/Creator-06-2026      # repack a folder for backup (tar.zst)
minihoard pack DIR --format zip           # ...as a broadly-supported .zip
minihoard pack DIR --split 4G             # ...split into 4 GB volumes (tar.zst)
minihoard unpack FILE.zip                 # restore a .zip or .tar.zst archive
minihoard unpack FILE.tar.zst.001         # ...or a split archive (first volume)
minihoard unpack FILE.tar.zst --delete-archive  # ...and remove it once extracted
minihoard whoami                          # show the logged-in account
minihoard upgrade                         # update to the latest release
```

`get` (alias of `download`) is the one-shot command: give it object ids or a
name to search for, and it produces ready-to-use releases — each object is
unpacked into `<unpack_dir>/{creator}-{MM-YYYY}/{release}/`, macOS artifacts
(`__MACOSX/`, `.DS_Store`, `._*`) are stripped, and the `.zip` is deleted
(unless `--keep-archive`). A name that matches several items lets you pick which
to fetch. (Searching by name needs the session cookie; ids don't.) `list` marks
items already downloaded; downloads are tracked in a local manifest.

For the monthly drop, batch a whole set with the same filters `list` uses —
`--month`, `--creator`, `--search`, `--undownloaded`. `get` previews the matches
and asks before downloading (use `-y` to skip the prompt). For example,
`minihoard get --month 2026-06 --undownloaded` grabs everything new this month.

### Packing for backup

`pack` turns a clean release folder into a single archive for off-site backup.
Two formats:

- **`tar.zst`** (default) — best compression and speed, and the only format
  that can be split into fixed-size volumes (`--split 4G`) for chunked backup.
  It's a stream, so reading one file means decompressing the archive. Restore
  with `minihoard unpack`, or with standard tools
  (`zstd -dc a.tar.zst | tar -x`; for splits, `cat a.tar.zst.* | zstd -dc | tar -x`).
- **`zip`** (`--format zip`) — broadly supported, native double-click
  extraction, and random-access (a catalog can read one entry without unpacking
  everything). Single file only — no `--split`.

`--level N` sets the zstd level (1–22, default 19). Compression runs
multi-threaded across your cores.

Each archive gets a `<archive>.json` **sidecar index** next to it (disable with
`--no-sidecar`). It lists the creator, release month, and every file inside —
categorized as `model` / `image` / `doc` / `other` — so a catalog (or you) can
see what's in an archive, and find printable meshes or preview images, *without
decompressing it*. This matters most for `tar.zst`, which has no random access.

To restore, `minihoard unpack` handles `.zip` and `.tar.zst` (point it at the
`.001` volume for a split set). Add `--delete-archive` to remove the archive —
all split volumes plus the sidecar — once extraction succeeds.

## How it works

- **Auth**: OAuth2 Authorization Code via a localhost loopback redirect.
- **Discovery**: the website's `data-library/objectPreviews` endpoint lists the
  whole library (id, name, creator, release month, added date).
- **Download**: object files via the OAuth API; the download host is behind
  Cloudflare, so requests use a Chrome-impersonating client to pass the bot
  check, with resumable streaming.

## Roadmap

- `download --pack` to repack each release right after downloading.
- Per-source filters/grouping (Tribes / shared / Kickstarter / store).

## License

MIT — see [LICENSE](LICENSE).
