//! Retain complex artwork while making its safely removable text editable.
use crate::export::{Glyph, PageContent, layout};
use crate::pdfium::{Bitmap, Page};

pub(super) struct Region {
    pub rect: [f32; 4],
    pub lines: Vec<layout::Line>,
    pub exact: Vec<layout::Line>,
}

pub(super) struct Layers {
    regions: Vec<Region>,
    render: Option<Option<Bitmap>>,
}

fn inside(g: &Glyph, r: [f32; 4]) -> bool {
    let (x, y) = ((g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0);
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

impl Layers {
    pub(super) fn prepare(
        page: &Page,
        content: &PageContent,
        fonts: &[crate::export::FontInfo],
        regions: Vec<[f32; 4]>,
        original: &mut Option<Option<Bitmap>>,
    ) -> Result<Self, String> {
        // Assign each glyph once. Only complete, opaque text objects can
        // be removed safely; partial objects and text effects stay painted.
        let assigned: Vec<_> = content
            .glyphs
            .iter()
            .filter(|g| g.source >= 0 && !g.invisible && !matches!(g.ch as u32,0xe000..=0xf8ff|0xf0000..=0xffffd|0x100000..=0x10fffd))
            .filter(|g|!fonts.get(g.font as usize).is_some_and(super::equations::painted))
            .filter_map(|g| regions.iter().position(|&r| inside(g, r)).map(|i| (g, i)))
            .collect();
        let selected = assigned.iter().map(|(g, _)| g.source).collect();
        if !assigned.is_empty() {
            let scale = crate::export::PICTURE_DPI / 72.0;
            let (w, h) = (
                ((content.size[0] * scale).round() as u32).max(1),
                ((content.size[1] * scale).round() as u32).max(1),
            );
            // Keep the original for inline equations and figures with no
            // editable labels, before mutating this export-only PDF page.
            if original
                .get_or_insert_with(|| page.render_whole(w, h, content.turns))
                .is_none()
            {
                return Err("could not render the original page artwork".into());
            }
        }
        let hidden = page.hide_selected_text(&selected);
        let regions = regions
            .into_iter()
            .enumerate()
            .map(|(i, rect)| Region {
                rect,
                exact:Vec::new(),
                lines: {
                    let mut glyphs = assigned
                        .iter()
                        .filter(|(g, region)| *region == i && hidden.contains(&g.source))
                        .map(|(g, _)| (*g).clone())
                        .collect();
                    layout::join_accents(&mut glyphs);
                    layout::lines(glyphs)
                },
            })
            .collect();
        Ok(Self {
            regions,
            render: None,
        })
    }

    pub(super) fn take(
        &mut self,
        page: &Page,
        content: &PageContent,
        rect: [f32; 4],
        original: &mut Option<Option<Bitmap>>,
    ) -> Result<(Vec<u8>, Region), String> {
        let index = self
            .regions
            .iter()
            .position(|r| (0..4).all(|i| (r.rect[i] - rect[i]).abs() < 0.1));
        let region = index.map(|i| self.regions.remove(i)).unwrap_or(Region {
            rect,
            lines: Vec::new(),
            exact:Vec::new(),
        });
        // Crops without exported labels must use the untouched rendering:
        // nearby removable objects can extend beyond their region boundary.
        let render = if region.lines.is_empty() {
            original
        } else {
            &mut self.render
        };
        Ok((super::render_crop(page, content, rect, render)?, region))
    }
}

/// The group's viewport may be scaled by its inline paragraph, while its
/// children keep their original PDF coordinates relative to the region.
pub(super) fn graphic(
    x: &mut String,
    rel: usize,
    n: usize,
    viewport: [i64; 2],
    region: &Region,
    out: &mut super::Out,
) {
    use std::fmt::Write;
    let [cx, cy] = viewport;
    let grouped = !region.lines.is_empty() || !region.exact.is_empty();
    let (w, h) = if grouped {
        (
            super::emu(region.rect[2] - region.rect[0]),
            super::emu(region.rect[3] - region.rect[1]),
        )
    } else {
        (cx, cy)
    };
    let mut text = String::new();
    if grouped {
        out.textboxes.block(&mut text, &region.lines, region.rect, out.fonts, out.lookup);
        out.textboxes.math_block(&mut text,&region.exact,region.rect,out.fonts,out.lookup);
    }
    if grouped {
        let _ = write!(
            x,
            r#"<a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"><wpg:wgp xmlns:wpg="http://schemas.microsoft.com/office/word/2010/wordprocessingGroup" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wpg:cNvGrpSpPr/><wpg:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/><a:chOff x="0" y="0"/><a:chExt cx="{w}" cy="{h}"/></a:xfrm></wpg:grpSpPr>"#
        );
    } else {
        x.push_str(r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#);
    }
    let _ = write!(
        x,
        r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="image{rel}.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg{rel}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>"#
    );
    if grouped {
        x.push_str(&text);
        x.push_str("</wpg:wgp>");
    }
    x.push_str("</a:graphicData></a:graphic>");
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_ruled_table_keeps_editable_labels_without_painting_duplicates() {
        let stream = "BT /F 10 Tf 40 700 Td (Ordinary editable body text before the table remains a paragraph.) Tj ET 100 599 350 1 re f 100 559 350 1 re f 100 499 350 1 re f BT /F 10 Tf 110 575 Td (Editable cell) Tj ET BT /F 10 Tf 310 575 Td (First value) Tj ET BT /F 10 Tf 110 535 Td (Another cell) Tj ET BT /F 10 Tf 310 535 Td (Second value) Tj ET BT /F 10 Tf 40 430 Td (Ordinary editable body text after the table remains a paragraph.) Tj ET";
        let pdf = crate::pdfium::tests::write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F 5 0 R >> >> >>".into(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream",stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ]);
        let input =
            std::env::temp_dir().join(format!("metalpdf_overlay_{}.pdf", std::process::id()));
        let output = input.with_extension("docx");
        std::fs::write(&input, pdf).unwrap();
        assert_eq!(
            crate::export::run_paragraphs(&input, &output, None).unwrap(),
            1
        );
        let bytes = std::fs::read(&output).unwrap();
        let read = |name| crate::export::zip::read_entry(&bytes, name).unwrap();
        let xml = String::from_utf8(read("word/document.xml")).unwrap();
        assert_eq!(xml.matches("<wpg:wgp ").count(), 1);
        // A small table needs one text story, not a shape per cell. Large
        // books otherwise make Word spend seconds laying out child shapes.
        assert_eq!(xml.matches("<w:txbxContent>").count(), 1);
        let labels = xml
            .split("<w:txbxContent>")
            .nth(1)
            .unwrap()
            .split("</w:txbxContent>")
            .next()
            .unwrap();
        assert_eq!(labels.matches("<w:p>").count(), 2);
        assert_eq!(labels.matches("<w:r><w:tab/></w:r>").count(), 2);
        let text: String = xml
            .split("<w:t xml:space=\"preserve\">")
            .skip(1)
            .map(|part| part.split("</w:t>").next().unwrap())
            .collect();
        for label in [
            "Editable cell",
            "First value",
            "Another cell",
            "Second value",
        ] {
            assert_eq!(text.matches(label).count(), 1);
        }
        assert!(xml.contains("<wp:inline ") && !xml.contains("w:framePr"));
        assert!(xml.contains("Ordinary editable body text after the table"));
        let png = read("word/media/image1.png");
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let width = decoder.info().width as usize;
        let height = decoder.info().height as usize;
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        let frame = decoder.next_frame(&mut pixels).unwrap();
        assert_eq!(frame.color_type, png::ColorType::Rgb);
        // Away from the three rules every pixel must be white: none of
        // the old cell values may remain underneath the editable labels.
        let mut rule_pixels = 0;
        for y in 0..height {
            let pdf_y = 199.5 + y as f32 * 72.0 / crate::export::PICTURE_DPI;
            for x in 4..width - 4 {
                let at = (y * width + x) * 3;
                let dark = pixels[at..at + 3].iter().any(|&v| v < 240);
                if [200.0, 240.0, 300.0]
                    .iter()
                    .all(|v| (pdf_y - v).abs() > 2.0)
                {
                    assert!(
                        !dark,
                        "painted text remains in the table background at {x},{y}"
                    );
                } else if dark {
                    rule_pixels += 1;
                }
            }
        }
        assert!(rule_pixels > width);
        std::fs::remove_file(input).unwrap();
        std::fs::remove_file(output).unwrap();
    }
}
