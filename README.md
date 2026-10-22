<p align="center">
  <img src="images/icon.png" width="128" height="128" alt="MetalPDF icon">
</p>

<h1 align="center">MetalPDF</h1>

<p align="center">Short description of the program for Windows</p>

<p align="center">
  <a href="https://github.com/Fan4Metal/metalpdf/releases/latest"><img src="https://img.shields.io/github/v/release/Fan4Metal/metalpdf?label=release" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/Windows-10%2B-0078D6" alt="Windows 10 or later">
  <img src="https://img.shields.io/badge/Rust-B7410E?logo=rust&logoColor=white" alt="Rust">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center"><b>English</b> | <a href="README.ru.md">Русский</a></p>

MetalPDF is a <one paragraph: what it does, for whom, what makes it different; no imperatives>.

![MetalPDF showing <what the screenshot shows>](images/screenshot.png)

*Screenshot of version 0.1.0.*

## Features

- **Fast**: <a measured number>.
- **<Feature>**: <one sentence> (see [Section](#section)).
- **Light, dark or system theme.**
- **English or Russian interface**: it follows the language of Windows and can be changed in the About window (`F1`).
- **Portable**: the [portable archive](#installation) keeps its settings next to the program.
- **Privacy**: no network access beyond the optional update check (see [Update check and privacy](#update-check-and-privacy)).

## Installation

The installer and the portable archive are on the [Releases](https://github.com/Fan4Metal/metalpdf/releases) page.

The installer, `MetalPDF_<version>_Setup.exe`, needs no administrator rights: the program is placed in `%LOCALAPPDATA%\Programs\MetalPDF`. The installed program keeps its settings in `%APPDATA%\MetalPDF`.

The portable archive, `MetalPDF_<version>_portable.zip`, holds the program in a `MetalPDF` folder and runs without installation. It also holds an empty `app.ron`. While that file is next to `metalpdf.exe` and the folder is writable, the settings are kept in it, so the program can be carried on a removable drive. Without the file, the program keeps them in `%APPDATA%\MetalPDF`, as the installed program does. The About window shows where the settings are kept.

The executables are not code-signed, so Windows SmartScreen may warn about an unknown publisher on first launch.

## Usage

<The main sections of the program: tables of settings, lists of commands.>

## Keyboard shortcuts

| Key | Action |
|---|---|
| `F1` | About |

## Update check and privacy

The update check is optional and off by default. With **Check for updates at start-up (once a day)** enabled in the About window, the program asks the GitHub Releases API (`api.github.com`) for the latest release at most once a day; **Check now** in the same window asks at once. The request carries nothing but the program version in its `User-Agent` header, and nothing is downloaded or installed: when a newer release exists, a button with its version appears beside the About button and opens the release page in the browser. Otherwise the program does not access the network.

## Building

Requirements: Windows 10 or later, a stable Rust toolchain (MSVC target, 2024 edition) and the Visual Studio 2022 Build Tools with the C++ workload. For `build.py`, [uv](https://docs.astral.sh/uv/).

```
uv run build.py          # target\release\metalpdf.exe
cargo test
```

`build.py` first closes a `metalpdf.exe` running from the project's `target` folder, because it would lock the file, then runs `cargo build --release`. When the program is not running, plain `cargo build --release` works as well. The executable links the C runtime statically (`.cargo/config.toml`), so it needs nothing beyond Windows itself.

## Installer and release

The release script builds the executable, the installer and the portable archive. It needs [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```
python tools/make_release.py              # tests, release build, installer, archive
python tools/make_release.py --no-tests   # the same without cargo test
python tools/make_release.py --install    # then a silent installation over the installed copy
```

The script writes the files to `dist` and takes the version from `Cargo.toml`. The installer script is `tools/setup.iss`; `metalpdf.exe --export-icon <file.ico>` writes the installer icon.

Releases on GitHub are built by the **Release** workflow (`.github/workflows/release.yml`). After the version in `Cargo.toml` is updated and committed, pushing a matching tag publishes a release with the installer and the portable archive:

```
git tag v0.1.0
git push origin v0.1.0
```

The workflow stops if the tag does not match the version in `Cargo.toml`. Running the workflow manually (Actions → Release → Run workflow) builds the files and attaches them to the run, without making a release.

## Built with

MetalPDF is built on the following open-source projects:

- [egui / eframe](https://github.com/emilk/egui): the user interface and the window;
- [Inno Setup](https://jrsoftware.org/isinfo.php): the installer.

## License

MIT, see [LICENSE](LICENSE).
