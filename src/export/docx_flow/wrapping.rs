//! Side figures follow the body paragraph instead of creating another column.
use super::super::layout::{self, Align, Figure, Item, Para};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side { Left, Right }

#[derive(Clone)]
pub(super) struct Picture {
    pub figure: Figure,
    pub anchor: usize,
    side: Side,
    gap: f32,
    column: [f32; 2],
    baseline: f32,
}

fn overlaps(line: &layout::Line, r: [f32; 4]) -> bool {
    line.rect[1].max(line.baseline-0.8*line.size) < r[3] - 1.0
        && line.rect[3].min(line.baseline+0.2*line.size) > r[1] + 1.0
}

fn same_column(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < 1.0 && (a[1] - b[1]).abs() < 1.0
}

/// Require prose alongside the figure. A caption, an icon, or text in a
/// neighbouring real column must not turn into a page-wide wrapping object.
fn candidate(items: &[Item], f: &Figure) -> Option<Picture> {
    let r = f.rect;
    let mut choices = Vec::new();
    for (anchor, item) in items.iter().enumerate() {
        let Item::Para(p) = item else { continue };
        if p.marker.is_some() { continue; }
        let size = p.size;
        // Icon strips and neighbouring figures are one visual row.
        if items.iter().any(|item| matches!(item,Item::Figure(g) if g.rect!=r
            && g.rect[1]<r[3] && g.rect[3]>r[1]
            && (g.rect[0]-r[2]).max(r[0]-g.rect[2]) < 2.5*size)) { continue; }
        if r[2]-r[0] < 2.0*size || r[3]-r[1] < 1.8*size
            || (r[2]-r[0]).max(r[3]-r[1]) < 4.0*size { continue; }
        let beside: Vec<_> = p.lines.iter().filter(|l| overlaps(l, r)).collect();
        let side = if beside.iter().all(|l| l.rect[0] >= r[2] + 1.0) { Side::Left }
            else if beside.iter().all(|l| l.rect[2] <= r[0] - 1.0) { Side::Right }
            else { continue };
        if beside.is_empty() || p.rect[1] > r[1] + size || p.rect[3] < r[1] { continue; }
        // Shortened lines can already have been split into individual
        // paragraphs by the general PDF analyser. Judge the whole lane.
        let beside: Vec<_> = items.iter().filter_map(|item| match item {
            Item::Para(q) if q.marker.is_none() && same_column(q.column,p.column)
                && (q.size-size).abs()<0.15*size => Some(q), _=>None,
        }).flat_map(|q|&q.lines).filter(|l| overlaps(l,r) && match side {
            Side::Left => l.rect[0]>=r[2]+1.0, Side::Right=>l.rect[2]<=r[0]-1.0,
        }).collect();
        if beside.len()<2 || beside.iter().map(|l|l.glyphs.len()).sum::<usize>()<40 { continue; }
        let gap = match side {
            Side::Left => beside.iter().map(|l| l.rect[0]-r[2]).fold(f32::INFINITY, f32::min),
            Side::Right => beside.iter().map(|l| r[0]-l.rect[2]).fold(f32::INFINITY, f32::min),
        };
        if !(1.0..=2.5*size).contains(&gap) { continue; }
        let column = [p.column[0].min(f.column[0]).min(r[0]), p.column[1].max(f.column[1]).max(r[2])];
        // A second lane, even a short label, cannot be represented by
        // one-sided wrapping without changing its reading order.
        if items.iter().any(|item| matches!(item,Item::Para(q) if q.lines.iter().any(|l|
            overlaps(l,r) && l.rect[0]>=column[0]-1.0 && l.rect[2]<=column[1]+1.0
                && match side {Side::Left=>l.rect[2]<=r[0]-1.0,Side::Right=>l.rect[0]>=r[2]+1.0}))) { continue; }
        let width = column[1]-column[0];
        let lane = match side { Side::Left => column[1]-r[2]-gap, Side::Right => r[0]-gap-column[0] };
        if lane < 8.0*size || r[2]-r[0] > 0.65*width { continue; }
        // Nearby text spanning both lanes proves they belong to one body.
        let bridge = items.iter().any(|item| matches!(item, Item::Para(q)
            if (q.size-size).abs() < 0.15*size && q.lines.iter().any(|l|
                l.rect[0] < r[0]+size && l.rect[2] > r[2]-size
                && l.rect[2]-l.rect[0] > 0.75*width
                && (l.rect[1]-r[3]).abs().min((l.rect[3]-r[1]).abs()) < 4.0*p.pitch())));
        // A figure's own text column, including an external caption, needs
        // the existing column/inline layout unless the body spans both lanes.
        let separate_text = items.iter().any(|item| matches!(item, Item::Para(q)
            if q.rect[2] > r[0]+size && q.rect[0] < r[2]-size
                && q.rect[2]-q.rect[0] < 0.75*width
                && q.rect[1] >= r[1]-size && q.rect[1] < r[3]+4.0*p.pitch()));
        if separate_text && !bridge { continue; }
        // Even with a full-width heading, prose below the picture in its
        // own lane is evidence of actual columns, not temporary wrapping.
        if items.iter().any(|item| matches!(item, Item::Para(q)
            if q.lines.len() >= 3 && q.rect[2] > r[0]+size && q.rect[0] < r[2]-size
                && q.rect[2]-q.rect[0] < 0.7*width
                && q.rect[1] >= r[3]-size && (q.size-size).abs() < 0.15*size)) { continue; }
        choices.push(Picture { figure: f.clone(), anchor, side, gap, column, baseline: p.lines[0].baseline });
    }
    // Two independent text lanes beside one object are ambiguous.
    let first = choices.first()?;
    if choices.iter().any(|p| p.side != first.side || !same_column(p.column,first.column)) { return None; }
    choices.into_iter().min_by(|a,b| a.baseline.total_cmp(&b.baseline))
}

