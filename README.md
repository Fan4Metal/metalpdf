<p align="center">
  <img src="images/icon.png" width="128" height="128" alt="MetalPDF icon">
</p>

<h1 align="center">MetalPDF</h1>

<p align="center">Fast PDF viewer for Windows</p>

<p align="center">
  <a href="https://github.com/Fan4Metal/metalpdf/releases/latest"><img src="https://img.shields.io/github/v/release/Fan4Metal/metalpdf?label=release" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/Windows-10%2B-0078D6" alt="Windows 10 or later">
  <img src="https://img.shields.io/badge/Rust-B7410E?logo=rust&logoColor=white" alt="Rust">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center"><b>English</b> | <a href="README.ru.md">Русский</a></p>

MetalPDF is a PDF viewer built around one idea: a document should be on screen the moment its window appears, and leafing through it should never show a blank page. It renders with PDFium, the PDF engine of Chromium, opens documents in tabs that can be pulled out into windows of their own, searches, selects text, shows thumbnails and the outline, prints, and can change the text of a line and save the result. There is no settings dialog and no network access beyond an optional update check.

![MetalPDF showing a textbook with the thumbnails sidebar open](images/screenshot.png)

*Screenshot of version 0.1.0.*

## Features

- **Instant**: the first page is rendered while the window is still being created; a 450 MB textbook of 1,300 pages opens in under 10 ms, and its first page is on screen about 160 ms after the program starts (see [Performance](#performance)).
- **Never blank**: pages scrolled to are drawn from the nearest render at hand or a small preview until the exact one arrives; renders nobody waits for any more are dropped mid-way.
- **Tabs and windows**: several documents in one window, a tab dragged out becomes a window; a second launch hands its files to the running copy; the last session is opened again (see [Tabs and windows](#tabs-and-windows)).
- **Continuous or single page**, fit width, fit page or any zoom, quarter turns, the keyboard of a viewer (see [Keyboard shortcuts](#keyboard-shortcuts)).
- **Sidebar** with page thumbnails and the document's outline, both following the page being read.
- **Find** across the document, with the hits highlighted on the pages and listed in the sidebar; **text selection** by dragging, copied with Ctrl+C.
- **Links** inside the document and to the web, with a history for going back.
- **Printed page numbers**: a book whose page 1 is the file's 17th is navigated and printed by the numbers on its pages (see [Page numbers](#page-numbers)).
- **Editing** (experimental): the text of a line changed in place, pictures moved, resized, replaced or removed, pages inserted, replaced or deleted, and the document saved (see [Editing text, pictures and pages](#editing-text-pictures-and-pages)).
- **Printing** with a preview of each sheet: the printer, its paper, orientation and sides, the pages, copies and scale, the printer's own settings behind Properties (see [Printing](#printing)).
- **Explorer thumbnails**: with MetalPDF the default program for PDF files, Explorer shows the files' first pages instead of icons.
- **Forms** are drawn with their field contents; **encrypted** documents ask for their password.
- **Light, dark or system theme**; the dark theme grey or near black.
- **English or Russian interface**: it follows the language of Windows and can be changed in the About window (`F1`).
- **Portable**: the [portable archive](#installation) keeps its settings next to the program.
- **Privacy**: no network access beyond the optional update check (see [Update check and privacy](#update-check-and-privacy)).

## Installation

The installer and the portable archive are on the [Releases](https://github.com/Fan4Metal/metalpdf/releases) page. Both ship `pdfium.dll`, the rendering engine, next to the program; the program starts without it and says what is missing. The executables are not code-signed, so Windows SmartScreen may warn about an unknown publisher on the first start ("More info" → "Run anyway").

- **Installer**, `MetalPDF_<version>_Setup.exe`: needs no administrator rights; the program is placed in `%LOCALAPPDATA%\Programs\MetalPDF`, keeps its settings in `%APPDATA%\MetalPDF\config\settings.json`, and is offered for PDF files in "Open with" and in Settings → Default apps (it never makes itself the default).
- **Portable archive**, `MetalPDF_<version>_portable.zip`: the program in a `MetalPDF` folder, run without installation; the settings are kept next to it, so it can be carried on a removable drive.

<details>
<summary>Explorer thumbnails, and where the portable program keeps its settings</summary>

- **Thumbnails**: while MetalPDF is the default program for PDF files, Explorer shows the files' first pages as their thumbnails instead of icons: `metalpdf_thumbs.dll`, part of the installation, draws them with the same PDFium engine. A file thumbnailed before keeps the picture Explorer cached until the thumbnail cache is cleared (Disk Cleanup, Thumbnails) or the file changes.
- **Portable settings**: the archive holds an empty `metalpdf.json`. While that file is next to `metalpdf.exe` and the folder is writable, the settings are kept in it. Without the file, the program keeps them in `%APPDATA%\MetalPDF`, as the installed program does. The About window shows where the settings are kept. A settings file that cannot be read is kept beside it as `.json.bad`, and the defaults are used.

</details>

## Usage

A PDF is opened with `Ctrl+O`, by dropping it on the window, from the recent files on the start page, from Explorer (double click once MetalPDF is chosen for PDF files, or "Open with"), or from the command line: `metalpdf.exe file.pdf`. Each file opens in a tab; a file already open is brought to the front instead. Where each document was left (page, zoom, turns) is remembered, as is the last session: started without files, MetalPDF opens the tabs it was closed with.

<details>
<summary>The toolbar and the tab bar</summary>

The toolbar under the tabs (not shown on the start page) holds:

- the page field: a page number or a label such as `iv` typed there goes to that page; `Ctrl+G` focuses it;
- the zoom with its menu, and the three zoom presets: actual size, fit width, fit page (keys `1`, `2`, `3`); the one in force is lit, and a preset chosen is the zoom new documents open at;
- the two turn buttons;
- the view mode: continuous pages, or one page at a time, where the scroll bar still spans the document and dragging it turns the pages;
- the text tool and the hand, the pencil of the editing mode and the printer, which opens the print dialog;
- on the right, a note that says when an edit was refused or a file saved, and the progress of a print job with a cross to cancel it.

The tab bar holds the sidebar toggle, Open, the tabs, and on the right the theme switch (its context menu chooses between a grey and a near-black dark theme), the update button when a newer version exists, and About.

</details>

### Tabs and windows

Tabs are switched with `Ctrl+Tab` and `Ctrl+Shift+Tab`, closed with `Ctrl+W` or the cross, reopened with `Ctrl+Shift+T`, and reordered by dragging. A tab dragged out of the bar becomes a window of its own; dragged onto another window of MetalPDF, it moves there. The tab's context menu offers the same, and "Close others".

<details>
<summary>One copy per session, and the command line</summary>

One copy of MetalPDF runs per user session: a second launch (a double click in Explorer, the command line) hands its files to the running copy, which opens them in the window used last and comes to the front. The command line options:

| Option | What it does |
|---|---|
| `--new-instance file.pdf` | Starts a copy of its own instead of handing the files over |
| `--register`, `--unregister` | Offer MetalPDF for `.pdf` files in Windows' "Open with", and withdraw it (the installer runs them) |
| `--bench file.pdf` | Prints the engine's timings for a file |
| `--screenshot file.png file.pdf` | Writes the window's picture after a few frames and exits |
| `--export-icon file.ico` | Writes the application icon |
| `--version`, `--help` | The version, the options |

</details>

### Sidebar

`F4` (or the button left of Open) shows the sidebar with its lists: **Pages**, **Outline**, and **Search** while the find bar is open. It stands on the left or on the right of the window, as chosen in the context menu of that button or of the lists' tabs; its width is kept.

<details>
<summary>The three lists</summary>

- **Pages** shows every page as a thumbnail, the current one framed and kept in view; a click goes to the page. The thumbnails' context menu inserts, replaces and deletes pages (see [Editing](#editing-text-pictures-and-pages)).
- **Outline** shows the document's bookmarks as a tree, the levels set apart by size, weight and indent with guides down them, each with its page number; the chapters are folded but the one the reader is in, whose bookmark is highlighted; a click follows a bookmark, a click on its triangle folds it.
- **Search** lists the hits of a search by page, each on the line it is on with the match highlighted as on the page and the current one marked; a click goes to it.

</details>

### Find

`Ctrl+F` opens the find bar. The search starts from the page being read and goes round the document, page by page; the hits are highlighted on the pages, the current one brighter, and the bar says which hit of how many. `Enter` and `F3` go to the next hit, `Shift+F3` to the previous; `Esc` closes the bar.

<details>
<summary>Options and the list of hits</summary>

- `Aa` and <u>ab</u> on the bar match the case and whole words only. Case is folded by Unicode's rules (Cyrillic as well as Latin), a space in the query matches a line break, and whole words are told apart in any script.
- The count shows a plus while the search goes on.
- A search opens the sidebar on its **Search** tab, which lists the hits by page; closing the search puts the sidebar back as it was.

</details>

### Text selection, links and the context menu

Text is selected by dragging over it, across pages too, and copied with `Ctrl+C`; `Ctrl+A` selects the current page's text, `Esc` clears the selection, and a double click selects a word. The pointer is an I-beam over text and a hand over a link; a link inside the document goes to its target, a web link opens in the browser. `Alt+Left` (or the mouse's back button) returns from a jump made through a link, a bookmark, a thumbnail, the page field or a hit; `Alt+Right` goes again.

<details>
<summary>The context menu and the two tools</summary>

- **The right button over the pages** opens a menu: copy the selection, select the page, copy the picture under the pointer (to the clipboard, at its own resolution, as shown: masks and transparency applied), open a link or copy its address, the tools, and in the editing mode the text run or the picture under the pointer.
- **The tools** are on the toolbar: with the text tool (`V`) dragging selects text; with the hand (`H`) dragging scrolls the pages, which coast on when let go, and in single-page mode a drag pulled well past the page's edge turns the page, either way. A double click selects the word under the pointer with either.

</details>

### Page numbers

When a document's printed page numbers differ from the file's (a book whose page 1 is the file's 17th page, say), the right button on the page field offers to read the printed numbers off the pages, to state the number of the page shown, or to go back to the file's numbering.

<details>
<summary>How the printed numbers are used</summary>

To read the numbers, two dozen pages are sampled and the shift they agree on is taken. The page field, the thumbnails, the outline and the search list then show the printed numbers (the pages before page 1 in roman numerals, as a book's front matter), a number typed in the field is the printed one, the print dialog reads its page list by them, and the setting is kept with the document's position. A document with page labels of its own (such as roman numerals for a preface) shows those as it is.

</details>

### Editing text, pictures and pages

The editing mode (`E`, or the pencil on the toolbar) outlines the lines of text and the pictures on the pages. A click on a line opens a field over it with the whole line; `Enter` applies the change, `Esc` leaves the line as it was, and `Ctrl+Z` takes the last change back. A click on a picture selects it to be moved, resized, replaced or deleted. Pages are inserted, replaced and deleted from the thumbnails' context menu. The changes live in a copy of the document until `Ctrl+S` saves it. This part of the program is experimental; a copy of a valued document is advised.

<details>
<summary>Lines of text</summary>

A PDF often writes a line as many text objects, cut wherever a pair of letters is kerned; the objects on one baseline, one after another, are edited as one line. Only the objects the change touches are merged into one; the rest of the line keeps its objects as they were, and the part after the change moves along the line by the change in width, so that a longer text has its room and nothing overlaps. Nothing is reflowed: a line made longer reaches further to the right.

</details>

<details>
<summary>Fonts</summary>

A run keeps its font, and so its look, whenever that font can show the new text; a font embedded as a subset of its glyphs can for the characters it has (the digits of a date, say). Only when it lacks a glyph is the run set in a stand-in font, at the same place, size and colour: the same font when it is installed on the computer (it is looked up by the name the document gives, among the fonts of Windows and of the user), else Arial, Times New Roman or Courier New after the look of the original, in its weight and slant, so that a bold run stays bold; and fitted to the width of the original run, so that an edit of the same length fills the same box. A run under the pointer shows its font and size, and whether that font is installed: when it is not, installing it makes the replacement exact.

</details>

<details>
<summary>Pictures and pages</summary>

- **Pictures**: a click selects one; it is moved by dragging and resized by its corner handles (the proportions kept), **Replace…** puts a JPEG or PNG file in its place, fitted into the old box with its own proportions, and **Delete** (or the `Del` key) removes it.
- **Pages**, from the context menu of a thumbnail in the sidebar: a blank page (the size of its neighbour) or every page of another PDF file inserted before or after it, the page replaced by the pages of another PDF, or the page deleted (the last one stays). These need no editing mode.

</details>

<details>
<summary>Saving, and what is not edited</summary>

- **Saving**: `Ctrl+S` opens a Save As dialog offering `name_edited.pdf`, so the original is replaced only when chosen; the tab then shows the file saved. Closing a tab or quitting with unsaved changes asks first.
- **Signed and encrypted documents** are not edited.
- **Tagged (accessible) documents**: an edited text run keeps the marks its tags point to, also when it is set in a stand-in font; a picture removed leaves its tag empty, and the tag structure is not updated for the pages put in or removed.
- **Form XObjects**: text and pictures inside them, which some generators put every page in, are not reachable.
- **Pages PDFium cannot write back as they are** take no edit: text in a Type 3 font (how Chrome and Edge print variable fonts such as Cascadia, the code in a web page), gradients drawn as shadings and inline images would be lost on saving, and fills with a pattern, soft masks and two different fonts sharing one name would come out wrong, so such an edit is refused: the page is compared with its content as written before the edit is taken. Colours in CMYK, grey or spot inks are kept as the colour shown.
- An edit refused is explained in the note on the toolbar's right.

</details>

### Printing

`Ctrl+P`, or Print in the context menu of the pages, opens the print dialog. On the left it shows each sheet as it will print, the arrows under it stepping through the sheets; on the right are the printer, its paper, orientation and sides, the pages, copies and scale. Printing goes on in the background, a page at a time, with its progress and a cross to cancel it on the right of the toolbar.

<details>
<summary>The print dialog in detail</summary>

- **The preview** shows the paper, the part of it the printer reaches, and the page placed on it.
- **The printer** is the one last used, else the default one of Windows; paper, orientation and two-sided printing are offered when the printer has them, and **Properties…** opens the printer's own dialog for the rest (quality, colour, trays).
- **The pages** are all of them, the one shown or a list such as `1-5, 8, 11-`, read as the page field reads it, so a book numbered by its printed numbers is printed by them too. Copies are collated when asked.
- **The scale**: a page larger than the printable area is shrunk to it by default; it may instead be fitted to the sheet, printed at its actual size or at a custom scale (10–800 %, centred and cut off where the sheet ends), and a document that asks not to be scaled starts at its actual size. A landscape page on a portrait sheet is turned, and the reverse, unless this is switched off.
- **Vectors or pictures**: the pages go to the printer as vectors, drawn by PDFium, with form fields as filled in. **Print as an image** sends each page as a picture of 300 dpi instead, for a printer that prints the vector output wrong; it is slower and larger.
- A document with unsaved edits is printed as shown; quitting while a job goes on asks first. Text printed to a PDF printer comes out as outlines, not as text that can be selected.

</details>

## Keyboard shortcuts

<details open>
<summary>Pages and zoom</summary>

| Keys | Action |
|---|---|
| `Page Down`, `Page Up`, `Space`, `Shift+Space` | A screen down or up (in single-page mode, the next or previous page at the end) |
| Mouse wheel | Scrolling; in single-page mode a notch turns a page (a page taller than the view is scrolled through first) |
| `Ctrl+Page Down`, `Ctrl+Page Up`, `N`, `P` | Next, previous page |
| `Home`, `End` | First, last page |
| `←` `↑` `→` `↓` | Scroll (in single-page mode, `←` `→` turn pages) |
| `Ctrl+G` | The page field |
| `1`, `2`, `3` | 100 %, fit width, fit page |
| `4`, `5` | Continuous pages, one page at a time |
| `Ctrl++`, `Ctrl+−`, `Ctrl+wheel` | Zoom in, zoom out (round the pointer with the wheel) |
| `R`, `Shift+R` | Turn the pages right, left |
| `Alt+←`, `Alt+→` | Back, forward through the jumps |

</details>

<details>
<summary>Find, selection and tools</summary>

| Keys | Action |
|---|---|
| `Ctrl+F`, `F3`, `Shift+F3`, `Esc` | Find, next hit, previous hit, close |
| `Ctrl+C`, `Ctrl+A`, `Esc` | Copy the selection, select the page, clear |
| `V`, `H` | The text tool (dragging selects), the hand (dragging scrolls) |
| `F4` | Sidebar |
| `E`, `Ctrl+Z`, `Ctrl+S`, `Del` | Editing mode, undo an edit, save as, remove the selected picture |

</details>

<details>
<summary>Files, tabs and program</summary>

| Keys | Action |
|---|---|
| `Ctrl+O` | Open a file |
| `Ctrl+P` | Print |
| `Ctrl+W`, `Ctrl+Shift+T` | Close the tab, reopen the last closed tab |
| `Ctrl+Tab`, `Ctrl+Shift+Tab` | Next, previous tab |
| `F11` | Full screen |
| `F1` | About (version, language, update check, licenses) |

</details>

## Performance

The first page is on screen about 160 ms after the program starts: the engine thread opens the first file and renders its first page while the window is being created, at the size predicted from the saved window, so the first frame finds the page ready.

<details>
<summary>Measurements</summary>

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

The first run of a machine is slower, as the DLL comes from disk.

</details>

## Update check and privacy

MetalPDF makes no network connection of its own, with one exception: the update check, **off by default**. When enabled in the About window (`F1`), the program asks `api.github.com` once a day at start-up, and on "Check now", for the latest release of this repository; the request carries the program's name and version as its user agent, nothing else, and nothing is downloaded. A newer version is shown as a button in the tab bar that opens the release page in the browser. The documents opened, the recent files and the settings never leave the computer.

## Building

Requirements: Windows 10 or later, Rust (stable, MSVC), the Visual Studio 2022 Build Tools with the C++ workload, Python 3 and [uv](https://docs.astral.sh/uv/); Inno Setup 6 for the installer.

```
uv run build.py          # target\release\metalpdf.exe with pdfium.dll next to it
cargo test
```

<details>
<summary>How the build works</summary>

The script closes a `metalpdf.exe` running from `target\`, fetches the pinned `pdfium.dll` into `vendor\pdfium` if it is missing (`tools\get_pdfium.py`, checked by SHA-256) and runs `cargo build --release`; the DLL is copied next to `target\release\metalpdf.exe` by the build, beside `metalpdf_thumbs.dll`, the Explorer thumbnail provider built with it. `cargo test` runs the unit tests; `cargo test pdfium -- --ignored --test-threads=1` also exercises the DLL (a PDF written by the test is opened, rendered, read, edited and saved). `metalpdf.exe --bench file.pdf` prints the engine's timings for a file. The executable links the C runtime statically and loads PDFium at run time, so it needs nothing beyond Windows and `pdfium.dll`.

</details>

<details>
<summary>Setting up the environment and the source layout</summary>

The tools can be installed with winget:

```
winget install Rustlang.Rustup Microsoft.VisualStudio.2022.BuildTools astral-sh.uv Python.Python.3.13 JRSoftware.InnoSetup
rustup default stable-msvc
```

The Build Tools need the "Desktop development with C++" workload, chosen in the Visual Studio Installer.

| Path | Contents |
|---|---|
| `src/pdfium/` | The PDFium C API declared by hand (`sys.rs`) and a safe layer over it (`mod.rs`): documents, pages, rendering (plain and progressive), text, search, links, bookmarks, editing, saving |
| `src/engine.rs` | The one thread PDFium is called from, with the scheduler of renders |
| `src/layout.rs`, `src/view.rs` | The geometry of the pages and the state of a view |
| `src/app.rs` | The windows and tabs |
| `src/ui/` | The page area, the tab bar, the toolbar, the sidebar, the print dialog, About, the Licenses window, the painted icons |
| `src/settings.rs`, `src/single_instance.rs` | The JSON settings; the hand-over between launches |
| `thumbs/` | `metalpdf_thumbs.dll`, the Explorer thumbnail provider |
| `tools/` | `get_pdfium.py`, `make_release.py`, `notices.py`, `setup.iss` |

</details>

## Installer and release

The release script builds the executable, the installer and the portable archive. It needs [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```
python tools\make_release.py                        # PDFium, tests, release build, installer, archive
python tools\make_release.py --no-tests --install   # the same, then a silent installation over the installed copy
```

<details>
<summary>The release procedure</summary>

The script writes `dist\MetalPDF_<version>_Setup.exe` and `_portable.zip` and takes the version from `Cargo.toml`. The installer script is `tools\setup.iss`; `metalpdf.exe --export-icon <file.ico>` writes the installer icon.

Releases on GitHub are built by the **Release** workflow (`.github/workflows/release.yml`). Between releases the version in `Cargo.toml` carries `-dev`. A release drops the suffix (after `python tools/notices.py` if `Cargo.lock` has changed: a test checks the notices), is committed as `Version X.Y.Z`, and pushing a matching tag publishes it with the installer and the portable archive; the next version with `-dev` follows:

```
git tag v0.1.0
git push origin v0.1.0
```

The workflow stops if the tag does not match the version in `Cargo.toml`.

</details>

## Built with

- [PDFium](https://pdfium.googlesource.com/pdfium/): the PDF engine of Chromium, as built by [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries);
- [egui / eframe](https://github.com/emilk/egui): the user interface and the window;
- [interprocess](https://github.com/kotauskas/interprocess): the pipe between launches;
- [rfd](https://github.com/PolyMeilex/rfd): the file dialogs;
- [serde](https://serde.rs): the settings;
- [png](https://github.com/image-rs/image-png): the PNG pictures put in place of images;
- [Inno Setup](https://jrsoftware.org/isinfo.php): the installer.

## License

MIT, see [LICENSE](LICENSE). PDFium and the libraries built into `pdfium.dll` are under the BSD 3-Clause and other permissive licenses, the Rust crates and fonts compiled into `metalpdf.exe` under permissive licenses as well: see [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md), [licenses/PDFium-LICENSE.txt](licenses/PDFium-LICENSE.txt) and [licenses/THIRD_PARTY_NOTICES.txt](licenses/THIRD_PARTY_NOTICES.txt). The program shows all of these texts in its About window (**Licenses…**), and the installer and the portable archive include them as files.
