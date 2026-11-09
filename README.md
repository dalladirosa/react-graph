# React Graph

A fast desktop app that maps a React codebase as an interactive graph of **components**, **hooks** and **functions**, with a Project panel inspired by JetBrains IDEs.

Open a folder of `.ts` / `.tsx` / `.js` / `.jsx` files and React Graph parses it with tree-sitter (it never runs your code). It shows what renders, calls and passes what, starting from your entry points.

![React Graph showing a sample project](docs/screenshot.png)

## Features

- **Graph view.** Components, hooks and functions are shown as cards, with edges for render, call and prop relations. Expand or collapse nodes, use **Dim others** to focus on one, zoom, and pan with the minimap.
- **Project panel.** A file tree with symbol counts. Search by typing anywhere, and use the tabs to show only components, hooks or functions (Ctrl+click combines kinds).
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

## Install (Windows)

Download the latest build from the [Releases page](https://github.com/dalladirosa/react-graph/releases/latest):

| File | What it is |
| --- | --- |
| `react-graph-<version>-setup.exe` | Installer. Adds a Start Menu entry (and an optional desktop shortcut) and an uninstaller. It installs for your user only, without admin rights, unless you choose "all users". |
| `react-graph-<version>-windows-x64-portable.zip` | Portable version: a single `react-graph.exe` you can run from anywhere. |

The builds aren't code-signed yet, so Windows SmartScreen may warn the first time. Click **More info → Run anyway**.

## Usage

```bash
react-graph.exe                     # opens the most recent project, or a welcome screen
react-graph.exe C:\path\to\my-app   # opens a specific folder
```

`node_modules` and other build folders are skipped. Settings (theme and recent projects) are stored in `%APPDATA%\react-graph\settings.txt`.

## Build from source

You need [Rust](https://rustup.rs/) (stable, 2024 edition) and, on Windows, the MSVC build tools.

```bash
cargo run --release -- C:\path\to\my-app
```

Run the tests with `cargo test`. The fixtures in `tests/fixtures` are small React projects.

To build the installer locally, install [Inno Setup 6](https://jrsoftware.org/isinfo.php) and then run:

```bash
cargo build --release
iscc /DAppVersion=0.1.1 installer/react-graph.iss   # writes dist/react-graph-0.1.1-setup.exe
```

## Releasing

Releases are built by [`.github/workflows/release.yml`](.github/workflows/release.yml):

1. Bump `version` in `Cargo.toml` (for example `0.1.2`) and commit.
2. Tag the commit and push the tag:
   ```bash
   git tag v0.1.2
   git push origin v0.1.2
   ```
3. The workflow builds `react-graph.exe` on Windows, packages the installer and the portable zip, and publishes a GitHub Release with automatic release notes.

The tag must match the `Cargo.toml` version, or the workflow stops. You can also start the workflow by hand from the **Actions** tab. That builds everything and attaches it to the run, but doesn't publish a release.