/// Restore the space occupied by a figure only for paragraph analysis.
/// Glyphs, baselines, first-line indents and ragged line ends stay intact.
pub(super) fn paragraph<'a>(p: &'a Para, pictures: &[Picture]) -> std::borrow::Cow<'a, Para> {
    if !pictures.iter().any(|f| same_column(f.column,p.column) && p.lines.iter().any(|l|overlaps(l,f.figure.rect))) {
        return std::borrow::Cow::Borrowed(p);
    }
    let mut q = p.clone();
    for picture in pictures.iter().filter(|f| same_column(f.column,p.column)) {
        let r = picture.figure.rect;
        for line in &mut q.lines {
            if !overlaps(line,r) { continue; }
            match picture.side {
                Side::Left if line.rect[0] >= r[2] => line.rect[0] = (line.rect[0] - (r[2]+picture.gap-p.column[0])).max(p.column[0]),
                Side::Right if line.rect[2] <= r[0] => line.rect[2] = (line.rect[2] + (p.column[1]-r[0]+picture.gap)).min(p.column[1]),
                _ => {},
            }
        }
    }
    q.rect = q.lines.iter().map(|l|l.rect).reduce(layout::union).unwrap_or(q.rect);
    q.align = layout::alignment(&q.lines,q.column,q.size);
    std::borrow::Cow::Owned(q)
}

fn continues(a: &Para, b: &Para, pictures: &[Picture]) -> bool {
    if !same_column(a.column,b.column) || b.marker.is_some() || a.marker.is_some()
        || (a.size-b.size).abs() > 0.1*a.size { return false; }
    let last = a.lines.last().unwrap();
    let first = &b.lines[0];
    let main = |line: &layout::Line| line.glyphs.iter().max_by(|a,b|a.size.total_cmp(&b.size)).map(|g|(g.font,g.color));
    if main(last)!=main(first) {return false;}
    let pitch = a.pitch().min(b.pitch());
    if first.baseline-last.baseline < 0.7*a.size || first.baseline-last.baseline > 1.3*pitch { return false; }
    let near = pictures.iter().any(|f| same_column(f.column,a.column)
        && (overlaps(last,f.figure.rect) || overlaps(first,f.figure.rect)));
    if !near { return false; }
    let na = paragraph(a,pictures);
    let nb = paragraph(b,pictures);
    let last = na.lines.last().unwrap();
    // A short closing line or a new first-line indent is a real paragraph.
    na.column[1]-last.rect[2] < 1.5*a.size
        && (nb.lines[0].rect[0]-na.column[0]).abs() < 0.7*a.size
        && !matches!(na.align,Align::Center|Align::Right)
        && !matches!(nb.align,Align::Center|Align::Right)
}

