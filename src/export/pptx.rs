//! The PowerPoint presentation: a slide a page, laid out as the page,
//! written as PresentationML by hand.
//!
//! The slides take the first page's size (PowerPoint has one size for a
//! presentation, from 1 to 56 inches a side: a page outside that is
//! scaled); a page of another size is fitted and centred. On a slide,
//! from the back: the picture of the page with its text and its own
//! pictures taken out (left off when it is white: a page of text has
//! none), the page's pictures each as a picture, a text box a paragraph.
//! Invisible text (a scan's recognised layer) is not taken: the scan
//! shows it already.

use std::fmt::Write as _;
use std::fs::File;
use std::io::BufWriter;

use super::layout::{self, Align, Item, Options, Vert};
use super::xml::{HEAD, esc};
use super::zip::ZipWriter;
use super::{Faces, FontInfo, GraphicKind, Meta, PageContent, all_white, crop_bgrx, encode_png, shrink_rgba};
use crate::pdfium::{Page, sys::Api};

pub struct Writer {
    zip: ZipWriter<BufWriter<File>>,
    /// The slide's size in EMUs.
    slide: [f64; 2],
    count: usize,
    slides: usize,
    pictures: usize,
    faces: Faces,
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

const GROUP: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

/// The PDF's page size in EMUs scaled into PowerPoint's limits.
pub fn slide_size(page: [f32; 2]) -> [f64; 2] {
    const MIN: f64 = 914_400.0;
    const MAX: f64 = 51_206_400.0;
    let (w, h) = (f64::from(page[0].max(1.0)) * 12700.0, f64::from(page[1].max(1.0)) * 12700.0);
    let mut k = 1.0;
    if w.max(h) * k > MAX {
        k = MAX / w.max(h);
    }
    if w.min(h) * k < MIN {
        k = MIN / w.min(h);
    }
    [(w * k).clamp(MIN, MAX).round(), (h * k).clamp(MIN, MAX).round()]
}

impl Writer {
    pub fn new(mut zip: ZipWriter<BufWriter<File>>, meta: Meta, first: [f32; 2], count: usize) -> std::io::Result<Writer> {
        let slide = slide_size(first);
        let add = |zip: &mut ZipWriter<BufWriter<File>>, name: &str, xml: &str| zip.add(name, format!("{HEAD}{xml}").as_bytes(), true);
        let mut types = String::from(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/presProps.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presProps+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
        for n in 1..=count {
            let _ = write!(types, r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#);
        }
        types.push_str("</Types>");
        add(&mut zip, "[Content_Types].xml", &types)?;
        add(
            &mut zip,
            "_rels/.rels",
            &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{REL}/extended-properties" Target="docProps/app.xml"/></Relationships>"#),
        )?;
        zip.add("docProps/core.xml", super::docx::core_xml(&meta).as_bytes(), true)?;
        add(&mut zip, "docProps/app.xml", r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>MetalPDF</Application></Properties>"#)?;
        let mut ids = String::new();
        let mut rels = format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="theme/theme1.xml"/><Relationship Id="rId3" Type="{REL}/presProps" Target="presProps.xml"/>"#);
        for n in 1..=count {
            let _ = write!(ids, r#"<p:sldId id="{}" r:id="rIdS{n}"/>"#, 255 + n);
            let _ = write!(rels, r#"<Relationship Id="rIdS{n}" Type="{REL}/slide" Target="slides/slide{n}.xml"/>"#);
        }
        rels.push_str("</Relationships>");
        add(
            &mut zip,
            "ppt/presentation.xml",
            &format!(r#"<p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>{ids}</p:sldIdLst><p:sldSz cx="{}" cy="{}"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#, slide[0], slide[1]),
        )?;
        add(&mut zip, "ppt/_rels/presentation.xml.rels", &rels)?;
        add(&mut zip, "ppt/presProps.xml", &format!("<p:presentationPr {NS}/>"))?;
        add(
            &mut zip,
            "ppt/slideMasters/slideMaster1.xml",
            &format!(r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree>{GROUP}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="4400"/></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#),
        )?;
        add(
            &mut zip,
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{REL}/theme" Target="../theme/theme1.xml"/></Relationships>"#),
        )?;
        add(
            &mut zip,
            "ppt/slideLayouts/slideLayout1.xml",
            &format!(r#"<p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{GROUP}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#),
        )?;
        add(
            &mut zip,
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            &format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#),
        )?;
        add(&mut zip, "ppt/theme/theme1.xml", THEME)?;
        Ok(Writer { zip, slide, count, slides: 0, pictures: 0, faces: Faces::default() })
    }

    fn picture(&mut self, png: &[u8]) -> Result<usize, String> {
        self.pictures += 1;
        self.zip.add(&format!("ppt/media/image{}.png", self.pictures), png, false).map_err(|e| e.to_string())?;
        Ok(self.pictures)
    }

    pub fn page(&mut self, _api: &'static Api, page: &Page, content: &PageContent, fonts: &[FontInfo], lookup: &mut crate::fonts::Fonts) -> Result<(), String> {
        let [w, h] = content.size;
        // Points to EMUs, fitted and centred on the slide.
        let k = (self.slide[0] / (f64::from(w.max(1.0)))).min(self.slide[1] / f64::from(h.max(1.0)));
        let (ox, oy) = ((self.slide[0] - f64::from(w) * k) / 2.0, (self.slide[1] - f64::from(h) * k) / 2.0);
        let ex = |x: f32| (ox + f64::from(x) * k).round() as i64;
        let ey = |y: f32| (oy + f64::from(y) * k).round() as i64;
        let len = |d: f32| (f64::from(d.max(0.0)) * k).round().max(1.0) as i64;
        // Points as drawn on the slide (font sizes, line pitches).
        let pt = |v: f32| f64::from(v) * k / 12700.0;
        let items = layout::analyse(content, Options { invisible: false, figures: false });
        let scale = super::PICTURE_DPI / 72.0;
        // The page's own pictures, read before they are taken out.
        let mut pictures: Vec<(usize, [f32; 4])> = Vec::new();
        let mut taken = Vec::new();
        for g in &content.graphics {
            let (Some(index), GraphicKind::Image) = (g.object, g.kind) else { continue };
            let (gw, gh) = (g.rect[2] - g.rect[0], g.rect[3] - g.rect[1]);
            if gw < 2.0 || gh < 2.0 {
                continue;
            }
            let Ok((iw, ih, rgba)) = page.image_pixels(index) else { continue };
            let (iw, ih, rgba) = shrink_rgba(iw, ih, rgba, (gw * scale).ceil() as u32, (gh * scale).ceil() as u32);
            let rel = self.picture(&encode_png(iw, ih, &rgba, true)?)?;
            pictures.push((rel, g.rect));
            taken.push(index);
        }
        page.hide_for_background(&taken);
        let bscale = super::BACKGROUND_DPI / 72.0;
        let (bw, bh) = (((w * bscale).round() as u32).max(1), ((h * bscale).round() as u32).max(1));
        let background = match page.render_whole(bw, bh, 0) {
            Some(bitmap) if !all_white(bitmap.pixels()) => Some(self.picture(&encode_png(bw, bh, &crop_bgrx(bitmap.pixels(), bw, [0, 0, bw, bh]), false)?)?),
            _ => None,
        };

        let mut tree = String::new();
        let mut rels = format!(r#"<Relationship Id="rId1" Type="{REL}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
        let mut id = 1;
        let mut pic = |tree: &mut String, rels: &mut String, rel: usize, name: &str, r: [f32; 4]| {
            id += 1;
            let _ = write!(rels, r#"<Relationship Id="rIdImg{rel}" Type="{REL}/image" Target="../media/image{rel}.png"/>"#);
            let _ = write!(
                tree,
                r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="{name} {id}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg{rel}"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
                ex(r[0]),
                ey(r[1]),
                len(r[2] - r[0]),
                len(r[3] - r[1])
            );
        };
        if let Some(rel) = background {
            pic(&mut tree, &mut rels, rel, "Page", [0.0, 0.0, w, h]);
        }
        for &(rel, r) in &pictures {
            pic(&mut tree, &mut rels, rel, "Picture", r);
        }
        for item in &items {
            let Item::Para(p) = item else { continue };
            id += 1;
            let first = &p.lines[0];
            let last = &p.lines[p.lines.len() - 1];
            let multi = p.lines.len() > 1;
            let x = p.rect[0];
            // The box's top: the first baseline less the font's ascent.
            let top = first.baseline - 0.9 * first.size;
            let height = (last.baseline - first.baseline) + 1.25 * p.size;
            let width = p.rect[2] - x;
            let width = if multi { width * 1.02 + 1.0 } else { width + 0.5 * p.size };
            let lefts: Vec<f32> = p.lines.iter().map(|l| l.rect[0]).collect();
            let (mar, indent) = match (p.text_start(), p.align) {
                (Some(start), _) => (start - x, lefts[0] - start),
                (None, Align::Center | Align::Right) => (0.0, 0.0),
                (None, _) => {
                    let body = if multi { lefts[1..].iter().copied().fold(f32::INFINITY, f32::min) } else { lefts[0] };
                    (body - x, lefts[0] - body)
                }
            };
            // A box of one line sits where the line was: aligned left, or
            // a different font's width moved it off.
            let algn = match p.align {
                _ if !multi => "l",
                Align::Left => "l",
                Align::Center => "ctr",
                Align::Right => "r",
                Align::Justify => "just",
            };
            let wrap = if multi { "square" } else { "none" };
            let _ = write!(
                tree,
                r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Text {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="{wrap}" lIns="0" tIns="0" rIns="0" bIns="0" rtlCol="0" anchor="t"><a:noAutofit/></a:bodyPr><a:lstStyle/><a:p><a:pPr marL="{}" indent="{}" algn="{algn}">"#,
                ex(x),
                ey(top),
                len(width),
                len(height),
                len(mar.max(0.0)).min(len(width)),
                ((f64::from(indent) * k).round() as i64).max(-len(mar.max(0.0))),
            );
            if multi {
                let _ = write!(tree, r#"<a:lnSpc><a:spcPts val="{}"/></a:lnSpc>"#, (pt(p.pitch()) * 100.0).round().clamp(0.0, 158_400.0) as i64);
            }
            tree.push_str("</a:pPr>");
            for span in layout::spans(p, " ") {
                let face = self.faces.get(span.font, fonts, lookup).for_text(&span.text).into_owned();
                let sz =(pt(span.size) * 100.0).round().clamp(100.0, 400_000.0) as i64;
                let _ = write!(tree, r#"<a:r><a:rPr sz="{sz}""#);
                if face.bold {
                    tree.push_str(r#" b="1""#);
                }
                if face.italic {
                    tree.push_str(r#" i="1""#);
                }
                match span.vert {
                    Vert::Super => tree.push_str(r#" baseline="30000""#),
                    Vert::Sub => tree.push_str(r#" baseline="-25000""#),
                    Vert::Base => {}
                }
                let [r, g, b] = span.color;
                let f = esc(&face.family);
                let _ = write!(
                    tree,
                    r#" dirty="0"><a:solidFill><a:srgbClr val="{r:02X}{g:02X}{b:02X}"/></a:solidFill><a:latin typeface="{f}"/><a:ea typeface="{f}"/><a:cs typeface="{f}"/></a:rPr><a:t>{}</a:t></a:r>"#,
                    esc(&span.text)
                );
            }
            tree.push_str("</a:p></p:txBody></p:sp>");
        }
        self.slides += 1;
        let n = self.slides;
        let slide = format!(r#"{HEAD}<p:sld {NS}><p:cSld><p:spTree>{GROUP}{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#);
        self.zip.add(&format!("ppt/slides/slide{n}.xml"), slide.as_bytes(), true).map_err(|e| e.to_string())?;
        let rels = format!(r#"{HEAD}<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#);
        self.zip.add(&format!("ppt/slides/_rels/slide{n}.xml.rels"), rels.as_bytes(), true).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn finish(self) -> Result<(), String> {
        if self.slides != self.count {
            return Err(format!("{} slides of {}", self.slides, self.count));
        }
        self.zip.finish().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slides_stay_within_powerpoints_limits() {
        // A4 as it is.
        assert_eq!(slide_size([595.0, 842.0]), [7_556_500.0, 10_693_400.0]);
        // A plan 200 inches wide, scaled down to 56.
        let big = slide_size([14_400.0, 7_200.0]);
        assert_eq!(big[0], 51_206_400.0);
        assert!((big[1] - 25_603_200.0).abs() < 2.0);
        // A label half an inch high, scaled up to an inch.
        let small = slide_size([144.0, 36.0]);
        assert_eq!(small[1], 914_400.0);
    }
}
