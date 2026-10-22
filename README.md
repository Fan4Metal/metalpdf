<p align="center">
  <img src="images/icon.png" width="128" height="128" alt="MetalPDF icon">
</p>

<h1 align="center">MetalPDF</h1>

<p align="center">Fast PDF viewer for Windows</p>

<p align="center">
  <a href="https://github.com/Fan4Metal/metalpdf/releases/latest"><img src="https://img.shields.io/github/v/release/Fan4Metal/metalpdf?label=release" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/Windows-10%2B-0078D6" alt="Windows 10 or later">
  <img src="https://img.shields.io/badge/Rust-B7410E?logo=rust&logoColor=white" alt="Rust">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center"><b>English</b> | <a href="README.ru.md">Русский</a></p>

MetalPDF is a PDF viewer built around one idea: a document should be on screen the moment its window appears, and leafing through it should never show a blank page. It renders with PDFium, the PDF engine of Chromium, opens documents in tabs that can be pulled out into windows of their own, searches, selects text, shows thumbnails and the outline, and can change the text of a line and save the result. There is no settings dialog and no network access beyond an optional update check.

![MetalPDF showing a textbook with the thumbnails sidebar open](images/screenshot.png)

*Screenshot of version 0.1.0.*

## Features

- **Instant**: the first page is rendered while the window is still being created; a 450 MB textbook of 1,300 pages opens in under 10 ms, and its first page is on screen about 160 ms after the program starts (see [Performance](#performance)).
- **Never blank**: pages scrolled to are drawn from the nearest render at hand or a small preview until the exact one arrives; renders nobody waits for any more are dropped mid-way.
- **Tabs and windows**: several documents in one window, a tab dragged out becomes a window; a second launch hands its files to the running copy; the last session is opened again (see [Tabs and windows](#tabs-and-windows)).
- **Continuous or single page**, fit width, fit page or any zoom, quarter turns, the keyboard of a viewer (see [Keyboard shortcuts](#keyboard-shortcuts)).
- **Sidebar** with page thumbnails and the document's outline, both following the page being read.
- **Find** across the document, with the hits highlighted on the pages; **text selection** by dragging, copied with Ctrl+C.
- **Links** inside the document and to the web, with a history for going back.
- **Editing** (experimental): the text of a line changed in place, pictures moved, resized, replaced or removed, pages inserted, replaced or deleted, and the document saved (see [Editing text, pictures and pages](#editing-text-pictures-and-pages)).
- **Forms** are drawn with their field contents; **encrypted** documents ask for their password.
- **Light, dark or system theme.**
- **English or Russian interface**: it follows the language of Windows and can be changed in the About window (`F1`).
- **Portable**: the [portable archive](#installation) keeps its settings next to the program.
- **Privacy**: no network access beyond the optional update check (see [Update check and privacy](#update-check-and-privacy)).

## Installation

The installer and the portable archive are on the [Releases](https://github.com/Fan4Metal/metalpdf/releases) page.

The installer, `MetalPDF_<version>_Setup.exe`, needs no administrator rights: the program is placed in `%LOCALAPPDATA%\Programs\MetalPDF` and offered for PDF files in "Open with" and in Settings → Default apps (it never makes itself the default). The installed program keeps its settings in `%APPDATA%\MetalPDF\config\settings.json`.

The portable archive, `MetalPDF_<version>_portable.zip`, holds the program in a `MetalPDF` folder and runs without installation. It also holds an empty `metalpdf.json`. While that file is next to `metalpdf.exe` and the folder is writable, the settings are kept in it, so the program can be carried on a removable drive. Without the file, the program keeps them in `%APPDATA%\MetalPDF`, as the installed program does. The About window shows where the settings are kept.

Both ship `pdfium.dll`, the rendering engine, next to the program; the program starts without it and says what is missing.

Windows SmartScreen may warn about an unsigned program on the first start: "More info" → "Run anyway".

## Usage

A PDF is opened with `Ctrl+O`, by dropping it on the window, from the recent files on the start page, from Explorer (double click once MetalPDF is chosen for PDF files, or "Open with"), or from the command line: `metalpdf.exe file.pdf`. Each file opens in a tab; a file already open is brought to the front instead.

The status bar holds the page field (a page number or a label such as `iv` typed there goes to that page; `Ctrl+G` focuses it), the zoom with its menu, the two turn buttons, the view mode (continuous pages or one page at a time) and the pencil of the editing mode. The tab bar holds Open, the sidebar toggle, the tabs, and on the right the theme switch, the update button when a newer version exists, and About.

Where each document was left (page, zoom, turns) is remembered, as is the last session: started without files, MetalPDF opens the tabs it was closed with.

### Tabs and windows

Tabs are switched with `Ctrl+Tab` and `Ctrl+Shift+Tab`, closed with `Ctrl+W` or the cross, reopened with `Ctrl+Shift+T`, and reordered by dragging. A tab dragged out of the bar becomes a window of its own; dragged onto another window of MetalPDF, it moves there. The tab's context menu offers the same, and "Close others".

One copy of MetalPDF runs per user session: a second launch (a double click in Explorer, the command line) hands its files to the running copy, which opens them in the window used last and comes to the front. `metalpdf.exe --new-instance file.pdf` starts a copy of its own instead.

### Sidebar

`F4` (or the button beside Open) shows the sidebar with two lists. **Pages** shows every page as a thumbnail, the current one framed and kept in view; a click goes to the page. **Outline** shows the document's bookmarks as a tree, the one the reader is in highlighted; a click follows it. The sidebar's width is kept.

### Find

`Ctrl+F` opens the find bar. The search starts from the page being read and goes round the document, page by page; the hits are highlighted on the pages, the current one brighter, and the bar says which hit of how many (with a plus while the search goes on). `Enter` and `F3` go to the next hit, `Shift+F3` to the previous; `Aa` and <u>ab</u> match the case and whole words only; `Esc` closes the bar.

### Text selection and links

Text is selected by dragging over it, across pages too, and copied with `Ctrl+C`; `Ctrl+A` selects the current page's text, `Esc` clears the selection. The pointer is an I-beam over text and a hand over a link; a link inside the document goes to its target, a web link opens in the browser. `Alt+Left` (or the mouse's back button) returns from a jump made through a link, a bookmark, a thumbnail, the page field or a hit; `Alt+Right` goes again.

### Editing text, pictures and pages

The editing mode (`E`, or the pencil in the status bar) outlines the text objects of the pages, the runs of text a PDF is made of: usually a line or a part of one. A click on a run opens a field over it; `Enter` replaces the run's text, `Esc` leaves it as it was. `Ctrl+Z` takes the last change back. The pages keep their layout: a longer text runs past where the old one ended, nothing is reflowed.

A run keeps its font when that font can show the new text; a font embedded as a subset of its glyphs cannot, and the run is set in Arial instead, at the same place, size and colour.

Pictures are outlined too. A click selects one: it is moved by dragging and resized by its corner handles (the proportions kept), **Replace…** puts a JPEG or PNG file in its place, fitted into the old box with its own proportions, and **Delete** (or the `Del` key) removes it.

Pages are handled from the context menu of a thumbnail in the sidebar: a blank page (the size of its neighbour) or every page of another PDF file inserted before or after it, the page replaced by the pages of another PDF, or the page deleted (the last one stays). These need no editing mode.

The changes live in a copy of the document until `Ctrl+S` saves it (a Save As dialog, so the original is replaced only when chosen); closing a tab or quitting with unsaved changes asks first. Signed, tagged (accessible) and encrypted documents are not edited, and text and pictures inside form XObjects, which some generators put every page in, are not reachable. This part of the program is experimental; a copy of a valued document is advised.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| `Ctrl+O` | Open a file |
| `Ctrl+W`, `Ctrl+Shift+T` | Close the tab, reopen the last closed tab |
| `Ctrl+Tab`, `Ctrl+Shift+Tab` | Next, previous tab |
| `Page Down`, `Page Up`, `Space`, `Shift+Space` | A screen down or up (in single-page mode, the next or previous page at the end) |
| `Ctrl+Page Down`, `Ctrl+Page Up`, `N`, `P` | Next, previous page |
| `Home`, `End` | First, last page |
| `←` `↑` `→` `↓` | Scroll (in single-page mode, `←` `→` turn pages) |
| `Ctrl+G` | The page field |
| `Ctrl+0`, `Ctrl+1`, `Ctrl+2` | Fit width, 100 %, fit page |
| `Ctrl++`, `Ctrl+−`, `Ctrl+wheel` | Zoom in, zoom out (round the pointer with the wheel) |
| `R`, `Shift+R` | Turn the pages right, left |
| `F4` | Sidebar |
| `Ctrl+F`, `F3`, `Shift+F3`, `Esc` | Find, next hit, previous hit, close |
| `Ctrl+C`, `Ctrl+A`, `Esc` | Copy the selection, select the page, clear |
| `Alt+←`, `Alt+→` | Back, forward through the jumps |
| `E`, `Ctrl+Z`, `Ctrl+S`, `Del` | Editing mode, undo an edit, save as, remove the selected picture |
| `F11` | Full screen |
| `F1` | About (version, language, update check, licenses) |

## Performance

Measured on an i7-13700KF with an RTX 4070 Ti SUPER, a 4K display at 150 % scaling, with `metalpdf.exe --bench file.pdf` (the engine alone) and `METALPDF_TRACE=1` (the window):

| Step | Time |
|---|---|
| Loading and initialising `pdfium.dll` | 2 ms |
| Opening a 2-page file; a 454 MB textbook of 1,347 pages | 0.2 ms; 9 ms |
| Reading the sizes of all 1,347 pages | 16 ms |
| Rendering a text page at 1,626 px wide; a page of large images | 3–5 ms; 17 ms |
| A 200 px preview of a page | 0.1–3 ms |
| The first page on screen (uploaded to the GPU) after the program starts | 160 ms |
| The window shown | 210–250 ms |

The engine thread opens the first file and renders its first page while the window is being created, at the size predicted from the saved window, so the first frame finds the page ready. The first run of a machine is slower, as the DLL comes from disk.

## Update check and privacy

MetalPDF makes no network connection of its own, with one exception: the update check, **off by default**. When enabled in the About window (`F1`), the program asks `api.github.com` once a day at start-up, and on "Check now", for the latest release of this repository; the request carries the program's name and version as its user agent, nothing else, and nothing is downloaded. A newer version is shown as a button in the tab bar that opens the release page in the browser. The documents opened, the recent files and the settings never leave the computer.

## Building

Requirements: Rust (stable, MSVC), Visual Studio 2022 Build Tools (C++ workload), Python 3 and [uv](https://docs.astral.sh/uv/); Inno Setup 6 for the installer.

```
uv run build.py
```

The script closes a `metalpdf.exe` running from `target\`, fetches the pinned `pdfium.dll` into `vendor\pdfium` if it is missing (`tools\get_pdfium.py`, checked by SHA-256) and runs `cargo build --release`; the DLL is copied next to `target\release\metalpdf.exe` by the build. `cargo test` runs the unit tests; `cargo test pdfium -- --ignored --test-threads=1` also exercises the DLL (a PDF written by the test is opened, rendered, read, edited and saved). `metalpdf.exe --bench file.pdf` prints the engine's timings for a file.

<details>
<summary>Setting up the environment with winget</summary>

```
winget install Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools astral-sh.uv Python.Python.3.13 JRSoftware.InnoSetup
rustup default stable-msvc
```

The Build Tools need the "Desktop development with C++" workload, chosen in the Visual Studio Installer.

</details>

<details>
<summary>Layout of the sources</summary>

- `src/pdfium/` — the PDFium C API declared by hand (`sys.rs`) and a safe layer over it (`mod.rs`): documents, pages, rendering (plain and progressive), text, search, links, bookmarks, editing, saving.
- `src/engine.rs` — the one thread PDFium is called from, with the scheduler of renders.
- `src/layout.rs`, `src/view.rs` — the geometry of the pages and the state of a view.
- `src/app.rs` — the windows and tabs; `src/ui/` — the page area, the tab bar, the status bar, the sidebar, About, the Licenses window, the painted icons.
- `src/settings.rs` — the JSON settings; `src/single_instance.rs` — the hand-over between launches.
- `tools/` — `get_pdfium.py`, `make_release.py`, `notices.py`, `setup.iss`.

</details>

## Installer and release

```
python tools\make_release.py                        # tests, build, dist\MetalPDF_<version>_Setup.exe and _portable.zip
python tools\make_release.py --no-tests --install   # the same, then a silent installation over the installed copy
```

The GitHub workflow builds both files on a push of a tag `vX.Y.Z` (which must match the version in `Cargo.toml`) and publishes them as a release.

## Built with

- [PDFium](https://pdfium.googlesource.com/pdfium/) — the PDF engine of Chromium, as built by [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries).
- [egui / eframe](https://github.com/emilk/egui) — the user interface.
- [interprocess](https://github.com/kotauskas/interprocess) — the pipe between launches; [rfd](https://github.com/PolyMeilex/rfd) — the file dialogs; [serde](https://serde.rs) — the settings.

## License

MIT, see [LICENSE](LICENSE). PDFium and the libraries built into `pdfium.dll` are under the BSD 3-Clause and other permissive licenses, the Rust crates and fonts compiled into `metalpdf.exe` under permissive licenses as well: see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md), [licenses/PDFium-LICENSE.txt](licenses/PDFium-LICENSE.txt) and [licenses/THIRD_PARTY_NOTICES.txt](licenses/THIRD_PARTY_NOTICES.txt); the program shows them in About → Licenses….
