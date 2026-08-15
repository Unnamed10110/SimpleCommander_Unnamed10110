# SimpleCommander

A fast, XYplorer-class dual-pane file explorer for Windows, written in Rust.

![status](https://img.shields.io/badge/status-alpha-orange)

## Highlights

- **Dual pane** (vertical or horizontal split) with tabs per pane, locked tabs,
  and full session persistence — or single-pane mode.
- **Extremely fast**: raw `FindFirstFileExW` enumeration with large-fetch
  batching, a GPU-rendered UI (egui + wgpu) with fully virtualized lists, and
  a strict "the UI thread never touches the filesystem" architecture.
- **Instant search**: Everything-style NTFS MFT indexing (`FSCTL_ENUM_USN_DATA`)
  with live USN-journal updates when running elevated; graceful fallback
  indexer otherwise. Plus scoped content search.
- **AMOLED theme**: the flagship theme is pure `#000000` everywhere — made for
  OLED panels — with a standard dark theme as alternative.
- **Full shell integration**: Explorer context menu, Open With, Properties,
  recycle-bin deletes via `IFileOperation`, CF_HDROP clipboard interop
  (cut/copy/paste round-trips with Explorer), native OLE drag & drop.
- **Background transfer queue**: queued copy/move/delete with progress,
  speed/ETA, pause/resume/cancel, conflict resolution (overwrite / keep both /
  skip / apply-to-all) and undo (Ctrl+Z).
- **Power features**: flatten branch view, wildcard filter box, folder sizes,
  batch rename (patterns, regex, counters, case transforms, live preview),
  colored labels + tags + comments (SQLite sidecar), color filters by wildcard,
  favorites, quick-jump palette (Ctrl+P), zip archives browsable as folders,
  preview pane (images, text, hex, audio tags/playback, PDF/HTML/video),
  status-bar totals (selection size, volume free space), SHA-256 column.
- **WASM plugins**: sandboxed wasmtime plugins with capability-based
  permissions (a plugin sees nothing unless you grant it). Plugin kinds:
  commands, custom columns. Two reference plugins included.

## Building

```powershell
./build-debug.ps1           # fast debug build → target/debug/simplecommander.exe
./build-debug.ps1 -Run      # build and launch
./build-release.ps1         # optimized build → target/release/simplecommander.exe
./build-release.ps1 -Run    # build and launch
```

### Plugins

```powershell
./build-plugins.ps1     # builds plugins/dist/*.wasm
```

### Distribution

```powershell
./package-portable.ps1  # dist/SimpleCommander-portable.zip (exe + plugins + docs)

# MSI installer (requires the WiX v3 toolset):
cargo install cargo-wix
cargo wix -p sc-app --include wix/main.wxs
```

Install via **Tools → Plugin manager → Install plugin**, then grant the
`read-files` permission. Reference plugins:

- `image_dimensions.wasm` — adds a "Dimensions" column for images
- `crc32_command.wasm` — "CRC32 of selection" command in Tools / context menu

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| F6 | Switch pane |
| Ctrl+T / Ctrl+W | New / close tab |
| Ctrl+L | Focus address bar |
| Ctrl+F | Search (everywhere / this folder / content) |
| Ctrl+P | Quick-jump palette |
| Ctrl+C/X/V | Explorer-compatible clipboard |
| Ctrl+Shift+C | Copy full path(s) of the selection |
| Ctrl+Alt+C / Ctrl+Shift+M | Copy / move to other pane |
| Space | Toggle preview pane |
| F1 | Open configured terminal in the current folder |
| F2 | Rename (inline) |
| F5 / Ctrl+R | Refresh |
| F7 | New folder |
| Del / Shift+Del | Recycle / delete permanently |
| Ctrl+Z | Undo file operation |
| Ctrl+H | Toggle hidden files |
| Backspace / Alt+←/→ | Up / history back / forward |
| ← / → | Parent folder / enter folder |
| Middle-click folder | Open in a new tab |
| Shift+right-click | Windows Explorer context menu |

## Architecture

```
crates/
  sc-core     domain model: entries, snapshots, natural sort, pane/tab state
  sc-shell    Win32/COM: enumeration, icons, watchers, context menu, clipboard, drag&drop
  sc-index    NTFS MFT index + USN tailing; fallback walker; content search
  sc-ops      background operation queue: copy/move/delete, conflicts, undo
  sc-plugins  wasmtime host, capability-gated plugin ABI
  sc-app      egui application: panes, dialogs, theming
  sc-bench    criterion benchmarks + synthetic tree generators
plugins/      guest-side reference plugins (wasm32-unknown-unknown)
```

Performance budgets (enforced by `sc-bench`): cold start < 300 ms, 100k-entry
directory < 500 ms, idle RAM < 80 MB, search results < 50 ms via MFT index.

## Notes

- The MFT index requires running elevated; without elevation a background
  fallback indexer covers your user profile.
- Settings are portable (`settings.toml` next to the exe when writable,
  `%APPDATA%\SimpleCommander` otherwise).
