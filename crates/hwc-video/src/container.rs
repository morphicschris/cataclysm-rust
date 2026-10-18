//! The `.bik` container: file header, audio track table, frame index and per-frame packets.
//!
//! Layout (all integers little-endian):
//!
//! | offset | size | field                                              |
//! |--------|------|----------------------------------------------------|
//! | 0      | 3    | `"BIK"`                                            |
//! | 3      | 1    | codec revision (`b f g h i k`)                     |
//! | 4      | 4    | file size minus 8                                  |
//! | 8      | 4    | frame count                                        |
//! | 12     | 4    | largest frame packet in bytes                      |
//! | 16     | 4    | frame count (repeated, unused)                     |
//! | 20     | 4    | width                                              |
//! | 24     | 4    | height                                             |
//! | 28     | 4    | frame rate numerator                               |
//! | 32     | 4    | frame rate denominator                             |
//! | 36     | 4    | video flags (bit 20 alpha plane, bit 17 grayscale) |
//! | 40     | 4    | audio track count `n`                              |
//!
//! followed by `n` × u32 (largest decoded audio packet), `n` × (u16 sample rate, u16 flags),
//! `n` × u32 track id, then `frame count + 1` × u32 frame offsets (bit 0 marks a keyframe; the
//! last entry is the end of the final frame).
//!
//! Each frame packet holds, for every audio track in order, a u32 byte count followed by that
//! many bytes of audio, then the video data for the rest of the packet.

use std::path::Path;

use crate::{Error, Result};

const HEADER_SIZE: usize = 44;

/// Video flag: an alpha plane precedes the colour planes.
pub const VIDEO_FLAG_ALPHA: u32 = 1 << 20;
/// Video flag: the movie has no chroma planes.
pub const VIDEO_FLAG_GRAYSCALE: u32 = 1 << 17;

/// Audio flag: the track is coded with the DCT variant of Bink audio (otherwise RDFT).
pub const AUDIO_FLAG_DCT: u16 = 0x1000;
/// Audio flag: the track is stereo.
pub const AUDIO_FLAG_STEREO: u16 = 0x2000;
/// Audio flag: the source was 16-bit.
pub const AUDIO_FLAG_16BIT: u16 = 0x4000;

/// One audio track as declared in the file header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    /// Track id chosen by the encoder.
    pub id: u32,
    /// Output sample rate in Hz.
    pub sample_rate: u32,
    /// Raw track flags (`AUDIO_FLAG_*`).
    pub flags: u16,
    /// Largest decoded packet, as recorded by the encoder.
    pub max_decoded_size: u32,
}

impl AudioTrack {
    /// 1 or 2.
    pub fn channels(&self) -> u16 {
        if self.flags & AUDIO_FLAG_STEREO != 0 {
            2
        } else {
            1
        }
    }

    /// True for the DCT-based coding, false for the RDFT one.
    pub fn uses_dct(&self) -> bool {
        self.flags & AUDIO_FLAG_DCT != 0
    }
}

/// Parsed file header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The fourth signature byte: the codec revision (`b'f'` for every Cataclysm movie).
    pub revision: u8,
    pub width: u32,
    pub height: u32,
    pub frame_count: u32,
    pub largest_frame_size: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub video_flags: u32,
    pub audio_tracks: Vec<AudioTrack>,
}

impl Header {
    /// Frames per second as a float.
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }

    /// Presentation time of frame `index`, in seconds.
    pub fn frame_time(&self, index: u32) -> f64 {
        index as f64 * self.fps_den as f64 / self.fps_num as f64
    }

    /// Total running time in seconds.
    pub fn duration(&self) -> f64 {
        self.frame_time(self.frame_count)
    }

    pub fn has_alpha(&self) -> bool {
        self.video_flags & VIDEO_FLAG_ALPHA != 0
    }

    pub fn is_grayscale(&self) -> bool {
        self.video_flags & VIDEO_FLAG_GRAYSCALE != 0
    }
}

/// Location of one frame packet inside the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameEntry {
    pub offset: u32,
    pub size: u32,
    pub keyframe: bool,
}

/// One frame's payload split into its audio and video parts.
#[derive(Debug, Clone)]
pub struct FramePacket<'a> {
    pub keyframe: bool,
    /// One slice per audio track, in header order (may be empty).
    pub audio: Vec<&'a [u8]>,
    pub video: &'a [u8],
}

/// A whole `.bik` file held in memory.
#[derive(Debug, Clone)]
pub struct BinkFile {
    header: Header,
    frames: Vec<FrameEntry>,
    data: Vec<u8>,
}

fn u32_at(data: &[u8], pos: usize) -> Result<u32> {
    data.get(pos..pos + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated)
}

fn u16_at(data: &[u8], pos: usize) -> Result<u16> {
    data.get(pos..pos + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated)
}

