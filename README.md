# React Graph

A fast desktop app that maps a React codebase as an interactive graph of **components**, **hooks** and **functions**, with a Project panel inspired by JetBrains IDEs. It runs on **Windows** and **macOS (Apple Silicon)**.

Open a folder of `.ts` / `.tsx` / `.js` / `.jsx` files and React Graph parses it with tree-sitter (it never runs your code). It shows what renders, calls and passes what, starting from your entry points.

![React Graph showing a sample project](docs/screenshot.png)

## Features

- **Graph view.** Components, hooks and functions are shown as cards, with edges for render, call and prop relations. Expand or collapse nodes, use **Dim others** to focus on one, zoom, and pan with the minimap.
- **Project panel.** A file tree with symbol counts. Search by typing anywhere, and use the tabs to show only components, hooks or functions (Ctrl+click, or ⌘-click on a Mac, combines kinds).
- **Code-health filters.** Each filter shows how many symbols match:
  - **Duplicates:** similar code exists elsewhere (structural fingerprints, so renamed copies still match)
  - **Identical:** an exact structural twin exists, where only names or literals differ
  - **Warnings:** hook dependency problems
  - **Unused:** nothing renders, calls or passes it
  - **Memoized:** wrapped in `memo`, `useCallback` or `useMemo`
- **Compare.** Shows two similar symbols side by side, with the differing tokens highlighted.
- **Play flow.** Animates a trace from any symbol, either level by level ("waves") or in call-and-return order, and can also show it as a sequence diagram.
- **Source view.** Shows the code for the selected symbol or file.
- **Light and dark themes**, and a list of recently opened projects.

## Install

Download the latest build from the [Releases page](https://github.com/dalladirosa/react-graph/releases/latest).

### Windows

| File | What it is |
| --- | --- |
| `react-graph-<version>-setup.exe` | Installer. Adds a Start Menu entry (and an optional desktop shortcut) and an uninstaller. It installs for your user only, without admin rights, unless you choose "all users". |
| `react-graph-<version>-windows-x64-portable.zip` | Portable version: a single `react-graph.exe` you can run from anywhere. |

The builds aren't code-signed yet, so Windows SmartScreen may warn the first time. Click **More info → Run anyway**.

### macOS (Apple Silicon: M1 or later)

Download `react-graph-<version>-macos-arm64.dmg`, open it, and drag **React Graph** onto **Applications**.

The app isn't notarized by Apple yet, so macOS blocks it the first time ("Apple could not verify…"). To allow it once:

1. Try to open React Graph, then click **Done** in the warning.
2. Open **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** next to React Graph.

Or run this once in Terminal instead:

```bash
xattr -dr com.apple.quarantine "/Applications/React Graph.app"
```

## Usage

Open a folder from the project menu in the title bar (Ctrl+O, or ⌘O on a Mac). You can also pass a folder on the command line:

```bash
react-graph.exe C:\path\to\my-app                                        # Windows
"/Applications/React Graph.app/Contents/MacOS/react-graph" ~/code/my-app   # macOS
```

`node_modules` and other build folders are skipped. Settings (theme and recent projects) are stored in `%APPDATA%\react-graph\settings.txt` on Windows and `~/.config/react-graph/settings.txt` on macOS.

## Build from source

You need [Rust](https://rustup.rs/) (stable, 2024 edition), plus the MSVC build tools on Windows or the Xcode Command Line Tools on macOS.

```bash
cargo run --release -- path/to/my-app
```

Run the tests with `cargo test`. The fixtures in `tests/fixtures` are small React projects.

To build the packages locally:

```bash
cargo build --release
# Windows, with Inno Setup 6 (https://jrsoftware.org/isinfo.php) installed:
iscc /DAppVersion=0.2.0 installer/react-graph.iss   # writes dist/react-graph-0.2.0-setup.exe
# macOS:
installer/macos/bundle.sh 0.2.0                     # writes dist/React Graph.app and dist/react-graph-0.2.0-macos-arm64.dmg
```

## Releasing

Releases are built by [`.github/workflows/release.yml`](.github/workflows/release.yml):

1. Bump `version` in `Cargo.toml` (for example `0.2.1`) and commit.
2. Tag the commit and push the tag:
   ```bash
   git tag v0.2.1
   git push origin v0.2.1
   ```
3. The workflow builds the Windows installer and portable zip, and the macOS `.dmg` on an Apple Silicon machine. It then publishes them together in one GitHub Release with automatic release notes.

The tag must match the `Cargo.toml` version, or the workflow stops. You can also start the workflow by hand from the **Actions** tab. That builds everything and attaches it to the run, along with a screenshot of the Mac app starting up, but doesn't publish a release.

## Credits

The macOS icons come from [Lucide](https://lucide.dev) (ISC license; see [`assets/icons/LICENSE`](assets/icons/LICENSE)).
