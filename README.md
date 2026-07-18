# minihoard

A Rust CLI **and** MCP server to fetch your [MyMiniFactory](https://www.myminifactory.com/)
library, unpack and reorganize it into tidy per-creator/month folders, and
repack it (tar.zst or zip, splittable) for archival and cataloging alongside
[plinth](https://github.com/irongollem/plinth).

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

The MCP exposes `status`, `config`, `list_library`, `preview_download`,
`download_objects`, `pack`, `unpack`, `tidy`, and `job_status` — so you can
browse, download, repack, restore, and tidy entirely by chat. The long-running
tools (`download_objects`, `pack`, `unpack`, `tidy`) run in the **background**:
they return a job id right away and the assistant polls `job_status` to report
live progress
("3 of 5 done, downloading Behir…") instead of blocking silently — which would
otherwise trip the client's tool-call timeout on a big batch.

## Setup

```sh
minihoard configure    # API client id (+ secret) and your library folder
minihoard login        # browser OAuth — opens a tab, stores a refresh token
minihoard sync-cookie  # import your MMF session cookie from your browser
```

The session cookie is needed only for **library listing** (downloads use OAuth).
The easy way is `minihoard sync-cookie`, which reads it straight from a browser
you're already logged into MyMiniFactory with (like `yt-dlp --cookies-from-browser`)
— no manual copying. It reads the browser's on-disk cookie store; it does **not**
drive your browser and needs no extension. It auto-detects across installed
browsers, or pick one with `--browser firefox`. On macOS it prompts once for
Keychain access to decrypt Chromium cookies (Firefox isn't encrypted, so no
prompt). Re-run when the cookie expires (rarely — `REMEMBERME` is long-lived).

> **Windows + Chrome/Edge/Brave:** recent Chromium versions (Chrome/Edge 127+)
> added *app-bound encryption* that ties cookie decryption to the browser
> itself, which **breaks disk-based readers** like this one. So on a recent
> Windows machine `sync-cookie --browser edge` (or chrome/brave) will likely
> fail. Two reliable options there:
> - **`minihoard sync-cookie --browser firefox`** — Firefox doesn't encrypt
>   cookies, so it always works. (Log in to MyMiniFactory in Firefox once.)
> - **`minihoard set-cookie`** — paste the `cookie:` header from your browser's
>   DevTools (Network tab → any `www.myminifactory.com` request). Works in any
>   browser.

Secrets are stored in a `0600` file in the app config dir, never a system
keychain, so headless/automated use never prompts.

## Commands

```sh
minihoard list --month 2026-06            # filter by release month
minihoard list --creator "one page rules" # by creator
minihoard list --search dragon --undownloaded
minihoard list --source tribe             # by source channel (tribe/purchase/kickstarter/…)
minihoard get 806054 806051               # by id: download, unpack, clean, reorganize
minihoard get "dragon knight"             # by name: search, pick, then the same
minihoard get --month 2026-06             # batch: this month's whole drop (asks first)
minihoard get --month 2026-06 -j 6        # ...downloading 6 in parallel
minihoard get --creator "one page rules" --undownloaded   # batch: only what's new
minihoard get 806054 --keep-archive       # ...but keep the original .zip
minihoard get --month 2026-06 --pack --split 4G --name "Dungeon Classics - 2026-06"
                                          # download, then repack the month into named 4 GB chunks
minihoard pack ~/mmf/Creator-06-2026      # repack a folder for backup (tar.zst)
minihoard pack DIR --format zip           # ...as a broadly-supported .zip
minihoard pack DIR --split 4G             # ...split into 4 GB volumes (tar.zst)
minihoard pack DIR --name "Archive Name"  # ...with a custom archive filename
minihoard tidy                            # tidy whole library (strip junk, collapse nesting)
minihoard tidy ~/mmf/some-release         # ...or specific folders
minihoard unpack FILE.zip                 # restore a .zip or .tar.zst archive
minihoard unpack FILE.tar.zst.001         # ...or a split archive (first volume)
minihoard unpack FILE.tar.zst --delete-archive  # ...and remove it once extracted
minihoard status                          # install + auth health at a glance
minihoard where                           # show config + where files are stored
minihoard whoami                          # show the logged-in account
minihoard upgrade                         # update to the latest release
```

`status` is the quick "is everything wired up?" check: version, whether OAuth
works (and who you're logged in as), whether a session cookie is stored, and
where your library lives — without changing anything.

### Machine-readable output (`--json`)

`status`, `list`, and `get` accept a global `--json` flag that switches their
output to NDJSON (one JSON object per line) instead of human text: `status`
emits a `status` object, `list` streams one `entry` per object then a `summary`,
and `get` streams `object_start` / `file_progress` / `object_done` /
`object_failed` and closes with `job_done`. Errors become a single
`{"event":"error","kind":…}` line with a non-zero exit. This is the contract the
[plinth](https://github.com/irongollem/plinth) desktop app consumes to drive
minihoard as a library UI; day-to-day terminal use doesn't need it.

`get` (alias of `download`) is the one-shot command: give it object ids or a
name to search for, and it produces ready-to-use releases — each object is
unpacked into `<unpack_dir>/{creator}-{MM-YYYY}/{release}/`, macOS artifacts
(`__MACOSX/`, `.DS_Store`, `._*`) are stripped, and the `.zip` is deleted
(unless `--keep-archive`). A name that matches several items lets you pick which
to fetch. (Searching by name needs the session cookie; ids don't.) `list` marks
items already downloaded; downloads are tracked in a local manifest.

Redundant nesting is collapsed automatically: if an archive contains nothing but
a single top-level folder (the common `Release/Release/files` case), that wrapper
is removed and the release folder takes the inner folder's name — so you get
`{creator}-{MM-YYYY}/{release}/files`, not a doubled-up path. (`unpack` does the
same.)

For the monthly drop, batch a whole set with the same filters `list` uses —
`--month`, `--creator`, `--search`, `--source`, `--undownloaded`. `get` previews
the matches and asks before downloading (use `-y` to skip the prompt). For
example, `minihoard get --month 2026-06 --undownloaded` grabs everything new
this month, and `minihoard get --source tribe --month 2026-06` grabs just your
Tribe-tier releases. `list` with no filters shows breakdowns by month and by
source to help you choose.

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
multi-threaded across your cores. `--name "…"` sets the archive's base filename
verbatim (for a strict archive naming convention) instead of the folder name;
the archive's internal layout still keeps the real folder name.

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
  check, with resumable streaming. Objects download with bounded concurrency
  (default 5, like a browser's per-host connections; tune with `-j` or the
  `download_concurrency` config key).

`get --pack` chains the repack onto a download: after the releases land, each
touched month group is packed into a tar.zst archive. `--split`/`--name` (which
both imply `--pack`) set the chunk size and the archive's filename, so a whole
monthly drop becomes one named, chunked backup in a single command.

## License

MIT — see [LICENSE](LICENSE).