pub(super) fn prepare(mut items: Vec<Item>, content: &crate::export::PageContent) -> (Vec<Item>, Vec<Picture>) {
    let mut pictures = Vec::new();
    let mut removed = std::collections::HashSet::new();
    for (index,item) in items.iter().enumerate() {
        let Item::Figure(f) = item else { continue };
        // Text-backed artwork (code shading, legends, labelled diagrams)
        // retains its existing group instead of becoming a prose obstacle.
        let labels = content.glyphs.iter().filter(|g| !g.ch.is_whitespace() && !g.invisible
            && (g.rect[0]+g.rect[2])/2.0>=f.rect[0] && (g.rect[0]+g.rect[2])/2.0<=f.rect[2]
            && (g.rect[1]+g.rect[3])/2.0>=f.rect[1] && (g.rect[1]+g.rect[3])/2.0<=f.rect[3]).take(3).count();
        if labels>=3 { continue; }
        if let Some(picture) = candidate(&items,f) {
            // One paragraph cannot belong to two different wrapping columns.
            // Overlapping figures would also leave an unusably narrow lane.
            if pictures.iter().any(|p: &Picture| p.column[0]<picture.column[1] && p.column[1]>picture.column[0]
                && (!same_column(p.column,picture.column)
                    || p.figure.rect[1]<f.rect[3] && p.figure.rect[3]>f.rect[1])) { continue; }
            pictures.push(picture);
            removed.insert(index);
        }
    }
    if pictures.is_empty() { return (items,pictures); }
    for item in &mut items {
        let Item::Para(p) = item else { continue };
        for picture in &pictures {
            let r = picture.figure.rect;
            let beside = p.lines.iter().filter(|l|overlaps(l,r)).all(|l| match picture.side { Side::Left => l.rect[0] >= r[2], Side::Right => l.rect[2] <= r[0] });
            if beside && p.lines.iter().any(|l|overlaps(l,r))
                && p.rect[0] >= picture.column[0]-1.0 && p.rect[2] <= picture.column[1]+1.0 {
                p.column = picture.column;
            }
        }
    }
    let mut merged: Vec<Item> = Vec::new();
    for (index,item) in items.into_iter().enumerate() {
        if removed.contains(&index) { continue; }
        if let Item::Para(p) = &item
            && let Some(Item::Para(last)) = merged.last_mut()
            && continues(last,p,&pictures)
        {
            last.rect = layout::union(last.rect,p.rect);
            last.lines.extend(p.lines.iter().cloned());
            continue;
        }
        merged.push(item);
    }
    for picture in &mut pictures {
        picture.anchor = merged.iter().position(|item| matches!(item,Item::Para(p)
            if same_column(p.column,picture.column) && p.lines.iter().any(|l| (l.baseline-picture.baseline).abs()<0.1))).expect("wrapping anchor is preserved");
    }
    (merged,pictures)
}

