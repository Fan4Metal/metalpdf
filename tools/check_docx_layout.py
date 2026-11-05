"""Render a DOCX export in Microsoft Word and compare its pages with the PDF.

Optional QA dependencies (not needed by MetalPDF):
    uv run --with pymupdf --with pillow --with pywin32 tools/check_docx_layout.py input.pdf output.docx

Word is opened in a separate, hidden instance; the input files are read-only.
The rendered PDF, page images and JSON report are written beside the report.
Use --output-dir to keep these QA artifacts outside the source directories.
"""

import argparse
import difflib
import json
import statistics
import sys
from pathlib import Path

import pymupdf
from PIL import Image, ImageChops, ImageStat
from win32com.client import DispatchEx


def characters(page):
    return [
        (c["c"], c["origin"])
        for block in page.get_text("rawdict")["blocks"]
        if "lines" in block
        for line in block["lines"]
        for span in line["spans"]
        for c in span["chars"]
        if not c["c"].isspace()
    ]


def compare(source_path, rendered_path, output_dir):
    result = {"source": str(source_path), "rendered": str(rendered_path), "pages": []}
    with pymupdf.open(source_path) as source, pymupdf.open(rendered_path) as rendered:
        result.update(source_pages=len(source), rendered_pages=len(rendered))
        for i in range(min(len(source), len(rendered))):
            a, b = source[i], rendered[i]
            ca, cb = characters(a), characters(b)
            matcher = difflib.SequenceMatcher(
                None, "".join(c for c, _ in ca), "".join(c for c, _ in cb), autojunk=False
            )
            dx, dy = [], []
            for match in matcher.get_matching_blocks():
                for j in range(match.size):
                    x, y = ca[match.a + j][1]
                    u, v = cb[match.b + j][1]
                    dx.append(abs(x - u))
                    dy.append(abs(y - v))
            # Geometry statistics are diagnostic: repeated text, raster-only
            # artwork and a different reading order can confuse text matching.
            page = {"page": i + 1, "source_size": list(a.rect), "rendered_size": list(b.rect), "matched_characters": len(dx)}
            if dx:
                page.update(median_x_error_pt=statistics.median(dx), median_y_error_pt=statistics.median(dy))
            images = []
            for label, pdf_page in [("source", a), ("word", b)]:
                pix = pdf_page.get_pixmap(alpha=False)
                img = Image.frombytes("RGB", (pix.width, pix.height), pix.samples)
                img.save(output_dir / f"page-{i + 1}-{label}.png")
                images.append(img)
            same_size = images[0].size == images[1].size
            page["same_pixel_size"] = same_size
            if same_size:
                diff = ImageChops.difference(*images)
                page["mean_pixel_error_255"] = statistics.mean(ImageStat.Stat(diff).mean)
                diff.save(output_dir / f"page-{i + 1}-diff.png")
            result["pages"].append(page)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pdf", type=Path)
    parser.add_argument("docx", type=Path)
    parser.add_argument("--output-dir", type=Path, default=Path("local/docx-layout-check"))
    args = parser.parse_args()
    out = args.output_dir.resolve()
    out.mkdir(parents=True, exist_ok=True)
    pdf = out / "word.pdf"
    word = DispatchEx("Word.Application")
    word.Visible = False
    word.DisplayAlerts = 0
    try:
        document = word.Documents.Open(str(args.docx.resolve()), ReadOnly=True, AddToRecentFiles=False)
        try:
            document.ExportAsFixedFormat(str(pdf), 17)
        finally:
            document.Close(False)
    finally:
        word.Quit()
    report = compare(args.pdf.resolve(), pdf, out)
    (out / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"Pages: {report['source_pages']} PDF / {report['rendered_pages']} Word; report: {out / 'report.json'}")
    return 0 if report["source_pages"] == report["rendered_pages"] and all(p["same_pixel_size"] for p in report["pages"]) else 1


if __name__ == "__main__":
    sys.exit(main())
