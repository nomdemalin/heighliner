# Heighliner

Your files have tiers now. Hot, warm, cold, frozen. Like wine. The frozen ones get forgotten, then thrown away. Eventually.

Heighliner is a native macOS app that watches the directories you choose, moves files according to rules you write, and sorts them by how recently you touched them. It also shows you, in a treemap, exactly where your disk went. You will not like the answer.

Version 0.1.0. It works.

## What it does

- **Watches directories.** FSEvents, debounced to one second, so an editor that saves on every keystroke does not get a voice.
- **Indexes everything** into SQLite: size, created, modified, last access, tier, and which rule owns it.
- **Applies rules.** You declare them in TOML or in the app. Like a firewall, the first matching rule wins. Order matters. Choose wisely.
- **Tiers by age.** Accessed within 7 days is hot. Within 30, warm. Within 90, cold. Beyond that, frozen. All four thresholds are yours to change.
- **Sweeps on a schedule.** Rescan, retier, archive, purge, delete-after. Quietly, in the background, like a good concierge.
- **Shows the damage.** Dashboard with stat tiles, a tier breakdown bar and a squarified treemap. Click a tile to drill in. Sort by size. Reveal in Finder. Send to the Trash, which Finder can undo, because accidents happen to other people.
- **Keeps a log** of every move, archive, purge and skip. Nothing happens off the record.

## Dry run first

There is a `dry_run` switch. Use it. Heighliner logs what it *would* move instead of moving it. Read the log. Then decide whether you trust your own rules.

You do not, yet. That is healthy.

## Build

You need stable Rust (1.80 or newer) and macOS 11 or newer.

```sh
make            # lists every target, grouped
make dev        # debug build, verbose logs
make bundle     # dist/Heighliner.app
make install    # copies to /Applications, refuses to overwrite
make reinstall  # overwrites; you asked for it
```

`make icon` regenerates the icon from `assets/icon.svg` and needs `brew install librsvg`. The PNG is already in the repo, so a fresh clone builds without it.

`make` with no target prints help. It does not build. Patience.

## Where things live

| What   | Where |
| ------ | ----- |
| Rules  | `~/Library/Application Support/Heighliner/rules.toml` |
| Index  | `~/Library/Application Support/Heighliner/index.sqlite` |
| Logs   | `~/Library/Application Support/Heighliner/logs/` |

The config file appears on first launch, empty. Edit it in the app (structured editor or raw TOML) or by hand. The file is written atomically, so a crash does not leave you with half a rule.

`make distclean` removes all of this. It waits three seconds first. Use them.

## Configuration

A complete example. Annotated, because you will forget.

```toml
[general]
dry_run = true            # log, do not touch. Start here.
rescan_interval = "6h"    # full walk of every watched root
retier_interval = "1d"    # re-sort managed files by age

[[watch]]
path = "/Users/you/Downloads"
recursive = true

[[watch]]
path = "/Users/you/Desktop"
recursive = false

[tiers]
hot_within  = "7d"
warm_within = "30d"
cold_within = "90d"       # anything older is frozen

[archive]
destination = "/Volumes/Cellar/heighliner"
compress = true           # zip, DEFLATE, keeps mode and mtime
schedule = "weekly"       # daily | weekly | monthly
purge_frozen_after = "1y" # deletes. Actually deletes.

[[rule]]
name = "PDFs to the library"
[rule.match]
extension = ["pdf"]
source_prefix = "/Users/you/Downloads"
[rule.action]
kind = "move"
to = "/Users/you/Documents/Library"

[[rule]]
name = "Installers are temporary"
[rule.match]
extension = ["dmg", "pkg"]
[rule.action]
kind = "delete_after"
after = "14d"

[[rule]]
name = "Big old videos"
[rule.match]
extension = ["mov", "mp4"]
min_size = 1_000_000_000
older_than = "180d"
[rule.action]
kind = "archive"
to = "/Volumes/Cellar/video"
delete_source = true
```

Durations are humantime: `30s`, `6h`, `7d`, `2months`, `1y`.

### Match fields

All optional. All must hold for a rule to match.

| Field | Meaning |
| ----- | ------- |
| `extension` | Extensions without the dot. Case does not matter. |
| `name_glob` | Globs against the filename. |
| `path_glob` | Globs against the full path. |
| `source_prefix` | File must live under this path. |
| `older_than` | Last access is at least this long ago. |
| `in_tier` | `hot`, `warm`, `cold` or `frozen`. |
| `min_size`, `max_size` | Bytes. Plain integers. |

### Actions

| `kind` | What happens |
| ------ | ------------ |
| `move` | Moves to `to/<tier>/<parent folder>/<file>`. Keeping the parent folder name stops two `invoice.pdf` files from fighting. |
| `retier` | Sets the tier to `to` in the index. |
| `archive` | Zips (or copies) the file into `to`. Removes the source unless `delete_source = false`. |
| `delete_after` | Deletes the file once its last access is older than `after`. Checked hourly. |

### Sweeps

| Sweep | Cadence | Does |
| ----- | ------- | ---- |
| Rescan | `rescan_interval` | Walks every root, indexes, applies rules to what it finds. |
| Retier | `retier_interval` | Recomputes each managed file's tier from its age. |
| Archive | `archive.schedule` | Empties every `frozen/` folder under your move destinations into `archive.destination`. |
| Purge | daily | Deletes frozen files not accessed within `purge_frozen_after`. |
| Delete-after | hourly | Enforces `delete_after` rules. |

The ticker wakes every 30 seconds. An interval shorter than that is ambition, not configuration.

## How it behaves

- **Moves are safe.** Same volume: atomic rename. Across volumes: copy, fsync, then delete. If the destination exists, the new file becomes `name (1).ext`. If it is already where it belongs, Heighliner leaves it alone and logs a skip.
- **Existing files count.** On startup it scans every watched root and applies rules to what is already there, not only to new arrivals. Dry run, remember.
- **Last access means `atime`.** One `stat` per file. Anything that reads a file (a backup tool, an indexer, you) makes it hot again. That is the deal.
- **It stays quiet when idle.** No redraws without a reason. One heartbeat every five seconds so "3 min ago" stays honest.

## Architecture, briefly

```
src/
  main.rs          boot: paths, config, index, engine, window
  app.rs           the shell: side nav, status bar, tabs
  engine/
    mod.rs         engine thread, message loop, caches, ticker
    watcher.rs     FSEvents via notify, debounced
    scanner.rs     metadata → index row
    rules.rs       compile globs, first match wins
    actions.rs     safe_move, unique names
    scheduler.rs   retier, archive, purge, delete-after sweeps
    index.rs       SQLite: files, actions, tier moves, counters
    config.rs      the TOML, typed
  ui/              egui panels: dashboard, treemap, browser,
                   watched, rules, tiers, log, theme
  osx/             Reveal in Finder, move to Trash
sql/               schema migrations
```

Rust, egui 0.29, SQLite (bundled), `notify`. No Electron. Obviously.

## Status

- macOS only. Linux compiles in theory. Theory is not support. A clean build is not support. Questions are not taken.
- No signed or notarised builds. Gatekeeper will have opinions. So does the author, and he is French about it.

## Licence

MIT or Apache-2.0, at your option.

No further questions.
