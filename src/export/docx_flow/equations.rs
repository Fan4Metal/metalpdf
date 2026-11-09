//! Fixed compositions for mathematical notation inside a flowing document.
use crate::export::{FontInfo,Glyph,PageContent,layout};
type Rect=[f32;4];

#[derive(Default)]
pub(super) struct Prepared {
    pub blocks: Vec<Rect>,
    /// In prose that stays a paragraph: what goes in its lines as small
    /// pictures (a letter with an accent over it, a symbol of a legacy
    /// font), the rest of the expression being text.
    pub inline: Vec<super::artwork::Inline>,
}

/// A character of a legacy TeX or symbol font that is safe as text in
/// a Word font: letters, digits and the common signs. Others (big
/// operators, arrows of odd fonts, glyphs mapped to private codes) keep
/// their drawn shape as a picture.
fn plain(c: char, font: &FontInfo) -> bool {
    // Not the modifier letters and combining marks: accents set apart.
    let accent = matches!(c, '\u{2b0}'..='\u{36f}');
    // The letters of the symbol fonts are script, fraktur, blackboard:
    // `H` in CMSY is ℋ, `R` in MSBM is ℝ.
    let letters_drawn_otherwise = ["cmsy", "cmbsy", "rsfs", "eufm", "msbm", "msam"].iter().any(|p| name(font).starts_with(p));
    !accent && (c.is_alphanumeric() && !(letters_drawn_otherwise && c.is_alphabetic()) || "()[]{},.;:=+-−'′/|<>*!?·×±∞→←↔≤≥≠≈∼∈∂∇⟨⟩".contains(c))
}

/// Prose with expressions in it, not an expression: mostly letters of
/// the body, a few symbols. It stays an ordinary paragraph.
fn prose(p: &layout::Para, math: &dyn Fn(&Glyph) -> bool) -> bool {
    let glyphs: Vec<&Glyph> = p.lines.iter().flat_map(|l| &l.glyphs).collect();
    let symbols = glyphs.iter().filter(|g| math(g)).count();
    let letters = glyphs.iter().filter(|g| !math(g) && g.ch.is_alphabetic()).count();
    letters >= 25 && symbols as f32 <= 0.3 * glyphs.len() as f32
}

