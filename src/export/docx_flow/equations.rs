//! Fixed compositions for mathematical notation inside a flowing document.
use crate::export::{FontInfo,Glyph,PageContent,layout};
type Rect=[f32;4];

#[derive(Default)]
pub(super) struct Prepared {pub blocks:Vec<Rect>}

fn name(font:&FontInfo)->String {font.name.rsplit('+').next().unwrap_or(&font.name).to_ascii_lowercase()}
fn tex(font:&FontInfo)->bool {
    let n=name(font);
    ["cmmi","cmsy","cmex","cmr","cmbx","msam","msbm","eufm","rsfs","stix","math","symbol","mtextra"].iter().any(|p|n.starts_with(p)||n.contains("math"))
}
fn strong(font:&FontInfo)->bool {
    let n=name(font);
    tex(font) && !["cmr","cmbx","cmss","cmtt","symbol","mtextra"].iter().any(|p|n.starts_with(p))
}
fn symbol(c:char)->bool {
    matches!(c,'='|'≠'|'≤'|'≥'|'∑'|'∏'|'∫'|'∂'|'√'|'∞'|'±'|'×'|'⊗'|'∇'|'→'|'↔'|'⇒'|'⇔'|'∈'|'∉'|'∝'|'≈')
        || ('α'..='ω').contains(&c) || ('Α'..='Ω').contains(&c)
}
fn centre_inside(g:&Glyph,r:Rect)->bool {
    let(x,y)=((g.rect[0]+g.rect[2])/2.0,(g.rect[1]+g.rect[3])/2.0);
    x>=r[0]&&x<=r[2]&&y>=r[1]&&y<=r[3]
}
fn touches(a:Rect,b:Rect,pad:f32)->bool {a[0]<=b[2]+pad&&a[2]>=b[0]-pad&&a[1]<=b[3]+pad&&a[3]>=b[1]-pad}

/// Legacy symbol encodings and absent TeX faces cannot be replaced by a
/// generic Word font without changing the expression's meaning.
pub(super) fn painted(font:&FontInfo)->bool {
    let n=name(font);
    ["cmmi","cmsy","cmex","cmr","cmbx","msam","msbm","eufm","rsfs","symbol","mtextra"].iter().any(|p|n.starts_with(p))
}

/// A baseline per row prevents a subscript becoming the next character's
/// advance, which can compress both characters into almost zero width.
pub(super) fn lines(glyphs:Vec<Glyph>)->Vec<layout::Line> {
    let mut sorted=glyphs;sorted.sort_by(|a,b|a.baseline.total_cmp(&b.baseline).then(a.x.total_cmp(&b.x)));
    let mut rows:Vec<layout::Line>=Vec::new();
    for g in sorted {
        if let Some(row)=rows.last_mut().filter(|r|(r.baseline-g.baseline).abs()<0.2) {
            row.rect=layout::union(row.rect,g.rect);row.size=row.size.max(g.size);row.glyphs.push(g);
        } else {rows.push(layout::Line {rect:g.rect,baseline:g.baseline,size:g.size,glyphs:vec![g]});}
    }
    let mut pieces:Vec<layout::Line>=Vec::new();
    for mut row in rows {
        row.glyphs.sort_by(|a,b|a.x.total_cmp(&b.x));
        let mut piece:Option<layout::Line>=None;
        for mut g in row.glyphs {
            if let Some(last)=piece.as_mut() {
                let gap=g.x-last.glyphs.last().unwrap().rect[2];
                if gap>0.7*last.size.max(g.size) {pieces.push(piece.take().unwrap());}
                else if gap>0.18*last.size.min(g.size) {g.space_before=true;}
            }
            if let Some(last)=piece.as_mut() {last.rect=layout::union(last.rect,g.rect);last.size=last.size.max(g.size);last.glyphs.push(g);}
            else {piece=Some(layout::Line {rect:g.rect,baseline:g.baseline,size:g.size,glyphs:vec![g]});}
        }
        if let Some(piece)=piece {pieces.push(piece);}
    }
    pieces.sort_by(|a,b|a.rect[1].total_cmp(&b.rect[1]).then(a.rect[0].total_cmp(&b.rect[0])));
    pieces
}

/// Word must not reflow a mathematical composition's editable rows through
/// the symbols that remain painted in its background.
pub(super) fn editable(region:&mut super::overlay::Region) {
    region.exact=lines(std::mem::take(&mut region.lines).into_iter().flat_map(|l|l.glyphs).collect());
}

