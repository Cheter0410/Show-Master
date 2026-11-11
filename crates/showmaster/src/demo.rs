//! Generates a self-contained demo show (showfile + synthesised media) so
//! the application can be explored without any material at hand.

use sm_core::midi::{MidiMessage, MscCommand};
use sm_core::model::*;
use sm_core::showfile;
use sm_core::timecode::Timecode;
use std::f32::consts::TAU;
use std::io::Write;
use std::path::Path;

const SR: u32 = 48_000;

/// Write 16-bit PCM WAV. `render(frame, channel)` returns a sample in -1..1.
fn write_wav(
    path: &Path,
    channels: u16,
    seconds: f32,
    render: impl Fn(f32, u16) -> f32,
) -> std::io::Result<()> {
    let frames = (SR as f32 * seconds) as u32;
    let data_len = frames * u32::from(channels) * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&SR.to_le_bytes());
    out.extend_from_slice(&(SR * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let t = i as f32 / SR as f32;
        for c in 0..channels {
            let s = (render(t, c).clamp(-1.0, 1.0) * 32_000.0) as i16;
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    std::fs::File::create(path)?.write_all(&out)
}

fn tone(t: f32, freq: f32) -> f32 {
    (t * freq * TAU).sin()
}

/// Smooth fade at both ends of a sound of length `len`.
fn env(t: f32, len: f32, edge: f32) -> f32 {
    (t / edge).min((len - t) / edge).clamp(0.0, 1.0)
}

fn cue(show: &mut Show, number: &str, name: &str, kind: CueKind) -> Cue {
    Cue {
        id: show.alloc_id(),
        number: number.into(),
        name: name.into(),
        kind,
        ..Cue::default()
    }
}

fn audio(file: &str, duration: f64, channels: u16) -> AudioCue {
    AudioCue {
        file: format!("media/{file}"),
        file_duration: Some(duration),
        channels,
        ..AudioCue::default()
    }
}

/// Create `<dir>/demo.smshow` and `<dir>/media/*.wav`. Returns the showfile path.
pub fn make(dir: &Path) -> Result<std::path::PathBuf, String> {
    let media = dir.join("media");
    std::fs::create_dir_all(&media).map_err(|e| format!("{}: {e}", media.display()))?;
    let io = |e: std::io::Error| e.to_string();
    write_wav(&media.join("intro.wav"), 2, 12.0, |t, c| {
        let detune = if c == 0 { 1.0 } else { 1.004 };
        0.22 * env(t, 12.0, 0.4)
            * (tone(t, 220.0 * detune) + tone(t, 277.2 * detune) + tone(t, 329.6 * detune))
    })
    .map_err(io)?;
    // Exactly 4 s of whole periods: loops seamlessly.
    write_wav(&media.join("bed.wav"), 2, 4.0, |t, c| {
        0.25 * tone(t, if c == 0 { 110.0 } else { 165.0 }) * (0.6 + 0.4 * tone(t, 0.5))
    })
    .map_err(io)?;
    write_wav(&media.join("sting.wav"), 1, 1.5, |t, _| {
        0.6 * (-3.0 * t).exp() * (tone(t, 880.0) + 0.5 * tone(t, 1320.0))
    })
    .map_err(io)?;
    write_wav(&media.join("hit.wav"), 2, 2.0, |t, _| {
        0.7 * (-5.0 * t).exp() * tone(t, 60.0 + 200.0 * (-8.0 * t).exp())
    })
    .map_err(io)?;
    for (i, f) in [262.0f32, 330.0, 392.0].iter().enumerate() {
        write_wav(&media.join(format!("step{}.wav", i + 1)), 2, 1.2, |t, _| {
            0.4 * env(t, 1.2, 0.05) * tone(t, *f)
        })
        .map_err(io)?;
    }

    let mut show = Show {
        name: "Demo Show".into(),
        ..Show::default()
    };
    show.notes = "Demo show - press GO (Space) to step through. Cue 8 fires from timecode once cue 6 has started the generator.".into();
    show.midi_ports.push(MidiPortAlias {
        name: "Desk".into(),
        device: "rtp:Show Master".into(),
    });
    show.rtp_midi.enabled = true;
    show.timecode.start = Timecode::new(1, 0, 0, 0);
    show.cue_lists[0].tc_jump = TcJumpPolicy::Chase;
    let mut cues = Vec::new();

    let mut intro = audio("intro.wav", 12.0, 2);
    intro.fade_in = 1.0;
    let mut c = cue(&mut show, "1", "Intro pad", CueKind::Audio(intro));
    c.color = "blue".into();
    c.notes = "House lights to half, then GO.".into();
    let intro_id = c.id;
    cues.push(c);

    cues.push(cue(
        &mut show,
        "2",
        "Fade out intro",
        CueKind::Fade(FadeCue {
            target: Some(intro_id),
            duration: 3.0,
            ..FadeCue::default()
        }),
    ));

    let mut bed = audio("bed.wav", 4.0, 2);
    bed.loop_count = 0;
    bed.gain_db = -6.0;
    bed.fade_in = 2.0;
    let bed_cue = cue(&mut show, "", "Bed (loop)", CueKind::Audio(bed));
    let bed_id = bed_cue.id;
    let note = cue(
        &mut show,
        "",
        "Desk: note on",
        CueKind::Midi(MidiCue {
            port: "Desk".into(),
            message: MidiMessage::NoteOn {
                channel: 1,
                note: 60,
                velocity: 100,
            },
        }),
    );
    let mut scene = cue(
        &mut show,
        "3",
        "Scene 2 (bed + MIDI)",
        CueKind::Group(GroupCue {
            mode: GroupMode::Parallel,
            children: vec![bed_cue, note],
        }),
    );
    scene.color = "green".into();
    cues.push(scene);

    let mut sting = cue(
        &mut show,
        "4",
        "Sting (mono to both speakers)",
        CueKind::Audio(audio("sting.wav", 1.5, 1)),
    );
    sting.pre_wait = 0.5;
    sting.post_wait = 1.0;
    sting.continue_mode = ContinueMode::AutoContinue;
    cues.push(sting);

    let msc = MidiMessage::Msc {
        device_id: 1,
        command_format: 0x01,
        command: MscCommand::Go,
        q_number: "12.5".into(),
        q_list: String::new(),
        q_path: String::new(),
    };
    cues.push(cue(
        &mut show,
        "5",
        "Lighting desk: MSC GO 12.5",
        CueKind::Midi(MidiCue {
            port: "Desk".into(),
            message: msc,
        }),
    ));

    let mut tc = cue(
        &mut show,
        "6",
        "Start timecode at 01:00:00:00",
        CueKind::Timecode(TimecodeCue {
            tc: TcAction::Start,
        }),
    );
    tc.color = "purple".into();
    cues.push(tc);

    cues.push(cue(
        &mut show,
        "7",
        "Standby: wait for timecode",
        CueKind::Memo,
    ));

    let mut hit = cue(
        &mut show,
        "8",
        "Hit (timecode 01:00:05:00)",
        CueKind::Audio(audio("hit.wav", 2.0, 2)),
    );
    hit.tc_trigger = Some(Timecode::new(1, 0, 5, 0));
    cues.push(hit);

    cues.push(cue(
        &mut show,
        "9",
        "Stop bed (2 s fade)",
        CueKind::Control(ControlCue {
            action: ControlAction::Stop,
            target: Some(bed_id),
            time: 2.0,
        }),
    ));

    let steps: Vec<Cue> = (1..=3)
        .map(|i| {
            cue(
                &mut show,
                "",
                &format!("Step {i}"),
                CueKind::Audio(audio(&format!("step{i}.wav"), 1.2, 2)),
            )
        })
        .collect();
    let mut playlist = cue(
        &mut show,
        "10",
        "Playlist group",
        CueKind::Group(GroupCue {
            mode: GroupMode::Playlist,
            children: steps,
        }),
    );
    playlist.continue_mode = ContinueMode::AutoFollow;
    cues.push(playlist);

    cues.push(cue(
        &mut show,
        "11",
        "Stop timecode",
        CueKind::Timecode(TimecodeCue { tc: TcAction::Stop }),
    ));

    show.cue_lists[0].cues = cues;
    let path = dir.join(format!("demo.{}", showfile::EXTENSION));
    showfile::save(&show, &path).map_err(|e| e.to_string())?;
    Ok(path)
}
