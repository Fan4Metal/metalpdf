//! Captions belong to the figure's editable text story, outside body flow.
use crate::export::{Graphic, GraphicKind, PageContent, layout};
type Rect = [f32;4];

fn inside(g: &crate::export::Glyph, r: Rect) -> bool {
    let (x,y)=((g.rect[0]+g.rect[2])/2.0,(g.rect[1]+g.rect[3])/2.0);
    x>=r[0]-0.1 && x<=r[2]+0.1 && y>=r[1]-0.1 && y<=r[3]+0.1
}

fn label(glyphs: &[crate::export::Glyph]) -> bool {
    let text: String=glyphs.iter().take(18).map(|g|g.ch.to_lowercase().next().unwrap_or(g.ch)).collect();
    ["рис.","рисунок","fig.","figure"].iter().any(|prefix|
        text.strip_prefix(prefix).is_some_and(|tail|tail.trim_start().starts_with(|c:char|c.is_ascii_digit())))
}

fn starts_label(glyphs: &[crate::export::Glyph]) -> bool {
    let text: String=glyphs.iter().take(8).map(|g|g.ch.to_lowercase().next().unwrap_or(g.ch)).collect();
    ["рис.","рисунок","fig.","figure"].iter().any(|prefix|text.starts_with(prefix))
}

/// A continuous body line crossing the caption lane is not a continuation
/// of the caption. Separate columns leave a real gap at that boundary.
fn crosses(line: &layout::Line, lane: [f32;2]) -> bool {
    lane.iter().any(|&edge| {
        line.glyphs.iter().any(|g|g.rect[0]<edge-0.5 && g.rect[2]>edge+0.5)
            || line.glyphs.windows(2).any(|w|w[0].rect[2]<=edge && w[1].rect[0]>=edge
                && w[1].rect[0]-w[0].rect[2]<0.7*line.size)
    })
}