impl BinkFile {
    /// Reads and parses a movie from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// Parses a movie already in memory.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(Error::Truncated);
        }
        if &data[0..3] != b"BIK" {
            return Err(Error::NotBink);
        }
        let revision = data[3];
        let frame_count = u32_at(&data, 8)?;
        let largest_frame_size = u32_at(&data, 12)?;
        let width = u32_at(&data, 20)?;
        let height = u32_at(&data, 24)?;
        let fps_num = u32_at(&data, 28)?;
        let fps_den = u32_at(&data, 32)?;
        let video_flags = u32_at(&data, 36)?;
        let track_count = u32_at(&data, 40)? as usize;

        if width == 0 || height == 0 || width > 7680 || height > 4800 {
            return Err(Error::Invalid("frame dimensions"));
        }
        if fps_num == 0 || fps_den == 0 {
            return Err(Error::Invalid("frame rate"));
        }
        if track_count > 256 {
            return Err(Error::Invalid("audio track count"));
        }

        let mut pos = HEADER_SIZE;
        let mut audio_tracks = Vec::with_capacity(track_count);
        for i in 0..track_count {
            let max_decoded_size = u32_at(&data, pos + 4 * i)?;
            audio_tracks.push(AudioTrack {
                id: 0,
                sample_rate: 0,
                flags: 0,
                max_decoded_size,
            });
        }
        pos += 4 * track_count;
        for (i, track) in audio_tracks.iter_mut().enumerate() {
            track.sample_rate = u16_at(&data, pos + 4 * i)? as u32;
            track.flags = u16_at(&data, pos + 4 * i + 2)?;
        }
        pos += 4 * track_count;
        for (i, track) in audio_tracks.iter_mut().enumerate() {
            track.id = u32_at(&data, pos + 4 * i)?;
        }
        pos += 4 * track_count;

        let mut frames = Vec::with_capacity(frame_count as usize);
        let mut prev = u32_at(&data, pos)?;
        for i in 0..frame_count as usize {
            let next = u32_at(&data, pos + 4 * (i + 1))?;
            let start = prev & !1;
            let end = next & !1;
            if end < start || end as usize > data.len() {
                return Err(Error::Invalid("frame index"));
            }
            frames.push(FrameEntry {
                offset: start,
                size: end - start,
                keyframe: prev & 1 != 0,
            });
            prev = next;
        }

        Ok(Self {
            header: Header {
                revision,
                width,
                height,
                frame_count,
                largest_frame_size,
                fps_num,
                fps_den,
                video_flags,
                audio_tracks,
            },
            frames,
            data,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn frame_count(&self) -> u32 {
        self.header.frame_count
    }

    pub fn frame_entries(&self) -> &[FrameEntry] {
        &self.frames
    }

    pub fn audio_tracks(&self) -> &[AudioTrack] {
        &self.header.audio_tracks
    }

    /// Splits frame `index` into its audio and video parts.
    pub fn packet(&self, index: u32) -> Result<FramePacket<'_>> {
        let entry = self
            .frames
            .get(index as usize)
            .ok_or(Error::Invalid("frame number out of range"))?;
        let mut rest = &self.data[entry.offset as usize..(entry.offset + entry.size) as usize];
        let mut audio = Vec::with_capacity(self.header.audio_tracks.len());
        for _ in &self.header.audio_tracks {
            let len = u32_at(rest, 0)? as usize;
            let body = rest.get(4..4 + len).ok_or(Error::Truncated)?;
            audio.push(body);
            rest = &rest[4 + len..];
        }
        Ok(FramePacket {
            keyframe: entry.keyframe,
            audio,
            video: rest,
        })
    }

    /// A video decoder configured for this file.
    pub fn video_decoder(&self, tables: &crate::BinkTables) -> Result<crate::VideoDecoder> {
        crate::VideoDecoder::new(&self.header, tables)
    }

    /// Decodes every frame in order, yielding RGBA images.
    pub fn frames(&self, tables: &crate::BinkTables) -> Result<crate::Frames<'_>> {
        crate::Frames::new(self, tables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"BIKf");
        for v in [0u32, 2, 20, 2, 32, 16, 30, 1, 0, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&4096u32.to_le_bytes()); // max decoded size
        d.extend_from_slice(&22050u16.to_le_bytes());
        d.extend_from_slice(&(AUDIO_FLAG_STEREO | AUDIO_FLAG_DCT).to_le_bytes());
        d.extend_from_slice(&7u32.to_le_bytes()); // track id
        let index_at = d.len();
        let first = index_at as u32 + 12;
        // frame 0: audio(4 + 3 bytes) + video 5 bytes; frame 1: audio(4 + 0) + video 2 bytes
        d.extend_from_slice(&(first | 1).to_le_bytes());
        d.extend_from_slice(&(first + 12).to_le_bytes());
        d.extend_from_slice(&(first + 18).to_le_bytes());
        d.extend_from_slice(&3u32.to_le_bytes());
        d.extend_from_slice(&[1, 2, 3, 9, 9, 9, 9, 9]);
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&[8, 8]);
        d
    }

    #[test]
    fn parses_header_tracks_and_packets() {
        let f = BinkFile::from_bytes(synthetic()).unwrap();
        let h = f.header();
        assert_eq!(
            (h.revision, h.width, h.height, h.frame_count),
            (b'f', 32, 16, 2)
        );
        assert_eq!(h.fps(), 30.0);
        let t = &f.audio_tracks()[0];
        assert_eq!(
            (t.id, t.sample_rate, t.channels(), t.uses_dct()),
            (7, 22050, 2, true)
        );
        let p0 = f.packet(0).unwrap();
        assert!(p0.keyframe);
        assert_eq!(p0.audio[0], &[1, 2, 3]);
        assert_eq!(p0.video, &[9, 9, 9, 9, 9]);
        let p1 = f.packet(1).unwrap();
        assert!(!p1.keyframe);
        assert!(p1.audio[0].is_empty());
        assert_eq!(p1.video, &[8, 8]);
        assert!(f.packet(2).is_err());
    }

    #[test]
    fn rejects_non_bink() {
        let mut d = synthetic();
        d[0] = b'X';
        assert!(matches!(BinkFile::from_bytes(d), Err(Error::NotBink)));
        assert!(matches!(
            BinkFile::from_bytes(vec![0; 10]),
            Err(Error::Truncated)
        ));
    }
}
