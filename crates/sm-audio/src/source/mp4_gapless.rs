//! Encoder delay ("priming") and padding of AAC in MP4/M4A files.
//!
//! AAC encoders put ~1024-2112 frames of silence in front of the audio and
//! pad the end to a whole packet. symphonia decodes them as audio, so a cue
//! would start that late. The real start and length are stored in the file:
//!
//! * `iTunSMPB` (iTunes / Apple encoders): `moov/udta/meta/ilst/----`,
//!   hex fields `00000000 <delay> <padding> <total frames> ...`;
//! * the edit list of the sound track (ffmpeg and most others):
//!   `moov/trak/edts/elst`, `media_time` = delay (media timescale) and
//!   `segment_duration` = length (movie timescale).
//!
//! Only the box headers on the path to those atoms are read; anything odd
//! (truncated boxes, absurd sizes) just means "no gapless info".

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Frames to drop at the start and the real length, at the file's rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gapless {
    pub delay: u64,
    pub frames: Option<u64>,
}

/// Atoms we read whole are small; never allocate more than this for one.
const MAX_ATOM: u64 = 1 << 20;

struct Atom {
    kind: [u8; 4],
    /// Payload start and end (file offsets).
    start: u64,
    end: u64,
}

fn read_u32(f: &mut File) -> Option<u32> {
    let mut b = [0u8; 4];
    f.read_exact(&mut b).ok()?;
    Some(u32::from_be_bytes(b))
}

/// The child atoms between `from` and `to`.
fn children(f: &mut File, from: u64, to: u64) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut pos = from;
    while pos + 8 <= to && out.len() < 512 {
        if f.seek(SeekFrom::Start(pos)).is_err() {
            break;
        }
        let Some(size32) = read_u32(f) else { break };
        let mut kind = [0u8; 4];
        if f.read_exact(&mut kind).is_err() {
            break;
        }
        let (size, header) = match size32 {
            0 => (to - pos, 8),
            1 => {
                let mut b = [0u8; 8];
                if f.read_exact(&mut b).is_err() {
                    break;
                }
                (u64::from_be_bytes(b), 16)
            }
            n => (u64::from(n), 8),
        };
        if size < header || pos + size > to {
            break;
        }
        out.push(Atom {
            kind,
            start: pos + header,
            end: pos + size,
        });
        pos += size;
    }
    out
}

fn child<'a>(atoms: &'a [Atom], kind: &[u8; 4]) -> Option<&'a Atom> {
    atoms.iter().find(|a| &a.kind == kind)
}

