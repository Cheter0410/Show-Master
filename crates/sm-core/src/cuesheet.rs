//! Cue sheet export for stage management and crew: CSV (spreadsheets) and a
//! printable PDF (A4 landscape), both generated here without any library.
//!
//! The PDF uses the standard Helvetica fonts every PDF reader has (no font
//! embedding) with WinAnsi encoding, so umlauts and other Western European
//! characters print; anything else becomes `?`.

use crate::model::{ContinueMode, Cue, CueKind, Show, TcAction};

/// One printed line.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetRow {
    pub list: String,
    /// Nesting depth (children of groups are indented).
    pub depth: usize,
    pub number: String,
    pub name: String,
    pub kind: &'static str,
    pub action: String,
    pub duration: String,
    pub pre_wait: String,
    pub post_wait: String,
    pub cont: &'static str,
    pub tc: String,
    pub armed: bool,
    pub notes: String,
}

fn secs(s: f64) -> String {
    if s <= 0.0 {
        return String::new();
    }
    let total = s.round() as u64;
    if s < 60.0 {
        format!("{s:.1}s")
    } else {
        format!("{}:{:02}", total / 60, total % 60)
    }
}

fn file_name(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

fn label(show: &Show, id: Option<u64>) -> String {
    id.and_then(|i| show.cue(i)).map_or_else(
        || "(none)".to_string(),
        |c| match (c.number.is_empty(), c.name.is_empty()) {
            (false, false) => format!("{} {}", c.number, c.name),
            (false, true) => c.number.clone(),
            _ => c.name.clone(),
        },
    )
}

/// What the cue does, in one line.
pub fn action_text(cue: &Cue, show: &Show) -> String {
    match &cue.kind {
        CueKind::Audio(a) if a.file.is_empty() => "(no file)".into(),
        CueKind::Audio(a) => {
            let mut s = file_name(&a.file).to_string();
            if a.gain_db.abs() > 0.05 {
                s.push_str(&format!(" {:+.1} dB", a.gain_db));
            }
            if a.loop_count != 1 {
                s.push_str(if a.loop_count == 0 { " loop" } else { " x" });
                if a.loop_count > 1 {
                    s.push_str(&a.loop_count.to_string());
                }
            }
            s
        }
        CueKind::Fade(f) => format!(
            "fade {}{}",
            label(show, f.target),
            if f.stop_target { ", then stop" } else { "" }
        ),
        CueKind::Midi(m) => format!(
            "{} {}",
            if m.port.is_empty() {
                "(no port)"
            } else {
                &m.port
            },
            m.message.to_text()
        ),
        CueKind::Osc(o) => o.summary(),
        CueKind::Http(h) => h.summary(),
        CueKind::Timecode(t) => match &t.tc {
            TcAction::Start => "TC start".into(),
            TcAction::StartFrom { from } => format!("TC start from {from}"),
            TcAction::Stop => "TC stop".into(),
            TcAction::Pause => "TC pause".into(),
            TcAction::Resume => "TC resume".into(),
            TcAction::Relocate { to } => format!("TC locate {to}"),
        },
        CueKind::Control(c) => format!(
            "{} {}",
            serde_json::to_value(c.action)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
                .unwrap_or_default(),
            if c.action == crate::model::ControlAction::Reset {
                String::new()
            } else {
                label(show, c.target)
            }
        ),
        CueKind::Group(g) => format!(
            "{} group, {} cue{}",
            serde_json::to_value(g.mode)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            g.children.len(),
            if g.children.len() == 1 { "" } else { "s" }
        ),
        CueKind::Memo => String::new(),
    }
}

/// All rows, list by list, groups expanded.
pub fn rows(show: &Show) -> Vec<SheetRow> {
    fn walk(show: &Show, list: &str, cues: &[Cue], depth: usize, out: &mut Vec<SheetRow>) {
        for c in cues {
            out.push(SheetRow {
                list: list.to_string(),
                depth,
                number: c.number.clone(),
                name: c.name.clone(),
                kind: match &c.kind {
                    CueKind::Audio(_) => "Audio",
                    CueKind::Fade(_) => "Fade",
                    CueKind::Midi(_) => "MIDI",
                    CueKind::Osc(_) => "OSC",
                    CueKind::Http(_) => "HTTP",
                    CueKind::Timecode(_) => "Timecode",
                    CueKind::Control(_) => "Control",
                    CueKind::Group(_) => "Group",
                    CueKind::Memo => "Memo",
                },
                action: action_text(c, show),
                duration: c
                    .action_duration()
                    .filter(|d| *d > 0.0)
                    .map(secs)
                    .unwrap_or_default(),
                pre_wait: secs(c.pre_wait),
                post_wait: secs(c.post_wait),
                cont: match c.continue_mode {
                    ContinueMode::DoNotContinue => "",
                    ContinueMode::AutoContinue => "auto-continue",
                    ContinueMode::AutoFollow => "auto-follow",
                },
                tc: c.tc_trigger.map(|t| t.to_string()).unwrap_or_default(),
                armed: c.armed,
                notes: c.notes.clone(),
            });
            walk(show, list, c.children(), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    for l in &show.cue_lists {
        walk(show, &l.name, &l.cues, 0, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Semicolon separated with a UTF-8 BOM: opens directly in Excel (also in
/// German settings), LibreOffice and Numbers.
pub fn to_csv(show: &Show) -> String {
    fn field(s: &str) -> String {
        if s.contains([';', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    }
    let mut out = String::from("\u{feff}");
    out.push_str("List;Number;Name;Type;Action;Duration;Pre-wait;Post-wait;Continue;TC trigger;Armed;Notes\r\n");
    for r in rows(show) {
        let name = format!("{}{}", "  ".repeat(r.depth), r.name);
        let cols = [
            r.list.as_str(),
            &r.number,
            &name,
            r.kind,
            &r.action,
            &r.duration,
            &r.pre_wait,
            &r.post_wait,
            r.cont,
            &r.tc,
            if r.armed { "yes" } else { "no" },
            &r.notes,
        ];
        out.push_str(&cols.iter().map(|c| field(c)).collect::<Vec<_>>().join(";"));
        out.push_str("\r\n");
    }
    out
}

// ---------------------------------------------------------------------------
// PDF
// ---------------------------------------------------------------------------

/// Helvetica advance widths (1/1000 em) for ASCII 32..=126.
const HELV: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

fn char_w(c: char, size: f32, bold: bool) -> f32 {
    let w = match c as u32 {
        32..=126 => HELV[(c as u32 - 32) as usize],
        _ => 556,
    };
    w as f32 / 1000.0 * size * if bold { 1.06 } else { 1.0 }
}

fn text_w(s: &str, size: f32, bold: bool) -> f32 {
    s.chars().map(|c| char_w(c, size, bold)).sum()
}

/// Cut `s` to `width` (points), with an ellipsis when it was too long.
fn fit(s: &str, width: f32, size: f32, bold: bool) -> String {
    if text_w(s, size, bold) <= width {
        return s.to_string();
    }
    let ell = char_w('\u{2026}', size, bold);
    let mut out = String::new();
    let mut w = 0.0;
    for c in s.chars() {
        let cw = char_w(c, size, bold);
        if w + cw + ell > width {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('\u{2026}');
    out
}

/// Word-wrap `s` into lines of at most `width` points.
fn wrap(s: &str, width: f32, size: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in s.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if text_w(&candidate, size, false) <= width || line.is_empty() {
                line = fit(&candidate, width, size, false);
            } else {
                lines.push(std::mem::take(&mut line));
                line = fit(word, width, size, false);
            }
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// WinAnsi (cp1252) bytes for a PDF string literal, escaped.
fn pdf_str(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'(');
    for c in s.chars() {
        let b = match c {
            '(' | ')' | '\\' => {
                out.push(b'\\');
                c as u8
            }
            ' '..='~' => c as u8,
            '\u{2026}' => 0x85,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            '\u{20ac}' => 0x80,
            '\u{201e}' => 0x84,
            '\u{201c}' => 0x93,
            '\u{201d}' => 0x94,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{a0}'..='\u{ff}' => c as u32 as u8,
            _ => b'?',
        };
        out.push(b);
    }
    out.push(b')');
    out
}

struct Col {
    title: &'static str,
    width: f32,
}

const PAGE_W: f32 = 842.0;
const PAGE_H: f32 = 595.0;
const MARGIN: f32 = 32.0;
const SIZE: f32 = 8.5;
const LINE: f32 = 11.0;

/// A4 landscape cue sheet. `subtitle` goes under the show name (e.g. the date).
pub fn to_pdf(show: &Show, subtitle: &str) -> Vec<u8> {
    let cols = [
        Col {
            title: "#",
            width: 38.0,
        },
        Col {
            title: "Name",
            width: 150.0,
        },
        Col {
            title: "Type",
            width: 50.0,
        },
        Col {
            title: "Action",
            width: 165.0,
        },
        Col {
            title: "Dur.",
            width: 38.0,
        },
        Col {
            title: "Pre",
            width: 32.0,
        },
        Col {
            title: "Post",
            width: 32.0,
        },
        Col {
            title: "Continue",
            width: 62.0,
        },
        Col {
            title: "TC trigger",
            width: 58.0,
        },
        Col {
            title: "Notes",
            width: PAGE_W - 2.0 * MARGIN - 625.0,
        },
    ];
    let title = if show.name.is_empty() {
        "Show"
    } else {
        show.name.as_str()
    };

    // Content streams are built as text with byte strings spliced in below.
    let mut streams: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    let txt = |buf: &mut Vec<u8>, x: f32, y: f32, bold: bool, size: f32, s: &str| {
        buf.extend_from_slice(
            format!(
                "BT /{} {size:.1} Tf {x:.1} {y:.1} Td ",
                if bold { "F2" } else { "F1" }
            )
            .as_bytes(),
        );
        buf.extend_from_slice(&pdf_str(s));
        buf.extend_from_slice(b" Tj ET\n");
    };
    let header = |buf: &mut Vec<u8>, n: usize| -> f32 {
        let top = PAGE_H - MARGIN;
        txt(buf, MARGIN, top - 12.0, true, 15.0, title);
        txt(
            buf,
            MARGIN,
            top - 26.0,
            false,
            9.0,
            &format!("Cue sheet  {subtitle}"),
        );
        let p = format!("Page {n}");
        txt(
            buf,
            PAGE_W - MARGIN - text_w(&p, 9.0, false),
            top - 26.0,
            false,
            9.0,
            &p,
        );
        let mut x = MARGIN;
        let hy = top - 46.0;
        for c in &cols {
            txt(buf, x, hy, true, SIZE, c.title);
            x += c.width;
        }
        buf.extend_from_slice(
            format!(
                "0.5 w {MARGIN:.1} {:.1} m {:.1} {:.1} l S\n",
                hy - 4.0,
                PAGE_W - MARGIN,
                hy - 4.0
            )
            .as_bytes(),
        );
        hy - 4.0 - LINE
    };
    let mut y = header(&mut cur, 1);
    let mut last_list: Option<String> = None;
    let multi = show.cue_lists.len() > 1;
    for r in rows(show) {
        if multi && last_list.as_deref() != Some(r.list.as_str()) {
            if y < MARGIN + 3.0 * LINE {
                streams.push(std::mem::take(&mut cur));
                y = header(&mut cur, streams.len() + 1);
            }
            txt(
                &mut cur,
                MARGIN,
                y - 2.0,
                true,
                10.0,
                &format!("List: {}", r.list),
            );
            y -= LINE + 4.0;
            last_list = Some(r.list.clone());
        }
        let notes = wrap(&r.notes, cols[9].width - 4.0, SIZE);
        let height = LINE * notes.len().max(1) as f32;
        if y - height < MARGIN {
            streams.push(std::mem::take(&mut cur));
            y = header(&mut cur, streams.len() + 1);
        }
        let indent = 8.0 * r.depth as f32;
        let mut name = r.name.clone();
        if !r.armed {
            name.push_str(" (disarmed)");
        }
        let cells = [
            (r.number.as_str(), true),
            (name.as_str(), false),
            (r.kind, false),
            (r.action.as_str(), false),
            (r.duration.as_str(), false),
            (r.pre_wait.as_str(), false),
            (r.post_wait.as_str(), false),
            (r.cont, false),
            (r.tc.as_str(), false),
        ];
        let mut x = MARGIN;
        for (i, (s, bold)) in cells.iter().enumerate() {
            let pad = if i == 1 { indent } else { 0.0 };
            let s = fit(s, cols[i].width - 4.0 - pad, SIZE, *bold);
            if !s.is_empty() {
                txt(&mut cur, x + pad, y, *bold, SIZE, &s);
            }
            x += cols[i].width;
        }
        for (k, line) in notes.iter().enumerate() {
            txt(&mut cur, x, y - LINE * k as f32, false, SIZE, line);
        }
        y -= height;
        cur.extend_from_slice(
            format!(
                "0.85 G 0.3 w {MARGIN:.1} {:.1} m {:.1} {:.1} l S 0 G\n",
                y + LINE - 3.0,
                PAGE_W - MARGIN,
                y + LINE - 3.0
            )
            .as_bytes(),
        );
    }
    streams.push(cur);

    // Objects: 1 catalog, 2 pages, 3 F1, 4 F2, then (page, content) pairs.
    let n = streams.len();
    let mut objs: Vec<Vec<u8>> = Vec::new();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes());
    objs.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    objs.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    for (i, s) in streams.iter().enumerate() {
        objs.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                6 + 2 * i
            )
            .into_bytes(),
        );
        let mut c = format!("<< /Length {} >>\nstream\n", s.len()).into_bytes();
        c.extend_from_slice(s);
        c.extend_from_slice(b"\nendstream");
        objs.push(c);
    }
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::{AudioCue, GroupCue};

    fn show() -> Show {
        let mut s = Show {
            name: "Müller; \"Gala\"".into(),
            ..Show::default()
        };
        let mut add = |kind: CueKind, number: &str, name: &str, notes: &str| {
            let id = s.alloc_id();
            Cue {
                id,
                number: number.into(),
                name: name.into(),
                notes: notes.into(),
                kind,
                ..Cue::default()
            }
        };
        let a = add(
            CueKind::Audio(AudioCue {
                file: "/m/intro.wav".into(),
                file_duration: Some(93.0),
                gain_db: -3.0,
                ..AudioCue::default()
            }),
            "1",
            "Intro (Einlass)",
            "Licht auf Bühne; warten bis Saal ruhig",
        );
        let kid = add(CueKind::Memo, "2.1", "Kind", "");
        let g = add(
            CueKind::Group(GroupCue {
                children: vec![kid],
                ..GroupCue::default()
            }),
            "2",
            "Szene",
            &"Lange Notiz ".repeat(40),
        );
        s.cue_lists[0].cues = vec![a, g];
        s
    }

    #[test]
    fn csv_quotes_and_lists_every_cue() {
        let csv = to_csv(&show());
        assert!(csv.starts_with('\u{feff}'));
        let lines: Vec<&str> = csv.trim_end().split("\r\n").collect();
        assert_eq!(lines.len(), 4, "{csv}");
        assert!(lines[1].contains("intro.wav -3.0 dB"));
        assert!(lines[1].contains("\"Licht auf Bühne; warten bis Saal ruhig\""));
        assert!(lines[3].contains(";  Kind;"));
        assert!(lines[1].contains(";1:33;"));
    }

    #[test]
    fn pdf_is_well_formed() {
        let mut s = show();
        // Enough cues for several pages.
        let many: Vec<Cue> = (0..120)
            .map(|i| Cue {
                id: 1000 + i,
                number: format!("{}", 10 + i),
                name: format!("Cue {i} ({})", "x".repeat((i % 40) as usize)),
                kind: CueKind::Memo,
                ..Cue::default()
            })
            .collect();
        s.cue_lists[0].cues.extend(many);
        let pdf = to_pdf(&s, "2026-11-12");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = String::from_utf8_lossy(&pdf);
        let pages = text.matches("/Type /Page ").count();
        assert!(pages >= 3, "{pages} pages");
        assert!(text.contains(&format!("/Count {pages}")));
        // xref offsets point at the objects.
        let xref_at: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[xref_at..].starts_with(b"xref"));
        let entries: Vec<usize> = text[xref_at..]
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .map(|l| l[..10].parse().unwrap())
            .collect();
        for (i, off) in entries.iter().enumerate() {
            assert!(pdf[*off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()));
        }
        // Umlauts are WinAnsi bytes, parentheses escaped.
        assert!(pdf.windows(3).any(|w| w == [b'M', 0xFC, b'l']));
        assert!(text.contains("Intro \\(Einlass\\)"));
    }

    #[test]
    fn wrapping_and_fitting() {
        let lines = wrap("one two three four five six seven", 40.0, 8.5);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| text_w(l, 8.5, false) <= 40.0 + 0.01));
        assert!(fit(&"W".repeat(50), 30.0, 8.5, false).ends_with('\u{2026}'));
        assert_eq!(fit("ok", 30.0, 8.5, false), "ok");
    }
}
