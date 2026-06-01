# minihoard

A Rust CLI to fetch your [MyMiniFactory](https://www.myminifactory.com/) library,
unpack it, and (later) restructure + repack it for archival and cataloging
alongside [stl-pack](https://github.com/irongollem/stl-pack).

> Status: **early scaffolding.** `configure` and `unpack` work; auth, listing,
> and download are stubbed pending the M0 feasibility spike (see below).

## Why

Every month you want to pull your new MMF releases and pack them for storage and
later cataloging. `minihoard` automates: **login → list → download → unpack**,
with restructure/repack to come.

## Architecture

A Cargo workspace:

- `crates/mmf-core` — all logic as a library (auth, API client, download,
  unpack). Built as a lib so the CLI and a future MCP server stay thin.
- `crates/mmf-cli` — the `minihoard` binary.

Non-secret settings live in `config.toml`
(`~/Library/Application Support/minihoard/` on macOS). The OAuth **refresh
token** is stored in the OS keychain, never on disk.

## Commands

```
minihoard configure   # first-run wizard: API client id + directories
minihoard login       # browser OAuth, stores refresh token        (M2)
minihoard logout      # clear stored credentials
minihoard list        # show available releases                    (M4)
minihoard download    # download releases by id, or all new        (M5)
minihoard unpack FILE # extract a downloaded .zip
minihoard sync        # monthly flow: list -> download -> unpack    (M7)
```

## ⚠️ Open question (M0 spike)

The documented MMF API exposes your **published** objects, **liked** objects,
and **collections** — there is no clearly documented "things I purchased /
subscribed to" endpoint. Download URLs work only for an OAuth-connected user.
The first build step authenticates and confirms we can list and download an
item you actually own, before investing in the rest.

## Roadmap

- Restructure unpacked releases into stl-pack's `{designer}-{MM-YYYY}-{release}`
  layout (generated `release.json` / `model.json`, `supported/` detection).
- Repack as **tar + zstd** (primary), then optional 2/4 GB multi-volume split
  for off-site backup.
- MCP server facade over `mmf-core`.

## License

MIT