/// Preserve a mathematical paragraph as one composition. A tall fraction
/// must not redefine the font or line pitch of the surrounding prose.
pub(super) fn prepare(content:&mut PageContent,fonts:&[FontInfo])->Prepared {
    let mut counts=std::collections::HashMap::new();
    for g in content.glyphs.iter().filter(|g|g.ch.is_alphabetic()&&!g.invisible) {*counts.entry(g.font).or_insert(0usize)+=1;}
    let body=counts.into_iter().max_by_key(|&(f,n)|(n,std::cmp::Reverse(f))).map(|(f,_)|f);
    let math=|g:&Glyph| {
        if g.invisible {return false;}
        let font=fonts.get(g.font as usize);
        let mono=font.is_some_and(|f|["courier","consolas","mono","cmtt"].iter().any(|n|name(f).contains(n)));
        font.is_some_and(|f|strong(f)||tex(f)&&!name(f).starts_with("symbol")&&!name(f).starts_with("mtextra")&&Some(g.font)!=body&&!g.ch.is_alphabetic()) || !mono&&symbol(g.ch)
    };
    let excluded:Vec<_>=layout::labelled_figures(content).into_iter().map(|f|f.rect).collect();
    let seeds:Vec<_>=content.glyphs.iter().filter(|g|math(g)&&!excluded.iter().any(|&r|centre_inside(g,r))).map(|g|g.rect).collect();
    if seeds.is_empty() {return Prepared::default();}
    let items=layout::analyse(content,layout::Options {invisible:false,figures:false});
    let mut blocks:Vec<Rect>=items.iter().filter(|item| seeds.iter().any(|r|touches(item.rect(),*r,0.0))).map(layout::Item::rect).collect();
    // Analyse may place a numerator, its denominator and the equation number
    // into different paragraphs. Include all intersecting rows and artwork
    // before merging so no part can be painted twice or cropped away.
    let rows=layout::lines(content.glyphs.clone());
    loop {
        let previous=blocks.clone();
        for r in &mut blocks {
            for item in &items {
                let row=item.rect();
                if r[1]<=row[3] && r[3]>=row[1] {*r=layout::union(*r,row);}
            }
            for row in &rows {
                let t=row.text();
                let number=t.starts_with('(') && t.ends_with(')') && t.chars().any(|c|c.is_ascii_digit())
                    && t.chars().all(|c|c.is_ascii_digit()||matches!(c,'('|')'|'.'|' '));
                if r[1]<=row.rect[3] && r[3]>=row.rect[1]
                    || number && row.rect[0]>0.55*content.size[0] && row.rect[1]<=r[3]+1.5*row.size && row.rect[3]>=r[1]-row.size {*r=layout::union(*r,row.rect);}
            }
            for g in &content.graphics {
                if g.rect[2]-g.rect[0]<0.9*content.size[0] && g.rect[3]-g.rect[1]<0.8*content.size[1] && touches(*r,g.rect,1.0) {*r=layout::union(*r,g.rect);}
            }
        }
        let mut merged:Vec<Rect>=Vec::new();
        for r in blocks {
            if let Some(last)=merged.iter_mut().find(|last|touches(**last,r,1.0)) {*last=layout::union(*last,r);}
            else {merged.push(r);}
        }
        blocks=merged;
        if blocks==previous {break;}
    }
    for r in &mut blocks {r[0]=(r[0]-0.5).max(0.0);r[1]=(r[1]-1.0).max(0.0);r[2]=(r[2]+0.5).min(content.size[0]);r[3]=(r[3]+1.0).min(content.size[1]);}
    content.glyphs.retain(|g|!blocks.iter().any(|&r|centre_inside(g,r)));
    content.graphics.retain(|g|!blocks.iter().any(|r|g.rect[0]>=r[0]&&g.rect[2]<=r[2]&&g.rect[1]>=r[1]&&g.rect[3]<=r[3]));
    Prepared {blocks}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn text(s:&str,x:f32,y:f32,font:u16)->Vec<Glyph> {
        s.chars().enumerate().map(|(i,ch)|Glyph {source:-1,ch,rect:[x+5.0*i as f32,y-8.0,x+5.0*(i+1) as f32,y+2.0],x:x+5.0*i as f32,baseline:y,size:10.0,font,color:[0;3],space_before:false,invisible:false}).collect()
    }
    fn fonts()->Vec<FontInfo> {vec![FontInfo {name:"Times New Roman".into(),..Default::default()},FontInfo {name:"ABCDEF+CMMI10".into(),..Default::default()}]}
    #[test]
    fn a_fraction_and_its_lower_number_share_one_composition() {
        let mut glyphs=text("Ordinary prose above the expression.",50.0,80.0,0);
        glyphs.extend(text("x=",100.0,150.0,1));
        glyphs.extend(text("a",140.0,142.0,1));
        glyphs.extend(text("b",140.0,158.0,1));
        glyphs.extend(text("(4.3)",500.0,176.0,0));
        glyphs.extend(text("Ordinary prose below the expression.",50.0,230.0,0));
        let mut p=PageContent {size:[600.0,800.0],glyphs,graphics:vec![crate::export::Graphic {rect:[138.0,148.0,150.0,149.0],kind:crate::export::GraphicKind::Path {curved:false},object:None}],turns:0};
        let prepared=prepare(&mut p,&fonts());
        assert_eq!(prepared.blocks.len(),1);
        let r=prepared.blocks[0];assert!(r[0]<=100.0 && r[2]>=525.0 && r[1]<=134.0 && r[3]>=178.0);
        assert_eq!(p.glyphs.iter().map(|g|g.ch).collect::<String>(),"Ordinary prose above the expression.Ordinary prose below the expression.");
        assert!(p.graphics.is_empty());
    }
    #[test]
    fn inline_math_preserves_the_whole_paragraph_and_its_source_baselines() {
        let mut glyphs=text("Ordinary prose before",50.0,100.0,0);
        glyphs.extend(text("α",157.0,100.0,1));
        glyphs.extend(text("and after the variable.",170.0,100.0,0));
        glyphs.extend(text("The same paragraph continues on this line.",50.0,113.0,0));
        glyphs.extend(text("A separate paragraph remains editable.",50.0,180.0,0));
        let mut p=PageContent {size:[600.0,800.0],glyphs, ..Default::default()};
        let prepared=prepare(&mut p,&fonts());
        assert_eq!(prepared.blocks.len(),1);assert!(prepared.blocks[0][3]>=115.0);
        assert_eq!(p.glyphs.iter().map(|g|g.ch).collect::<String>(),"A separate paragraph remains editable.");
    }
    #[test]
    fn monospace_code_is_not_mistaken_for_an_equation() {
        for font in ["Courier New","Consolas","CMTT10"] {
            let mut p=PageContent {size:[600.0,800.0],glyphs:text("value = alpha + beta",50.0,100.0,0),..Default::default()};
            let original=p.clone();let f=FontInfo {name:font.into(),..Default::default()};
            assert!(prepare(&mut p,&[f]).blocks.is_empty());assert_eq!(p,original);
        }
    }
    #[test]
    fn symbol_font_bullets_do_not_freeze_ordinary_lists() {
        let mut p=PageContent {size:[600.0,800.0],glyphs:text("•",40.0,100.0,1),..Default::default()};
        p.glyphs.extend(text("Ordinary text in a bulleted list",50.0,100.0,0));
        let original=p.clone();let f=vec![FontInfo {name:"Arial".into(),..Default::default()},FontInfo {name:"Symbol".into(),..Default::default()}];
        assert!(prepare(&mut p,&f).blocks.is_empty());assert_eq!(p,original);
    }
    #[test]
    fn equation_rows_do_not_stretch_across_numbers_or_subscripts() {
        let mut glyphs=text("xy",50.0,100.0,1);glyphs.extend(text("1",55.0,104.0,1));glyphs.extend(text("(1)",500.0,100.0,0));
        let rows=lines(glyphs);assert_eq!(rows.len(),3);
        let main=rows.iter().find(|l|l.text()=="xy").unwrap();assert_eq!(main.rect[2]-main.rect[0],10.0);
        assert_eq!(rows.iter().find(|l|l.text()=="1").unwrap().baseline,104.0);
        assert!(painted(&fonts()[1]));assert!(!painted(&FontInfo {name:"Cambria Math".into(),..Default::default()}));
    }
    #[test]
    #[ignore = "needs vendor/pdfium"]
    fn pdfium_equation_number_and_prose_survive_fragmented_source_rows() {
        let stream="BT /F 10 Tf 40 720 Td (Ordinary editable paragraph before the formula.) Tj ET BT /F 12 Tf 100 650 Td (x = y) Tj ET BT /F 10 Tf 140 659 Td (a) Tj ET BT /F 10 Tf 140 642 Td (b) Tj ET 138 653 15 1 re f BT /F 10 Tf 500 630 Td (\\(4.3\\)) Tj ET BT /F 10 Tf 40 550 Td (Ordinary editable paragraph after the formula.) Tj ET";
        let bytes=crate::pdfium::tests::write_pdf(vec!["<< /Type /Catalog /Pages 2 0 R >>".into(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R /Resources << /Font << /F 5 0 R >> >> >>".into(),format!("<< /Length {} >>\nstream\n{stream}\nendstream",stream.len()),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()]);
        let input=std::env::temp_dir().join(format!("metalpdf_equation_{}.pdf",std::process::id()));let output=input.with_extension("docx");std::fs::write(&input,bytes).unwrap();
        crate::export::run_paragraphs(&input,&output,None).unwrap();let bytes=std::fs::read(&output).unwrap();
        let xml=String::from_utf8(crate::export::zip::read_entry(&bytes,"word/document.xml").unwrap()).unwrap();
        let plain:String=xml.split("<w:t xml:space=\"preserve\">").skip(1).map(|s|s.split("</w:t>").next().unwrap()).collect();
        assert_eq!(plain.matches("(4.3)").count(),1,"{plain}");assert_eq!(plain.matches("x = y").count(),1);
        assert_eq!(plain.matches("Ordinary editable paragraph before the formula.").count(),1);assert_eq!(plain.matches("Ordinary editable paragraph after the formula.").count(),1);
        assert_eq!(xml.matches("<wpg:wgp ").count(),1);assert!(!xml.contains("<wp:wrapSquare"));
        std::fs::remove_file(input).unwrap();std::fs::remove_file(output).unwrap();
    }

}