fn payload(f: &mut File, a: &Atom) -> Option<Vec<u8>> {
    let len = a.end.checked_sub(a.start)?;
    if len > MAX_ATOM {
        return None;
    }
    f.seek(SeekFrom::Start(a.start)).ok()?;
    let mut buf = vec![0u8; len as usize];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn be(b: &[u8], at: usize, n: usize) -> Option<u64> {
    let s = b.get(at..at + n)?;
    Some(s.iter().fold(0u64, |acc, x| (acc << 8) | u64::from(*x)))
}

/// Timescale of an `mvhd` / `mdhd` payload.
fn timescale(b: &[u8]) -> Option<u64> {
    match b.first()? {
        1 => be(b, 20, 4),
        _ => be(b, 12, 4),
    }
    .filter(|t| *t > 0)
}

/// Fields of `iTunSMPB`: (delay, padding, total frames).
pub(crate) fn parse_itunsmpb(text: &str) -> Option<(u64, u64, u64)> {
    let f: Vec<u64> = text
        .split_whitespace()
        .take(4)
        .map(|w| u64::from_str_radix(w, 16).ok())
        .collect::<Option<_>>()?;
    match f.as_slice() {
        [_, delay, padding, total] => Some((*delay, *padding, *total)),
        _ => None,
    }
}

fn itunsmpb(f: &mut File, moov: &[Atom]) -> Option<(u64, u64, u64)> {
    let udta = child(moov, b"udta")?;
    let udta = children(f, udta.start, udta.end);
    let meta = child(&udta, b"meta")?;
    // `meta` is a full box: 4 bytes version/flags before the children.
    let meta = children(f, meta.start + 4, meta.end);
    let ilst = child(&meta, b"ilst")?;
    for item in children(f, ilst.start, ilst.end) {
        if &item.kind != b"----" {
            continue;
        }
        let parts = children(f, item.start, item.end);
        let name = child(&parts, b"name").and_then(|a| payload(f, a))?;
        if name.get(4..) != Some(b"iTunSMPB".as_slice()) {
            continue;
        }
        let data = child(&parts, b"data").and_then(|a| payload(f, a))?;
        let text = String::from_utf8_lossy(data.get(8..)?).to_string();
        return parse_itunsmpb(&text);
    }
    None
}

/// The sound track's edit list: (delay, length) at `rate`.
fn edit_list(f: &mut File, moov: &[Atom], rate: u64) -> Option<(u64, Option<u64>)> {
    let movie_scale = child(moov, b"mvhd")
        .and_then(|a| payload(f, a))
        .and_then(|b| timescale(&b));
    for trak in moov.iter().filter(|a| &a.kind == b"trak") {
        let t = children(f, trak.start, trak.end);
        let Some(mdia) = child(&t, b"mdia") else {
            continue;
        };
        let mdia = children(f, mdia.start, mdia.end);
        let handler = child(&mdia, b"hdlr").and_then(|a| payload(f, a));
        if handler.as_deref().and_then(|h| h.get(8..12)) != Some(b"soun".as_slice()) {
            continue;
        }
        let media_scale = child(&mdia, b"mdhd")
            .and_then(|a| payload(f, a))
            .and_then(|b| timescale(&b))?;
        let edts = child(&t, b"edts")?;
        let edts = children(f, edts.start, edts.end);
        let elst = child(&edts, b"elst").and_then(|a| payload(f, a))?;
        let v1 = *elst.first()? == 1;
        let count = be(&elst, 4, 4)?;
        let entry = if v1 { 20 } else { 12 };
        let mut at = 8;
        for _ in 0..count.min(16) {
            let (dur, media_time) = if v1 {
                (be(&elst, at, 8)?, be(&elst, at + 8, 8)? as i64)
            } else {
                (
                    be(&elst, at, 4)?,
                    i64::from(be(&elst, at + 4, 4)? as u32 as i32),
                )
            };
            at += entry;
            if media_time < 0 {
                continue; // empty edit (a gap before the track)
            }
            let delay = media_time as u64 * rate / media_scale;
            let frames = movie_scale.filter(|_| dur > 0).map(|ms| {
                ((u128::from(dur) * u128::from(rate) + u128::from(ms) / 2) / u128::from(ms)) as u64
            });
            return Some((delay, frames));
        }
        return None;
    }
    None
}

/// Gapless info of an MP4/M4A file at sample rate `rate`, if it has any.
pub(crate) fn read(path: &Path, rate: u32) -> Option<Gapless> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let top = children(&mut f, 0, len);
    let moov = child(&top, b"moov")?;
    let moov = children(&mut f, moov.start, moov.end);
    if let Some((delay, _padding, total)) = itunsmpb(&mut f, &moov) {
        return Some(Gapless {
            delay,
            frames: (total > 0).then_some(total),
        });
    }
    let (delay, frames) = edit_list(&mut f, &moov, u64::from(rate))?;
    (delay > 0 || frames.is_some()).then_some(Gapless { delay, frames })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn itunsmpb_fields() {
        assert_eq!(
            parse_itunsmpb(" 00000000 00000840 000001C0 0000000000056AC0 00000000"),
            Some((2112, 448, 0x56AC0))
        );
        assert_eq!(parse_itunsmpb("xyz"), None);
        assert_eq!(parse_itunsmpb("0 1"), None);
    }

    #[test]
    fn edit_list_of_an_ffmpeg_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/onset_4800.m4a");
        let g = read(&path, 48_000).expect("gapless info");
        assert_eq!(g.delay, 1024);
        assert_eq!(g.frames, Some(24_000));
    }

    #[test]
    fn non_mp4_files_have_none() {
        let dir = std::env::temp_dir().join(format!("sm-gapless-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("x.m4a");
        let _ = std::fs::write(&p, b"not an mp4 file at all");
        assert_eq!(read(&p, 48_000), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