/// The pictures prose with expressions needs: characters drawn over one
/// another (an accent, a small fraction), each pile one picture, and the
/// characters of legacy fonts that are not `plain`.
fn inline_pieces(p: &layout::Para, raw: &[Glyph], fonts: &[FontInfo]) -> Vec<super::artwork::Inline> {
    // Every character with the line it is on (an accent drawn apart can
    // be a line of its own).
    // The page's own characters (the lines dropped the accents that make
    // no character with their letter), each with the line it is on.
    let line_of = |g: &Glyph| {
        let c = [(g.rect[0] + g.rect[2]) / 2.0, (g.rect[1] + g.rect[3]) / 2.0];
        // The line it is in, else the nearest within a third of an em: a
        // superscript's centre is within that of the line above too.
        let off = |l: &layout::Line| (l.rect[1] - c[1]).max(c[1] - l.rect[3]).max(0.0);
        (0..p.lines.len())
            .filter(|&k| {
                let l = &p.lines[k];
                c[0] >= l.rect[0] - 0.5 && c[0] <= l.rect[2] + 0.5 && off(l) <= 0.3 * l.size
            })
            .min_by(|&a, &b| off(&p.lines[a]).total_cmp(&off(&p.lines[b])))
    };
    let gs: Vec<(&Glyph, usize)> = raw.iter().filter(|g| !g.invisible && !g.ch.is_whitespace()).filter_map(|g| line_of(g).map(|k| (g, k))).collect();
    let width = |g: &Glyph| g.rect[2] - g.rect[0];
    let mut pile: Vec<usize> = (0..gs.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for i in 0..gs.len() {
        for j in i + 1..gs.len() {
            let (a, b) = (gs[i].0, gs[j].0);
            let narrow = width(a).min(width(b));
            let shared = a.rect[2].min(b.rect[2]) - a.rect[0].max(b.rect[0]);
            // On one line: the characters of neighbouring lines stand over one another too.
            let near = gs[i].1 == gs[j].1;
            if narrow > 0.5 && shared > 0.5 * narrow && near && !(a.ch == b.ch && (a.x - b.x).abs() < 0.5) {
                let (x, y) = (find(&mut pile, i), find(&mut pile, j));
                pile[x] = y;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for i in 0..gs.len() {
        let root = find(&mut pile, i);
        groups.entry(root).or_default().push(i);
    }
    // Each picture with the line it sits on: the longest line among its
    // characters' (an accent's own line is a character long).
    let mut pieces: Vec<(Rect, usize)> = Vec::new();
    let odd = |g: &Glyph| fonts.get(g.font as usize).is_some_and(|f| painted(f) && !plain(g.ch, f));
    for members in groups.values() {
        // A letter and its accent that make a character (`е` and `¨` are
        // `ё`) are text: the lines join them.
        let mut pair: Vec<Glyph> = members.iter().map(|&i| gs[i].0.clone()).collect();
        let base: Vec<char> = pair.iter().map(|g| g.ch).collect();
        layout::join_accents(&mut pair);
        if members.len() >= 2 && pair.len() == 1 && !base.contains(&pair[0].ch) {
            continue;
        }
        if members.len() >= 2 || odd(gs[members[0]].0) {
            let rect = members.iter().map(|&i| gs[i].0.rect).reduce(layout::union).unwrap();
            let line = members.iter().map(|&i| gs[i].1).max_by_key(|&k| p.lines[k].glyphs.len()).unwrap();
            pieces.push((rect, line));
        }
    }
    // Neighbours on a line as one picture.
    pieces.sort_by(|a, b| a.1.cmp(&b.1).then(a.0[0].total_cmp(&b.0[0])));
    let mut merged: Vec<(Rect, usize)> = Vec::new();
    for (r, k) in pieces {
        match merged.last_mut() {
            Some((last, lk)) if *lk == k && r[0] - last[2] < 0.15 * p.lines[k].size => *last = layout::union(*last, r),
            _ => merged.push((r, k)),
        }
    }
    merged.into_iter().map(|(r, k)| super::artwork::Inline { rect: [r[0] - 0.3, r[1] - 0.3, r[2] + 0.3, r[3] + 0.3], baseline: p.lines[k].baseline }).collect()
}

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

/// Small fractions set in a line of text (`⅔` as a numerator over a bar
/// over a denominator, in a smaller size): out of the content, each one
/// picture on the line's baseline. Left in, the numerator went to the
/// line and the denominator began a line of its own with the text after
/// it, the lines crowded together made a composition of the text between
/// two fractions, and the paragraph broke round it.
fn stacked_fractions(content:&mut PageContent,body_size:f32)->Vec<super::artwork::Inline> {
    // Runs of small characters on one baseline.
    let mut small:Vec<(usize,&Glyph)>=content.glyphs.iter().enumerate().filter(|(_,g)|!g.invisible&&!g.ch.is_whitespace()&&g.size<=0.85*body_size).collect();
    small.sort_by(|a,b|a.1.baseline.total_cmp(&b.1.baseline).then(a.1.x.total_cmp(&b.1.x)));
    let mut runs:Vec<(Rect,f32,f32,Vec<usize>)>=Vec::new();
    for (at,g) in small {
        match runs.iter_mut().rev().take_while(|r|g.baseline-r.1<0.15*g.size).find(|r|g.rect[0]-r.0[2]<0.5*g.size&&r.0[0]-g.rect[2]<0.5*g.size) {
            Some(r)=>{r.0=layout::union(r.0,g.rect);r.2=r.2.max(g.size);r.3.push(at);}
            None=>runs.push((g.rect,g.baseline,g.size,vec![at])),
        }
    }
    let mut out=Vec::new();
    let mut taken:Vec<usize>=Vec::new();
    let mut bars:Vec<usize>=Vec::new();
    for (b,bar) in content.graphics.iter().enumerate() {
        let r=bar.rect;
        let y=(r[1]+r[3])/2.0;
        if r[3]-r[1]>1.2||r[2]-r[0]<2.0||r[2]-r[0]>8.0*body_size {continue;}
        let over=|run:&(Rect,f32,f32,Vec<usize>)| {let c=(run.0[0]+run.0[2])/2.0;c>=r[0]-1.0&&c<=r[2]+1.0&&run.0[2]-run.0[0]<=r[2]-r[0]+2.0};
        let num:Vec<usize>=(0..runs.len()).filter(|&k|over(&runs[k])&&runs[k].1<=y+0.5&&y-runs[k].1<0.7*runs[k].2).collect();
        let den:Vec<usize>=(0..runs.len()).filter(|&k|over(&runs[k])&&runs[k].1>y&&runs[k].1-y<1.3*runs[k].2).collect();
        if num.is_empty()||den.is_empty() {continue;}
        let top=num.iter().map(|&k|runs[k].1).fold(f32::MAX,f32::min);
        let bottom=den.iter().map(|&k|runs[k].1).fold(f32::MIN,f32::max);
        // Text of the body's size going through it: the line it is in.
        let line=content.glyphs.iter()
            .filter(|g|!g.invisible&&g.size>0.85*body_size&&g.baseline>top&&g.baseline<bottom)
            .filter(|g|g.rect[2]>r[0]-3.0*body_size&&g.rect[0]<r[2]+3.0*body_size)
            .min_by(|a,b|{let d=|g:&Glyph|(g.rect[0]-r[2]).max(r[0]-g.rect[2]).max(0.0);d(a).total_cmp(&d(b))});
        let Some(line)=line else {continue};
        let members:Vec<usize>=num.iter().chain(&den).flat_map(|&k|runs[k].3.iter().copied()).collect();
        if members.iter().any(|m|taken.contains(m)) {continue;}
        let rect=members.iter().map(|&m|content.glyphs[m].rect).fold(r,layout::union);
        let first=members.iter().map(|&m|&content.glyphs[m]).min_by(|a,b|a.x.total_cmp(&b.x)).unwrap();
        let rect=[rect[0]-0.3,rect[1]-0.3,rect[2]+0.3,rect[3]+0.3];
        // A character of the line holds its place until the picture takes
        // it (`artwork::prepare` drops what is inside a picture): the hole
        // it left parted the line into two, read as columns.
        let mut stand_in=line.clone();
        stand_in.ch='0';
        // A picture's mark to the lines (it goes into holes), and none of
        // the pictures' indices.
        stand_in.source = -1_000_000;
        stand_in.x=rect[0]+0.3;
        stand_in.rect=[rect[0]+0.3,line.baseline-0.7*line.size,rect[2]-0.3,line.baseline+0.2*line.size];
        stand_in.space_before=first.space_before;
        out.push((super::artwork::Inline {rect,baseline:line.baseline},stand_in));
        taken.extend(members);
        bars.push(b);
    }
    let mut k=0;
    content.glyphs.retain(|_|{k+=1;!taken.contains(&(k-1))});
    content.glyphs.extend(out.iter().map(|(_,g)|g.clone()));
    let out:Vec<_>=out.into_iter().map(|(f,_)|f).collect();
    let mut k=0;
    content.graphics.retain(|_|{k+=1;!bars.contains(&(k-1))});
    out
}

/// Preserve a mathematical paragraph as one composition. A tall fraction
/// must not redefine the font or line pitch of the surrounding prose.
pub(super) fn prepare(content:&mut PageContent,fonts:&[FontInfo])->Prepared {
    let mut counts=std::collections::HashMap::new();
    for g in content.glyphs.iter().filter(|g|g.ch.is_alphabetic()&&!g.invisible) {*counts.entry(g.font).or_insert(0usize)+=1;}
    let body=counts.into_iter().max_by_key(|&(f,n)|(n,std::cmp::Reverse(f))).map(|(f,_)|f);
    let mut sizes=std::collections::HashMap::new();
    for g in content.glyphs.iter().filter(|g|g.ch.is_alphabetic()&&!g.invisible&&Some(g.font)==body) {*sizes.entry((g.size*2.0).round() as i32).or_insert(0usize)+=1;}
    let body_size=sizes.into_iter().max_by_key(|&(s,n)|(n,s)).map_or(10.0,|(s,_)|s as f32/2.0);
    let fractions=stacked_fractions(content,body_size);
    let math=|g:&Glyph| {
        if g.invisible {return false;}
        let font=fonts.get(g.font as usize);
        let mono=font.is_some_and(|f|["courier","consolas","mono","cmtt"].iter().any(|n|name(f).contains(n)));
        font.is_some_and(|f|strong(f)||tex(f)&&!name(f).starts_with("symbol")&&!name(f).starts_with("mtextra")&&Some(g.font)!=body&&!g.ch.is_alphabetic()) || !mono&&symbol(g.ch)
    };
    let excluded:Vec<_>=layout::labelled_figures(content).into_iter().map(|f|f.rect).collect();
    let seeds:Vec<_>=content.glyphs.iter().filter(|g|math(g)&&!excluded.iter().any(|&r|centre_inside(g,r))).map(|g|g.rect).collect();
    if seeds.is_empty() {return Prepared {blocks:Vec::new(),inline:fractions};}
    let all_items=layout::analyse(content,layout::Options {invisible:false,figures:false});
    // Prose with a variable or two in it stays a paragraph that rewraps
    // when edited; only the expressions set apart are fixed compositions.
    let mut inline = Vec::new();
    let mut prose_lines: Vec<Rect> = Vec::new();
    let mut display: Vec<Rect> = Vec::new();
    let items: Vec<&layout::Item> = all_items.iter().filter(|item| match item {
        layout::Item::Para(p) if seeds.iter().any(|r| touches(p.rect, *r, 0.0)) && prose(p, &math) => {
            // Its lines that are an expression set apart (mostly symbols),
            // or the parts of a fraction (lines crowded closer than a
            // line's pitch), stay compositions; the rest flows.
            let n = p.lines.len();
            let set_apart: Vec<bool> = (0..n).map(|k| {
                let l = &p.lines[k];
                let symbols = l.glyphs.iter().filter(|g| math(g)).count();
                let crowded = |o: usize| (p.lines[o].baseline - l.baseline).abs() < 0.8 * p.size;
                symbols >= 2 && 2 * symbols >= l.glyphs.len() || k > 0 && crowded(k - 1) || k + 1 < n && crowded(k + 1)
            }).collect();
            let mut flowing = p.clone();
            flowing.lines = p.lines.iter().zip(&set_apart).filter(|(_, apart)| !**apart).map(|(l, _)| l.clone()).collect();
            display.extend(p.lines.iter().zip(&set_apart).filter(|(_, apart)| **apart).map(|(l, _)| l.rect));
            inline.extend(inline_pieces(&flowing, &content.glyphs, fonts));
            prose_lines.extend(flowing.lines.iter().map(|l| l.rect));
            false
        }
        _ => true,
    }).collect();
    let in_prose = |row: &layout::Line| {
        let c = [(row.rect[0] + row.rect[2]) / 2.0, row.baseline - 0.3 * row.size];
        prose_lines.iter().any(|r| c[0] >= r[0] && c[0] <= r[2] && c[1] >= r[1] && c[1] <= r[3])
    };
    let mut blocks:Vec<Rect>=items.iter().filter(|item| seeds.iter().any(|r|touches(item.rect(),*r,0.0))).map(|item| item.rect()).collect();
    blocks.extend(display);
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
            for row in rows.iter().filter(|row| !in_prose(row)) {
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
    // A fraction in a composition is painted with it.
    for f in fractions {
        match blocks.iter_mut().find(|r|touches(**r,f.rect,0.0)) {
            Some(r)=>*r=layout::union(*r,f.rect),
            None=>inline.push(f),
        }
    }
    content.glyphs.retain(|g|!blocks.iter().any(|&r|centre_inside(g,r)));
    content.graphics.retain(|g|!blocks.iter().any(|r|g.rect[0]>=r[0]&&g.rect[2]<=r[2]&&g.rect[1]>=r[1]&&g.rect[3]<=r[3]));
    Prepared {blocks, inline}
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
        let mut p=PageContent {size:[600.0,800.0],glyphs,graphics:vec![crate::export::Graphic {rect:[138.0,148.0,150.0,149.0],kind:crate::export::GraphicKind::Path {curved:false},object:None}],turns:0,..Default::default()};
        let prepared=prepare(&mut p,&fonts());
        assert_eq!(prepared.blocks.len(),1);
        let r=prepared.blocks[0];assert!(r[0]<=100.0 && r[2]>=525.0 && r[1]<=134.0 && r[3]>=178.0);
        assert_eq!(p.glyphs.iter().map(|g|g.ch).collect::<String>(),"Ordinary prose above the expression.Ordinary prose below the expression.");
        assert!(p.graphics.is_empty());
    }
    #[test]
    fn prose_with_a_variable_stays_a_paragraph_with_only_its_piles_as_pictures() {
        let mut glyphs=text("Ordinary prose before",50.0,100.0,0);
        glyphs.extend(text("α",157.0,100.0,1));
        glyphs.extend(text("and after the variable.",170.0,100.0,0));
        glyphs.extend(text("The same paragraph continues on this line.",50.0,113.0,0));
        glyphs.extend(text("A separate paragraph remains editable.",50.0,180.0,0));
        let mut p=PageContent {size:[600.0,800.0],glyphs, ..Default::default()};
        let original=p.clone();
        let prepared=prepare(&mut p,&fonts());
        // The variable is a letter: text, the paragraph untouched.
        assert!(prepared.blocks.is_empty() && prepared.inline.is_empty());
        assert_eq!(p,original);
        // A hat drawn over a letter of the expression, and a sign of a
        // legacy font with no safe character: pictures in the line.
        let mut hat=text("ˆ",157.0,100.0,1);hat[0].rect[0]+=0.5;hat[0].x+=0.5;
        p.glyphs.extend(hat);
        p.glyphs.extend(text("♮",261.0,113.0,1));
        let prepared=prepare(&mut p,&fonts());
        assert!(prepared.blocks.is_empty());
        assert_eq!(prepared.inline.len(),2,"{:?}",prepared.inline.iter().map(|a|(a.rect,a.baseline)).collect::<Vec<_>>());
        assert!(prepared.inline.iter().any(|a|a.rect[0]<=157.0 && a.rect[2]>=162.0 && a.baseline==100.0));
        assert!(prepared.inline.iter().any(|a|a.rect[0]<=261.0 && a.baseline==113.0));
    }

    #[test]
    fn small_fractions_in_a_line_are_pictures_on_its_baseline() {
        // "charge 2/3 ) and d (charge -1/3 ) quarks", the fractions in 7 pt.
        let small=|s:&str,x:f32,y:f32|{let mut g=text(s,x,y,1);for g in &mut g {g.size=7.0;g.rect=[g.rect[0],y-5.0,g.rect[0]+4.0,y+1.5];}g};
        let mut glyphs=text("Protons and neutrons are made of u (charge",50.0,100.0,0);
        glyphs.extend(small("2",262.0,97.0));glyphs.extend(small("3",262.0,104.0));
        glyphs.extend(text(") and d (charge -",268.0,100.0,0));
        glyphs.extend(small("1",355.0,97.0));glyphs.extend(small("3",355.0,104.0));
        glyphs.extend(text(") quarks",361.0,100.0,0));
        glyphs.extend(text("and the paragraph goes on to its next line here.",50.0,112.0,0));
        let bar=|x:f32|crate::export::Graphic {rect:[x,98.0,x+5.0,98.4],kind:crate::export::GraphicKind::Path {curved:false},object:None};
        let mut p=PageContent {size:[600.0,800.0],glyphs,graphics:vec![bar(261.5),bar(354.5)],..Default::default()};
        let prepared=prepare(&mut p,&fonts());
        assert!(prepared.blocks.is_empty());
        assert_eq!(prepared.inline.len(),2);
        assert!(prepared.inline.iter().all(|f|f.baseline==100.0&&f.rect[3]<108.0));
        assert!(p.graphics.is_empty());
        let rows=layout::lines(p.glyphs);
        assert_eq!(rows.len(),2,"{:?}",rows.iter().map(|r|(r.text(),r.rect)).collect::<Vec<_>>());
        assert!(rows[0].text().contains(") and d (charge -"));
    }

    #[test]
    fn an_expression_set_apart_is_still_one_composition() {
        let mut glyphs=text("Ordinary prose before the expression goes on.",50.0,100.0,0);
        glyphs.extend(text("αβ=γδ+",150.0,130.0,1));
        glyphs.extend(text("A separate paragraph remains editable.",50.0,180.0,0));
        let mut p=PageContent {size:[600.0,800.0],glyphs, ..Default::default()};
        let prepared=prepare(&mut p,&fonts());
        assert_eq!(prepared.blocks.len(),1);
        assert_eq!(p.glyphs.iter().map(|g|g.ch).collect::<String>(),"Ordinary prose before the expression goes on.A separate paragraph remains editable.");
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