/// Extend only confidently labelled compositions before body analysis.
/// Layers reads their editable text from the untouched source page.
pub(super) fn prepare(content: &mut PageContent) -> Vec<Rect> {
    if !content.glyphs.iter().enumerate().any(|(i,g)| matches!(g.ch,'Р'|'р'|'F'|'f') && starts_label(&content.glyphs[i..])) {
        return Vec::new();
    }
    let mut figures=layout::labelled_figures(content);
    // A labelled logical gate may be just one rectangle and a few ports;
    // the general figure detector deliberately leaves such rules as text.
    for graphic in &content.graphics {
        let mut r=graphic.rect;
        if r[2]-r[0]<20.0 || r[3]-r[1]<12.0
            || figures.iter().any(|f|r[0]>=f.rect[0]-1.0 && r[2]<=f.rect[2]+1.0 && r[1]>=f.rect[1]-1.0 && r[3]<=f.rect[3]+1.0) {continue;}
        loop {
            let previous=r;
            for other in &content.graphics {
                let b=other.rect;
                let union=layout::union(r,b);
                if b[0]<=r[2]+2.0 && b[2]>=r[0]-2.0 && b[1]<=r[3]+2.0 && b[3]>=r[1]-2.0
                    && (union[2]-union[0])*(union[3]-union[1])<0.65*content.size[0]*content.size[1] {r=union;}
            }
            if r==previous {break;}
        }
        if !figures.iter().any(|f|f.rect==r) {figures.push(layout::Figure {rect:r,column:[r[0],r[2]],image:None});}
    }
    let mut glyphs=content.glyphs.clone();
    layout::join_accents(&mut glyphs);
    let lines=layout::lines(glyphs);
    // Some diagrams are typeset entirely with font glyphs. Their narrow,
    // smaller caption still needs its own text story, even without an image.
    let mut sizes=std::collections::HashMap::new();
    for g in content.glyphs.iter().filter(|g|g.ch.is_alphabetic()) {*sizes.entry((g.size*2.0).round() as i32).or_insert(0usize)+=1;}
    let body=sizes.into_iter().max_by_key(|&(s,n)|(n,s)).map_or(0.0,|(s,_)|s as f32/2.0);
    for line in &lines {
        if !starts_label(&line.glyphs) {continue;}
        let size=line.glyphs[0].size;
        if size>=0.96*body {continue;}
        let caption: Vec<_>=line.glyphs.iter().filter(|g|(g.size-size).abs()<0.06*size).collect();
        let Some(rect)=caption.iter().map(|g|g.rect).reduce(layout::union) else {continue};
        if rect[2]-rect[0]<30.0 || rect[2]-rect[0]>0.45*content.size[0] {continue;}
        figures.push(layout::Figure {rect:[rect[0],rect[1],rect[2],rect[1]],column:[rect[0],rect[2]],image:None});
    }
    let mut regions=Vec::new();
    for figure in &figures {
        let r=figure.rect;
        let width=r[2]-r[0];
        if width<30.0 || r[3]>r[1] && r[3]-r[1]<12.0 {continue;}
        let mut proposals=Vec::new();
        for line in &lines {
            for start in 0..line.glyphs.len() {
                let g=&line.glyphs[start];
                let size=g.size;
                if g.rect[1]<r[3]-1.0 || g.rect[1]>r[3]+2.5*size
                    || g.x>r[2]-size || !starts_label(&line.glyphs[start..]) {continue;}
                let centred=((line.rect[0]+line.rect[2]-r[0]-r[2])/2.0).abs()<0.3*width+size;
                if g.x<r[0]-2.0*size && !centred {continue;}
                let caption_only=r[1]==r[3];
                let beside=|l: &&layout::Line| if caption_only {l.rect[1]<r[3]+3.0*size && l.rect[3]>r[1]-size}
                    else {l.rect[1]<r[3]-size && l.rect[3]>r[1]+size};
                let left=lines.iter().filter(beside).filter(|l|l.rect[2]<=r[0]+0.5).map(|l|l.rect[2]).fold(f32::NEG_INFINITY,f32::max);
                let right=lines.iter().filter(beside).filter(|l|l.rect[0]>=r[2]-0.5).map(|l|l.rect[0]).fold(f32::INFINITY,f32::min);
                let lane=if caption_only {[(r[0]-0.5*size).max(left+0.2*size),(r[2]+size).min(right-0.2*size)]}
                    else {[(r[0]-2.0*size).min(g.x-0.5*size).max(left+0.2*size),
                        (r[2]+2.0*size).max(line.rect[2]+0.5*size).min(right-0.2*size)]};
                if g.x<lane[0]-0.5 {continue;}
                let selected: Vec<_>=content.glyphs.iter().filter(|g|
                    g.rect[0]>=lane[0]-0.5 && g.rect[2]<=lane[1]+0.5
                    && g.rect[1]>=line.rect[1]-0.5 && g.rect[1]<line.rect[1]+12.0*size
                    && !g.invisible).cloned().collect();
                let mut selected=selected;
                layout::join_accents(&mut selected);
                let mut caption=Vec::new();
                let mut rows: Vec<layout::Line>=Vec::new();
                for part in layout::lines(selected) {
                    if let Some(last)=rows.last_mut().filter(|l|(l.baseline-part.baseline).abs()<0.3*size) {
                        last.rect=layout::union(last.rect,part.rect);
                        last.glyphs.extend(part.glyphs);
                        last.glyphs.sort_by(|a,b|a.x.total_cmp(&b.x));
                    } else {rows.push(part);}
                }
                for part in rows {
                    if part.glyphs.iter().any(|g|g.source < -1) {break;}
                    if part.size>1.12*size || part.size<0.45*size {break;}
                    if let Some(previous)=caption.last() {
                        let previous: &layout::Line=previous;
                        let pitch=part.baseline-previous.baseline;
                        if pitch<0.65*size || pitch>1.65*size || label(&part.glyphs) {break;}
                    } else if !label(&part.glyphs) {break;}
                    if lines.iter().any(|l|(l.baseline-part.baseline).abs()<0.25*size && crosses(l,lane)) {break;}
                    let ends=part.glyphs.last().is_some_and(|g|matches!(g.ch,'.'|'!'|'?'))
                        && part.glyphs.iter().filter(|g|g.ch.is_alphabetic()).count()>8;
                    caption.push(part);
                    if ends {break;}
                }
                let Some(caption_rect)=caption.iter().map(|l|l.rect).reduce(layout::union) else {continue};
                if caption.iter().map(|l|l.glyphs.len()).sum::<usize>()<8 {continue;}
                let region=layout::union(r,caption_rect);
                // Expanding a narrow picture to a wide caption must not
                // swallow prose alongside its image or between caption rows.
                if content.glyphs.iter().any(|g|inside(g,region) && !inside(g,r)
                    && !caption.iter().any(|l|inside(g,l.rect))) {continue;}
                if regions.iter().any(|&other: &Rect|region[0]<other[2] && region[2]>other[0] && region[1]<other[3] && region[3]>other[1]) {continue;}
                proposals.push((caption_rect[1]-r[3],region));
            }
        }
        if let Some((_,region))=proposals.into_iter().min_by(|a,b|a.0.total_cmp(&b.0)) {
            regions.push(region);
        }
    }
    content.glyphs.retain(|g|!regions.iter().any(|&r|inside(g,r)));
    content.graphics.retain(|g|!regions.iter().any(|r|g.rect[0]>=r[0]-0.5 && g.rect[2]<=r[2]+0.5 && g.rect[1]>=r[1]-0.5 && g.rect[3]<=r[3]+0.5));
    for &rect in &regions {
        content.graphics.push(Graphic {rect,kind:GraphicKind::Image,object:None});
    }
    regions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{Glyph,Graphic,GraphicKind};

    fn text(value: &str,x: f32,y: f32,width: f32,size: f32) -> Vec<Glyph> {
        let advance=width/value.chars().count() as f32;
        value.chars().enumerate().map(|(i,ch)| {
            let left=x+i as f32*advance;
            Glyph {source:-1,ch,rect:[left,y,left+advance,y+size],x:left,baseline:y+0.8*size,size,font:0,color:[0;3],space_before:false,invisible:false}
        }).collect()
    }

    fn page(left: bool) -> PageContent {
        let r=if left {[50.0,90.0,170.0,190.0]} else {[330.0,90.0,450.0,190.0]};
        let mut glyphs=Vec::new();
        for i in 0..11 {
            glyphs.extend(text("Ordinary editable body text continues through the illustration",if left {182.0} else {50.0},90.0+14.0*i as f32,268.0,10.0));
        }
        glyphs.extend(text("Fig. 1.10. A portrait",r[0],198.0,120.0,9.0));
        glyphs.extend(text("Scientist (1894-1984).",r[0],208.5,100.0,9.0));
        PageContent {size:[500.0,400.0],glyphs,graphics:vec![Graphic {rect:r,kind:GraphicKind::Image,object:Some(0)}],..Default::default()}
    }

    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_caption_is_editable_once_and_its_pixels_are_removed() {
        let mut stream=String::from("0.2 0.5 0.8 rg 330 600 120 100 re f 0 g ");
        use std::fmt::Write;
        for i in 0..10 {let _=write!(stream,"BT /F 10 Tf 50 {} Td (Ordinary editable body text stays outside this caption block.) Tj ET ",690-14*i);}
        stream.push_str("BT /F 9 Tf 330 580 Td (Fig. 1. A separate caption) Tj ET BT /F 9 Tf 330 569 Td (Scientist 1894-1984.) Tj ET");
        let pdf=crate::pdfium::tests::write_pdf(vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F 5 0 R >> >> >>".into(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream",stream.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        ]);
        let input=std::env::temp_dir().join(format!("metalpdf_caption_{}.pdf",std::process::id()));
        let output=input.with_extension("docx");
        std::fs::write(&input,pdf).unwrap();
        crate::export::run_paragraphs(&input,&output,None).unwrap();
        let bytes=std::fs::read(&output).unwrap();
        let read=|name|crate::export::zip::read_entry(&bytes,name).unwrap();
        let xml=String::from_utf8(read("word/document.xml")).unwrap();
        let caption=xml.split("<w:txbxContent>").nth(1).unwrap().split("</w:txbxContent>").next().unwrap();
        let text=|part: &str|part.split("<w:t xml:space=\"preserve\">").skip(1).map(|p|p.split("</w:t>").next().unwrap()).collect::<String>();
        let caption_text=text(caption);
        assert!(caption_text.contains("Fig. 1. A separate caption") && caption_text.contains("Scientist 1894-1984."));
        assert!(!caption_text.contains("Ordinary editable"));
        assert_eq!(text(&xml).matches("Fig. 1. A separate caption").count(),1);
        assert_eq!(xml.matches("<wpg:wgp ").count(),1);
        let mut decoder=png::Decoder::new(std::io::Cursor::new(read("word/media/image1.png"))).read_info().unwrap();
        let width=decoder.info().width as usize;
        let mut pixels=vec![0;decoder.output_buffer_size().unwrap()];
        let frame=decoder.next_frame(&mut pixels).unwrap();
        assert_eq!(frame.color_type,png::ColorType::Rgb);
        let row=(110.0*crate::export::PICTURE_DPI/72.0) as usize;
        assert!(pixels[row*width*3..frame.buffer_size()].iter().all(|&v|v>=248),"caption pixels must not be duplicated behind its editable form");
        std::fs::remove_file(input).unwrap();std::fs::remove_file(output).unwrap();
    }

    #[test]
    fn caption_rows_are_outside_body_flow_and_inside_one_wrapping_group() {
        for left in [true,false] {
            let mut content=page(left);
            let regions=prepare(&mut content);
            assert_eq!(regions.len(),1,"left={left}");
            assert_eq!(regions[0][3],217.5);
            let (items,_)=super::super::flowing_items(&content);
            let body:String=items.iter().filter_map(|i|match i {layout::Item::Para(p)=>Some(p.lines.iter().map(|l|l.text()).collect::<String>()),_=>None}).collect();
            assert!(!body.contains("portrait") && !body.contains("Scientist"));
            assert_eq!(body.matches("Ordinary").count(),11);
            let (items,pictures)=super::super::wrapping::prepare(items,&content);
            assert_eq!(pictures.len(),1);
            assert!(pictures[0].figure.rect[3]>=217.5);
            assert!(matches!(items[pictures[0].anchor],layout::Item::Para(_)));
        }
    }

    #[test]
    fn an_overhanging_caption_keeps_its_hyphenated_name_and_neighbouring_body() {
        let mut content=PageContent {size:[420.0,595.0],..Default::default()};
        for i in 0..15 {content.glyphs.extend(text("Continuous body text remains a separate story.",171.8,221.0+11.34*i as f32,200.0,9.96));}
        content.glyphs.extend(text("Рис. 1.11. Хейке Камерлинг–",48.48,347.72,113.38,8.966));
        content.glyphs.extend(text("Оннес в 1878 г. (1853–1926).",48.48,358.16,113.0,8.966));
        content.glyphs.extend(text("W",48.48,368.6,8.5,8.966));
        content.graphics.push(Graphic {rect:[64.8,221.48,145.5,340.6],kind:GraphicKind::Image,object:Some(0)});
        let regions=prepare(&mut content);
        assert_eq!(regions.len(),1);
        assert!(regions[0][0]<50.0 && regions[0][2]>161.0 && regions[0][3]>375.0);
        let body: String=content.glyphs.iter().map(|g|g.ch).collect();
        assert_eq!(body.matches("Continuous").count(),15);
        assert!(!body.contains("Оннес") && !body.contains("Камерлинг"));
    }

    #[test]
    fn a_centred_caption_can_be_wider_than_its_diagram() {
        let mut content=PageContent {size:[500.0,400.0],..Default::default()};
        content.graphics.push(Graphic {rect:[200.0,90.0,300.0,150.0],kind:GraphicKind::Image,object:Some(0)});
        content.glyphs.extend(text("Fig. 3.2. A centred diagram caption.",165.0,158.0,170.0,9.0));
        content.glyphs.extend(text("An ordinary body paragraph starts below the diagram.",50.0,180.0,400.0,10.0));
        let regions=prepare(&mut content);
        assert_eq!(regions.len(),1);
        assert!(regions[0][0]<166.0 && regions[0][2]>334.0);
        let body: String=content.glyphs.iter().map(|g|g.ch).collect();
        assert!(body.starts_with("An ordinary") && !body.contains("caption"));
    }

    #[test]
    fn a_single_rectangular_diagram_can_own_a_caption() {
        let mut content=PageContent {size:[500.0,400.0],..Default::default()};
        content.graphics.push(Graphic {rect:[50.0,90.0,150.0,150.0],kind:GraphicKind::Path {curved:false},object:None});
        content.graphics.push(Graphic {rect:[40.0,105.0,50.0,105.0],kind:GraphicKind::Path {curved:false},object:None});
        content.glyphs.extend(text("Рис. 10.1. Логический",40.0,158.0,115.0,9.0));
        content.glyphs.extend(text("вентиль.",40.0,168.5,60.0,9.0));
        let regions=prepare(&mut content);
        assert_eq!(regions.len(),1);
        assert!(regions[0][0]<=40.0 && regions[0][3]>=177.5);
    }

    #[test]
    fn a_narrow_caption_without_a_graphics_seed_keeps_its_own_story() {
        let mut content=page(false);
        content.graphics.clear();
        let regions=prepare(&mut content);
        assert_eq!(regions.len(),1);
        let text: String=content.glyphs.iter().map(|g|g.ch).collect();
        assert!(!text.contains("portrait") && !text.contains("Scientist"));
        assert_eq!(text.matches("Ordinary").count(),11);
    }

    #[test]
    fn an_in_text_reference_and_unlabelled_prose_stay_in_the_body() {
        let mut content=page(false);
        content.glyphs.retain(|g|g.rect[0]<320.0);
        content.glyphs.extend(text("See Fig. 1.10 for details.",50.0,198.0,400.0,10.0));
        assert!(prepare(&mut content).is_empty());
        let mut content=page(false);
        content.glyphs.retain(|g|g.rect[0]<320.0);
        content.glyphs.extend(text("Ordinary paragraph text.",330.0,198.0,120.0,9.0));
        assert!(prepare(&mut content).is_empty());
    }

    #[test]
    fn a_label_and_number_drawn_far_apart_still_identify_one_caption() {
        let mut content=page(false);
        content.glyphs.retain(|g|g.rect[0]<320.0);
        content.glyphs.extend(text("Рис.",330.0,198.0,16.0,9.0));
        content.glyphs.extend(text("1.10.",368.0,198.0,22.0,9.0));
        content.glyphs.extend(text("A portrait.",330.0,208.5,100.0,9.0));
        let regions=prepare(&mut content);
        assert_eq!(regions.len(),1);
        assert!(regions[0][3]>=217.5);
    }

    #[test]
    fn caption_stops_before_a_full_width_body_line() {
        let mut content=page(false);
        content.glyphs.retain(|g|g.rect[1]<215.0);
        content.glyphs.extend(text("A new body paragraph continues below the entire figure.",50.0,222.0,400.0,9.0));
        let regions=prepare(&mut content);
        assert_eq!(regions.len(),1);
        assert!(regions[0][3]<222.0);
    }
}
