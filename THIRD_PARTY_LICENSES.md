# Third-party licenses

MetalPDF's own source code is licensed under the MIT License (see [LICENSE](LICENSE)).

## PDFium

The installer and the portable archive include `pdfium.dll`, [PDFium](https://pdfium.googlesource.com/pdfium/), the PDF engine of Chromium, which parses and renders every document the program shows. The program loads it from its own folder at run time.

The included build comes from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries), release [chromium/8111](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F8111), file `pdfium-win-x64.tgz` (PDFium 158.0.8111.0). `tools/get_pdfium.py` downloads exactly this build and checks its SHA-256 (`3452d11031050ec65e57f180215b57b9e6d465e3fbedafa8a77ad90885c7d23e`).

PDFium is licensed under the **BSD 3-Clause License**; the libraries built into the DLL (FreeType, libjpeg-turbo, OpenJPEG, libpng, zlib, HarfBuzz, ICU, Little-CMS, Abseil, Highway, simdutf, agg, fast_float, dragonbox, llvm-libc) are under their own permissive licences. All of their texts are in [licenses/PDFium-LICENSE.txt](licenses/PDFium-LICENSE.txt), written by `tools/get_pdfium.py` from the archive's `licenses/` folder.

## egui / eframe

The user interface uses [egui and eframe](https://github.com/emilk/egui), licensed under the MIT License or the Apache License 2.0, and further Rust crates under permissive licenses (MIT, Apache 2.0, BSD, ISC, Zlib, BSL, Unicode), compiled into `metalpdf.exe`, together with the fonts egui embeds (Ubuntu Font Licence, SIL Open Font License 1.1, MIT). Their licence texts and copyright notices are in [licenses/THIRD_PARTY_NOTICES.txt](licenses/THIRD_PARTY_NOTICES.txt), written by `tools/notices.py` from `Cargo.lock`; the program also shows them in its About window (**Licenses…**).