pub(super) fn write(x: &mut String, picture: &super::FloatingPicture, top: f32, before: f32, out: &mut super::Out) {
    use std::fmt::Write;
    let p = &picture.placement;
    let r = p.figure.rect;
    let (cx,cy) = (super::emu(r[2]-r[0]),super::emu(r[3]-r[1]));
    let left = super::emu(r[0]-p.column[0]);
    // Word measures the paragraph anchor before its space-before, while
    // the PDF text box starts at the first line after that space.
    let y = super::emu(r[1]-top+before);
    let gap = super::emu(p.gap);
    let (dl,dr,side) = match p.side { Side::Left => (0,gap,"right"), Side::Right => (gap,0,"left") };
    out.doc_pr += 1;
    let n = out.doc_pr;
    let _ = write!(x,r#"<w:r><w:drawing><wp:anchor distT="0" distB="0" distL="{dl}" distR="{dr}" simplePos="0" relativeHeight="{n}" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="column"><wp:posOffset>{left}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>{y}</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapSquare wrapText="{side}"/><wp:docPr id="{n}" name="Wrapped picture {n}"/>"#);
    super::overlay::graphic(x,picture.rel,n,[cx,cy],&picture.region,out);
    x.push_str("</wp:anchor></w:drawing></w:r>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::Glyph;

    fn prepare(items: Vec<Item>) -> (Vec<Item>,Vec<Picture>) {
        super::prepare(items,&Default::default())
    }

    fn prose(left: f32, right: f32, baseline: f32, count: usize, column: [f32;2]) -> Para {
        let text = "Ordinary editable body text continues beside the illustration";
        let lines: Vec<_> = (0..count).map(|i| {
            let baseline = baseline+14.0*i as f32;
            let advance = (right-left)/text.len() as f32;
            let glyphs = text.chars().enumerate().map(|(j,ch)| {
                let x = left+advance*j as f32;
                Glyph { source:-1, ch, rect:[x,baseline-10.0,x+advance,baseline+2.0], x,baseline,size:10.0,font:0,color:[0;3],space_before:false,invisible:false }
            }).collect();
            layout::Line { glyphs,rect:[left,baseline-10.0,right,baseline+2.0],baseline,size:10.0 }
        }).collect();
        let rect = lines.iter().map(|l|l.rect).reduce(layout::union).unwrap();
        Para { lines,rect,column,align:Align::Justify,marker:None,size:10.0 }
    }

    fn page(left: bool) -> Vec<Item> {
        let f = Figure {rect:if left {[50.0,90.0,170.0,148.0]} else {[330.0,90.0,450.0,148.0]}, column:if left {[50.0,170.0]} else {[330.0,450.0]},image:Some(0)};
        let p = prose(if left {182.0} else {50.0},if left {450.0} else {318.0},100.0,4,if left {[182.0,450.0]} else {[50.0,318.0]});
        let body = Item::Para(prose(50.0,450.0,156.0,3,[50.0,450.0]));
        if left {vec![Item::Figure(f),Item::Para(p),body]} else {vec![Item::Para(p),Item::Figure(f),body]}
    }

    #[test]
    fn side_figures_restore_one_body_paragraph_in_both_reading_orders() {
        for left in [true,false] {
            let (items,pictures) = prepare(page(left));
            assert_eq!(pictures.len(),1);
            assert_eq!(items.len(),1);
            assert_eq!(pictures[0].anchor,0);
            assert_eq!(pictures[0].gap,12.0);
            let Item::Para(p) = &items[0] else {panic!()};
            assert_eq!(p.lines.len(),7);
            assert_eq!(p.column,[50.0,450.0]);
            let normalized = paragraph(p,&pictures);
            let doc = super::super::doc_para(&normalized,None);
            assert_eq!((doc.left,doc.right,doc.first),(0.0,0.0,0.0));
            assert_eq!(doc.align,Align::Justify);
            assert_eq!(p.lines[0].glyphs,normalized.lines[0].glyphs);
            assert_eq!(doc.spans.iter().map(|s|s.text.matches("Ordinary").count()).sum::<usize>(),7);
        }
    }

    #[test]
    fn page_analysis_finds_wrapping_without_a_second_column() {
        for left in [true,false] {
            let source=page(left);
            let content=crate::export::PageContent {
                size:[500.0,400.0],
                glyphs:source.iter().flat_map(|item| match item {Item::Para(p)=>p.lines.iter().flat_map(|l|l.glyphs.clone()).collect(),_=>Vec::new()}).collect(),
                graphics:source.iter().filter_map(|item| match item {Item::Figure(f)=>Some(crate::export::Graphic {rect:f.rect,kind:crate::export::GraphicKind::Image,object:Some(0)}),_=>None}).collect(),
                ..Default::default()
            };
            let (items,_)=super::super::flowing_items(&content);
            let (items,pictures)=super::prepare(items,&content);
            assert_eq!(pictures.len(),1,"left={left}; {:?}",items.iter().map(|i| match i {Item::Para(p)=>(p.rect,p.column,p.lines.len()),Item::Figure(f)=>(f.rect,f.column,0)}).collect::<Vec<_>>());
            assert!(layout::segments(&items,&content.graphics).iter().all(|s|s.columns.len()==1));
            assert_eq!(items.iter().map(|item|match item {Item::Para(p)=>p.lines.len(),_=>0}).sum::<usize>(),7);
        }
    }

    #[test]
    fn actual_columns_and_external_captions_keep_the_inline_layout() {
        let mut items = page(true);
        items.insert(1,Item::Para(prose(50.0,170.0,164.0,5,[50.0,170.0])));
        assert!(prepare(items).1.is_empty());
        let mut items = page(true);
        items.pop();
        items.insert(1,Item::Para(prose(50.0,170.0,164.0,1,[50.0,170.0])));
        assert!(prepare(items).1.is_empty());
    }

    #[test]
    fn paragraph_gaps_indents_and_short_closing_lines_are_preserved() {
        for mode in 0..3 {
            let mut items = page(false);
            if let Item::Para(p) = &mut items[2] {
                if mode==0 {
                    for line in &mut p.lines {line.baseline+=12.0;line.rect[1]+=12.0;line.rect[3]+=12.0;}
                    p.rect[1]+=12.0;p.rect[3]+=12.0;
                } else if mode==1 {p.lines[0].rect[0]+=20.0;}
            }
            if mode==2 && let Item::Para(p)=&mut items[0] {p.lines.last_mut().unwrap().rect[2]-=35.0;}
            let (items,pictures)=prepare(items);
            assert_eq!(pictures.len(),1);
            assert_eq!(items.len(),2,"mode {mode}");
        }
    }

    #[test]
    fn a_short_caption_or_a_tiny_icon_does_not_create_a_float() {
        let mut items=page(true);
        if let Item::Para(p)=&mut items[1] {p.lines.truncate(1);p.rect[3]=p.lines[0].rect[3];}
        assert!(prepare(items).1.is_empty());
        let mut items=page(true);
        if let Item::Figure(f)=&mut items[0] {f.rect[3]=f.rect[1]+8.0;}
        assert!(prepare(items).1.is_empty());
        let mut items=page(true);
        if let Item::Figure(f)=&mut items[0] {f.rect=[140.0,90.0,170.0,120.0];}
        assert!(prepare(items).1.is_empty());
    }

    #[test]
    fn stacked_figures_with_different_columns_do_not_lose_their_anchor() {
        let mut body=prose(182.0,450.0,100.0,4,[182.0,450.0]);
        let lower=prose(182.0,450.0,212.0,4,[182.0,450.0]);
        body.rect=layout::union(body.rect,lower.rect);
        body.lines.extend(lower.lines);
        let items=vec![
            Item::Figure(Figure {rect:[50.0,90.0,170.0,148.0],column:[50.0,170.0],image:Some(0)}),
            Item::Para(body),
            Item::Figure(Figure {rect:[40.0,202.0,170.0,260.0],column:[40.0,170.0],image:Some(1)}),
        ];
        let (items,pictures)=prepare(items);
        assert_eq!(pictures.len(),1);
        assert!(matches!(items[pictures[0].anchor],Item::Para(_)));
        assert_eq!(items.iter().filter(|i|matches!(i,Item::Figure(_))).count(),1);
    }

    #[test]
    fn short_labels_on_the_far_side_and_text_backed_artwork_stay_inline() {
        let mut items=page(true);
        if let Item::Figure(f)=&mut items[0] {f.rect=[190.0,90.0,300.0,148.0];}
        items[1]=Item::Para(prose(50.0,178.0,100.0,4,[50.0,450.0]));
        items.insert(2,Item::Para(prose(312.0,450.0,100.0,1,[50.0,450.0])));
        assert!(prepare(items).1.is_empty());
        let mut items=page(false);
        items.push(Item::Figure(Figure {rect:[462.0,90.0,482.0,148.0],column:[462.0,482.0],image:Some(1)}));
        assert!(prepare(items).1.is_empty());
        let items=page(true);
        let content=crate::export::PageContent {glyphs:prose(60.0,160.0,110.0,1,[50.0,170.0]).lines.remove(0).glyphs,..Default::default()};
        assert!(super::prepare(items,&content).1.is_empty());
    }

    #[test]
    fn floating_picture_shares_the_body_anchor_and_wraps_on_the_text_side() {
        let fonts=[crate::export::FontInfo {name:"Arial".into(),..Default::default()}];
        for left in [true,false] {
            let (items,pictures)=prepare(page(left));
            let Item::Para(p)=&items[0] else {panic!()};
            let mut doc=super::super::doc_para(&paragraph(p,&pictures),None);
            let placement=pictures[0].clone();
            doc.floating.push(super::super::FloatingPicture {rel:1,region:super::super::overlay::Region {rect:placement.figure.rect,lines:Vec::new()},placement});
            let mut lookup=crate::fonts::Fonts::default();
            let mut faces=crate::export::Faces::default();
            let mut out=super::super::Out {fonts:&fonts,lookup:&mut lookup,faces:&mut faces,doc_pr:0,scales:Default::default(),margin_left:50.0,textboxes:crate::export::docx::TextBoxes::new()};
            let mut xml=String::new();
            super::super::block_xml(&mut xml,&super::super::Block::Para(doc),&mut super::super::Prev {baseline:Some(74.0),bottom:Some(76.0)},400.0,false,&|_|None,&mut out);
            assert_eq!(xml.matches("<w:p>").count(),1);
            assert_eq!(xml.matches("<wp:anchor ").count(),1);
            assert!(xml.contains(if left {r#"wrapText="right""#} else {r#"wrapText="left""#}));
            assert!(xml.contains(r#"positionV relativeFrom="paragraph"><wp:posOffset>152400</wp:posOffset>"#));
            assert!(xml.contains(r#"positionH relativeFrom="column""#));
            assert!(!xml.contains("wp:inline") && !xml.contains("framePr"));
        }
    }
}
