r"""
Fetch pdfium.dll (and the PDFium headers, for reference) into vendor\pdfium.

The viewer loads pdfium.dll from its own folder at run time (src/pdfium/sys.rs),
so the build itself needs nothing; build.py, tools/make_release.py and the
release workflow call this script, and build.rs copies the DLL next to the exe.
The package is a pinned build of bblanchon/pdfium-binaries (Chromium's PDFium,
BSD-3-Clause), checked against its SHA-256. That project keeps every release,
so no mirror is needed. A second run does nothing when the DLL of the pinned
build is already there.

No external dependencies. Run from any folder: python tools/get_pdfium.py [--force]
"""

import argparse
import hashlib
import shutil
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

# --- The pinned build ---------------------------------------------------------------
# The release tag of bblanchon/pdfium-binaries (weekly builds, "chromium/<build>").
PDFIUM_TAG = "chromium/8111"
PDFIUM_ASSET = "pdfium-win-x64.tgz"
PDFIUM_URL = (
    "https://github.com/bblanchon/pdfium-binaries/releases/download/"
    f"{PDFIUM_TAG.replace('/', '%2F')}/{PDFIUM_ASSET}"
)
PDFIUM_SHA256 = "3452d11031050ec65e57f180215b57b9e6d465e3fbedafa8a77ad90885c7d23e"
# -------------------------------------------------------------------------------------

ROOT = Path(__file__).resolve().parent.parent
VENDOR = ROOT / "vendor"
PDFIUM = VENDOR / "pdfium"
DLL = PDFIUM / "pdfium.dll"
# Written last: the folder is complete only when it names the pinned build.
STAMP = PDFIUM / "VERSION"
# What is taken out of the archive: the DLL, its licence and the headers.
WANTED_PREFIXES = ("bin/pdfium.dll", "LICENSE", "VERSION", "include/", "licenses/")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


class DownloadError(Exception):
    pass


def download(url: str, to: Path, expected: str) -> None:
    """Download `url` to `to` and check its SHA-256; a failure leaves nothing behind."""
    print(f"Downloading {url}", flush=True)
    request = urllib.request.Request(url, headers={"User-Agent": "metalpdf-get_pdfium"})
    partial = to.with_suffix(to.suffix + ".part")
    try:
        with urllib.request.urlopen(request, timeout=60) as answer, partial.open("wb") as out:
            shutil.copyfileobj(answer, out, 1 << 20)
    except (urllib.error.URLError, OSError) as e:
        partial.unlink(missing_ok=True)
        raise DownloadError(f"cannot download {url}: {e}") from e
    actual = sha256(partial)
    if actual != expected:
        partial.unlink()
        raise DownloadError(f"SHA-256 mismatch for {url}\n  expected {expected}\n  got      {actual}")
    partial.replace(to)


def main() -> int:
    parser = argparse.ArgumentParser(description="Fetch the pinned pdfium.dll into vendor\\pdfium")
    parser.add_argument("--force", action="store_true", help="download again even if it is there")
    args = parser.parse_args()

    if not args.force and DLL.is_file() and STAMP.is_file() and STAMP.read_text().strip() == PDFIUM_TAG:
        write_licence_file()
        print(f"PDFium is up to date: {DLL}")
        return 0

    VENDOR.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=VENDOR) as tmp:
        archive = Path(tmp) / PDFIUM_ASSET
        try:
            download(PDFIUM_URL, archive, PDFIUM_SHA256)
        except DownloadError as e:
            raise SystemExit(f"ERROR: {e}") from e
        extracted = Path(tmp) / "x"
        with tarfile.open(archive, "r:gz") as tar:
            members = [m for m in tar.getmembers() if m.name.startswith(WANTED_PREFIXES)]
            # "data": no paths outside the folder, no device files, no setuid bits.
            tar.extractall(extracted, members=members, filter="data")
        if PDFIUM.exists():
            shutil.rmtree(PDFIUM)
        PDFIUM.mkdir()
        shutil.move(extracted / "bin" / "pdfium.dll", DLL)
        for name in ("LICENSE", "include", "licenses"):
            if (extracted / name).exists():
                shutil.move(extracted / name, PDFIUM / name)
        # The archive's own VERSION file (the Chromium build numbers) is kept
        # beside the DLL under another name; the stamp names the tag.
        if (extracted / "VERSION").is_file():
            shutil.move(extracted / "VERSION", PDFIUM / "BUILD")
        STAMP.write_text(PDFIUM_TAG + "\n")
    write_licence_file()
    print(f"PDFium: {DLL} ({DLL.stat().st_size >> 20} MB)")
    return 0


# The licence texts of the DLL, shipped with the program (the installer, the
# portable archive, About -> Licenses): PDFium's own first, then those of the
# libraries built into it, as the archive's licenses/ folder holds them.
LICENCE_FILE = ROOT / "licenses" / "PDFium-LICENSE.txt"


def write_licence_file() -> None:
    folder = PDFIUM / "licenses"
    if not folder.is_dir():
        return
    files = sorted(folder.iterdir(), key=lambda f: (f.name != "pdfium.txt", f.name.lower()))
    rule = "=" * 78
    parts = [
        f"PDFium {PDFIUM_TAG} (bblanchon/pdfium-binaries, {PDFIUM_ASSET})",
        rule,
        "",
        "pdfium.dll is PDFium, the PDF engine of Chromium, under the BSD 3-Clause",
        "License below, built with the libraries that follow it, each under its",
        "own licence.",
    ]
    for f in files:
        text = f.read_text(encoding="utf-8", errors="replace").strip()
        parts += ["", rule, f.name, rule, "", text]
    LICENCE_FILE.parent.mkdir(exist_ok=True)
    LICENCE_FILE.write_text("\n".join(parts) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    sys.exit(main())
