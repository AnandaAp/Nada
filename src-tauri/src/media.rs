use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom};
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player};
use rust_h264::decoder::{ChromaFormat, Decoder as H264Decoder, Frame as H264Frame};
use rust_h264::nal::{parse_avcc, parse_avcc_config, NalUnitType};
use symphonia::default::{register_enabled_codecs, register_enabled_formats};
use symphonia_core::codecs::audio::AudioDecoderOptions;
use symphonia_core::codecs::registry::CodecRegistry;
use symphonia_core::codecs::video::well_known::extra_data::VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG;
use symphonia_core::codecs::video::well_known::CODEC_ID_H264;
use symphonia_core::codecs::CodecParameters;
use symphonia_core::common::Tier;
use symphonia_core::errors::Error as SymphoniaError;
use symphonia_core::formats::probe::{Hint, Probe};
use symphonia_core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia_core::io::{MediaSource, MediaSourceStream};
use symphonia_core::meta::MetadataOptions;
use symphonia_core::units::{Time, TimeBase};
use symphonia_opus::OpusDecoder;

// ============================================================================
// HTTP Range Seekable MediaSource for Local StreamProxy
// ============================================================================

const WINDOW_SIZE: usize = 256 * 1024;

pub struct HttpRangeSource {
    client: reqwest::blocking::Client,
    url: String,
    pos: u64,
    total_len: Option<u64>,
    buf_start: u64,
    buf: Vec<u8>,
    cancel: Arc<AtomicBool>,
}

impl HttpRangeSource {
    pub fn open(url: &str, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("http client build failed: {e}"))?;

        let end = (WINDOW_SIZE as u64).saturating_sub(1);
        let resp = client
            .get(url)
            .header(reqwest::header::RANGE, format!("bytes=0-{end}"))
            .send()
            .map_err(|e| format!("stream open failed: {e}"))?;

        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            return Err(format!("stream HTTP {}", resp.status()));
        }

        let total_len = parse_total_from_headers(resp.headers()).or_else(|| resp.content_length());
        let bytes = resp
            .bytes()
            .map_err(|e| format!("stream initial read failed: {e}"))?;

        Ok(Self {
            client,
            url: url.to_string(),
            pos: 0,
            total_len,
            buf_start: 0,
            buf: bytes.to_vec(),
            cancel,
        })
    }

    fn refill(&mut self, at: u64) -> io::Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if let Some(total) = self.total_len {
            if at >= total {
                self.buf_start = at;
                self.buf.clear();
                return Ok(());
            }
        }

        let max_end = at + (WINDOW_SIZE as u64) - 1;
        let end = self
            .total_len
            .map(|t| max_end.min(t.saturating_sub(1)))
            .unwrap_or(max_end);

        let resp = self
            .client
            .get(&self.url)
            .header(reqwest::header::RANGE, format!("bytes={at}-{end}"))
            .send()
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?;

        if resp.status().as_u16() == 416 {
            self.buf_start = at;
            self.buf.clear();
            return Ok(());
        }
        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("HTTP {}", resp.status()),
            ));
        }

        if self.total_len.is_none() {
            self.total_len = parse_total_from_headers(resp.headers());
        }

        let bytes = resp
            .bytes()
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e.to_string()))?;

        self.buf_start = at;
        self.buf.clear();
        self.buf.extend_from_slice(&bytes);
        Ok(())
    }
}

fn parse_total_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    if let Some(cr) = headers
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some((_, total_str)) = cr.rsplit_once('/') {
            let trimmed = total_str.trim();
            if trimmed != "*" {
                if let Ok(total) = trimmed.parse::<u64>() {
                    return Some(total);
                }
            }
        }
    }
    None
}

impl Read for HttpRangeSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.cancel.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if let Some(total) = self.total_len {
            if self.pos >= total {
                return Ok(0);
            }
        }

        let buf_end = self.buf_start + self.buf.len() as u64;
        if self.pos < self.buf_start || self.pos >= buf_end {
            self.refill(self.pos)?;
            if self.buf.is_empty() {
                return Ok(0);
            }
        }

        let rel = (self.pos - self.buf_start) as usize;
        let avail = self.buf.len().saturating_sub(rel);
        let n = avail.min(out.len());
        out[..n].copy_from_slice(&self.buf[rel..rel + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for HttpRangeSource {
    fn seek(&mut self, style: SeekFrom) -> io::Result<u64> {
        let next: i64 = match style {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(off) => (self.pos as i64).saturating_add(off),
            SeekFrom::End(off) => {
                let total = self.total_len.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::Unsupported, "unknown stream length")
                })?;
                (total as i64).saturating_add(off)
            }
        };
        if next < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "negative seek position",
            ));
        }
        self.pos = next as u64;
        Ok(self.pos)
    }
}

impl MediaSource for HttpRangeSource {
    fn is_seekable(&self) -> bool {
        self.total_len.is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.total_len
    }
}

// ============================================================================
// Codec Registry + Probe Factory
// ============================================================================

fn build_codec_registry() -> CodecRegistry {
    let mut registry = CodecRegistry::new();
    register_enabled_codecs(&mut registry);
    registry.register_audio_decoder_at_tier::<OpusDecoder>(Tier::Fallback);
    registry
}

fn build_probe() -> Probe {
    let mut probe = Probe::new();
    register_enabled_formats(&mut probe);
    probe
}

fn open_format_reader(
    url: &str,
    mime: &str,
    cancel: Arc<AtomicBool>,
) -> Result<Box<dyn FormatReader>, String> {
    let source = HttpRangeSource::open(url, cancel)?;
    let mss = MediaSourceStream::new(Box::new(source), Default::default());
    let mut hint = Hint::new();
    let clean_mime = mime.split(';').next().unwrap_or("").trim();
    if !clean_mime.is_empty() {
        hint.mime_type(clean_mime);
    }
    if clean_mime.contains("mp4") {
        hint.with_extension("mp4");
    } else if clean_mime.contains("webm") {
        hint.with_extension("webm");
    }

    let probe = build_probe();
    probe
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("format probe failed: {e}"))
}

// ============================================================================
// Decoded Video RGBA Frame & YUV -> RGBA Conversion
// ============================================================================

#[derive(Clone)]
pub struct VideoRgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    pub pts_secs: f64,
}

pub fn yuv_frame_to_rgba(frame: &H264Frame) -> (u32, u32, Vec<u8>) {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let cw = (w / 2).max(1);
    let shift_y = match frame.chroma_format {
        ChromaFormat::Yuv420 => 1usize,
        ChromaFormat::Yuv422 => 0usize,
    };
    let shift_bd = frame.bit_depth.saturating_sub(8) as u32;

    let mut rgba = vec![255u8; w * h * 4];

    if frame.bit_depth <= 8 {
        for y in 0..h {
            let cy = y >> shift_y;
            let y_row = y * w;
            let c_row = cy * cw;
            for x in 0..w {
                let cx = x >> 1;
                let yv = *frame.y.get(y_row + x).unwrap_or(&16) as i32;
                let uv = *frame.u.get(c_row + cx).unwrap_or(&128) as i32 - 128;
                let vv = *frame.v.get(c_row + cx).unwrap_or(&128) as i32 - 128;

                // BT.709 limited-range integer approximation
                let c = (yv - 16).max(0) * 298;
                let r = ((c + 459 * vv + 128) >> 8).clamp(0, 255) as u8;
                let g = ((c - 55 * uv - 136 * vv + 128) >> 8).clamp(0, 255) as u8;
                let b = ((c + 541 * uv + 128) >> 8).clamp(0, 255) as u8;

                let out = (y_row + x) * 4;
                rgba[out] = r;
                rgba[out + 1] = g;
                rgba[out + 2] = b;
            }
        }
    } else {
        for y in 0..h {
            let cy = y >> shift_y;
            let y_row = y * w;
            let c_row = cy * cw;
            for x in 0..w {
                let cx = x >> 1;
                let yv = (frame.sample(&frame.y, y_row + x) >> shift_bd) as i32;
                let uv = (frame.sample(&frame.u, c_row + cx) >> shift_bd) as i32 - 128;
                let vv = (frame.sample(&frame.v, c_row + cx) >> shift_bd) as i32 - 128;

                let c = (yv - 16).max(0) * 298;
                let r = ((c + 459 * vv + 128) >> 8).clamp(0, 255) as u8;
                let g = ((c - 55 * uv - 136 * vv + 128) >> 8).clamp(0, 255) as u8;
                let b = ((c + 541 * uv + 128) >> 8).clamp(0, 255) as u8;

                let out = (y_row + x) * 4;
                rgba[out] = r;
                rgba[out + 1] = g;
                rgba[out + 2] = b;
            }
        }
    }

    (frame.width, frame.height, rgba)
}

// ============================================================================
// Media Engine State & Controller
// ============================================================================

#[derive(Clone, Debug)]
pub struct StreamInput {
    pub url: String,
    pub mime: String,
}

#[derive(Clone, Debug)]
pub struct PlayRequest {
    pub session_id: u64,
    pub audio_candidates: Vec<StreamInput>,
    pub muxed_candidates: Vec<StreamInput>,
    pub video_candidates: Vec<StreamInput>,
    pub want_video: bool,
    pub start_pos_secs: f64,
    pub autoplay: bool,
}

#[derive(Clone, Default)]
pub struct MediaSnapshot {
    pub session_id: u64,
    pub playing: bool,
    pub loading: bool,
    pub buffering: bool,
    pub ended: bool,
    pub has_video: bool,
    pub video_active: bool,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub error: Option<String>,
    pub latest_frame: Option<VideoRgbaFrame>,
}

enum EngineCmd {
    Play(PlayRequest),
    SetPaused(bool),
    Seek(f64),
    SetVolume(f32),
    Stop,
}

pub struct MediaEngine {
    cmd_tx: mpsc::Sender<EngineCmd>,
    shared: Arc<Mutex<MediaSnapshot>>,
    active_cancel: Arc<Mutex<Arc<AtomicBool>>>,
}

impl MediaEngine {
    pub fn start(repaint: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCmd>();
        let shared = Arc::new(Mutex::new(MediaSnapshot::default()));
        let active_cancel = Arc::new(Mutex::new(Arc::new(AtomicBool::new(false))));

        let shared_bg = shared.clone();
        let cancel_bg = active_cancel.clone();

        std::thread::Builder::new()
            .name("nada-media-coordinator".to_string())
            .spawn(move || {
                run_media_coordinator(cmd_rx, shared_bg, cancel_bg, repaint);
            })
            .expect("failed to spawn media coordinator thread");

        Self {
            cmd_tx,
            shared,
            active_cancel,
        }
    }

    pub fn play(&self, req: PlayRequest) {
        {
            let mut cancel_slot = self.active_cancel.lock().unwrap();
            cancel_slot.store(true, Ordering::Release);
            *cancel_slot = Arc::new(AtomicBool::new(false));
        }
        {
            let mut snap = self.shared.lock().unwrap();
            snap.session_id = req.session_id;
            snap.loading = true;
            snap.buffering = false;
            snap.ended = false;
            snap.error = None;
            snap.position_secs = req.start_pos_secs;
            if !req.want_video {
                snap.latest_frame = None;
                snap.video_active = false;
            }
        }
        let _ = self.cmd_tx.send(EngineCmd::Play(req));
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.cmd_tx.send(EngineCmd::SetPaused(paused));
    }

    pub fn seek(&self, secs: f64) {
        let _ = self.cmd_tx.send(EngineCmd::Seek(secs.max(0.0)));
    }

    pub fn set_volume(&self, vol: f32) {
        let _ = self.cmd_tx.send(EngineCmd::SetVolume(vol.clamp(0.0, 1.0)));
    }

    pub fn stop(&self) {
        self.active_cancel
            .lock()
            .unwrap()
            .store(true, Ordering::Release);
        let _ = self.cmd_tx.send(EngineCmd::Stop);
    }

    pub fn snapshot(&self) -> MediaSnapshot {
        self.shared.lock().unwrap().clone()
    }
}

struct ActiveWorker {
    cancel: Arc<AtomicBool>,
    seek_tx: mpsc::Sender<f64>,
    pause_flag: Arc<AtomicBool>,
    clock: Arc<AudioClock>,
}

fn notify_ui(repaint: &Option<Arc<dyn Fn() + Send + Sync>>) {
    if let Some(cb) = repaint {
        cb();
    }
}

fn run_media_coordinator(
    cmd_rx: mpsc::Receiver<EngineCmd>,
    shared: Arc<Mutex<MediaSnapshot>>,
    cancel_slot: Arc<Mutex<Arc<AtomicBool>>>,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let mut sink_opt: Option<MixerDeviceSink> = DeviceSinkBuilder::open_default_sink().ok();
    let mut player_opt: Option<Arc<Player>> = sink_opt
        .as_ref()
        .map(|s| Arc::new(Player::connect_new(&s.mixer())));
    let mut volume: f32 = 0.8;
    if let Some(p) = &player_opt {
        p.set_volume(volume);
    }

    let mut active: Option<ActiveWorker> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            EngineCmd::Play(req) => {
                if let Some(prev) = active.take() {
                    prev.cancel.store(true, Ordering::Release);
                }
                if sink_opt.is_none() {
                    sink_opt = DeviceSinkBuilder::open_default_sink().ok();
                }
                if let Some(sink) = &sink_opt {
                    if let Some(old_p) = player_opt.take() {
                        old_p.stop();
                    }
                    let new_p = Arc::new(Player::connect_new(&sink.mixer()));
                    new_p.set_volume(volume);
                    if !req.autoplay {
                        new_p.pause();
                    }
                    player_opt = Some(new_p);
                }

                let cancel = cancel_slot.lock().unwrap().clone();
                let (seek_tx, seek_rx) = mpsc::channel::<f64>();
                let pause_flag = Arc::new(AtomicBool::new(!req.autoplay));
                let clock = Arc::new(AudioClock::new(req.start_pos_secs));

                active = Some(ActiveWorker {
                    cancel: cancel.clone(),
                    seek_tx,
                    pause_flag: pause_flag.clone(),
                    clock: clock.clone(),
                });

                let player_clone = player_opt.clone();
                let shared_clone = shared.clone();
                let repaint_clone = repaint.clone();

                std::thread::Builder::new()
                    .name(format!("nada-decode-{}", req.session_id))
                    .spawn(move || {
                        run_playback_session(
                            req,
                            player_clone,
                            clock,
                            pause_flag,
                            seek_rx,
                            cancel,
                            shared_clone,
                            repaint_clone,
                        );
                    })
                    .ok();
            }
            EngineCmd::SetPaused(paused) => {
                if let Some(w) = &active {
                    w.pause_flag.store(paused, Ordering::Release);
                    w.clock.set_paused(paused);
                }
                if let Some(p) = &player_opt {
                    if paused {
                        p.pause();
                    } else {
                        p.play();
                    }
                }
                {
                    let mut snap = shared.lock().unwrap();
                    snap.playing = !paused;
                }
                notify_ui(&repaint);
            }
            EngineCmd::Seek(secs) => {
                if let Some(w) = &active {
                    let _ = w.seek_tx.send(secs);
                }
                {
                    let mut snap = shared.lock().unwrap();
                    snap.position_secs = secs;
                    snap.ended = false;
                }
                notify_ui(&repaint);
            }
            EngineCmd::SetVolume(v) => {
                volume = v;
                if let Some(p) = &player_opt {
                    p.set_volume(volume);
                }
            }
            EngineCmd::Stop => {
                if let Some(prev) = active.take() {
                    prev.cancel.store(true, Ordering::Release);
                }
                if let Some(p) = &player_opt {
                    p.clear();
                    p.pause();
                }
                {
                    let mut snap = shared.lock().unwrap();
                    snap.playing = false;
                    snap.loading = false;
                    snap.buffering = false;
                    snap.latest_frame = None;
                    snap.video_active = false;
                }
                notify_ui(&repaint);
            }
        }
    }
}

// ============================================================================
// Audio + Video Decode Session (Master Audio Clock + Synchronized H.264 Stage)
// ============================================================================

struct ClockInner {
    accumulated_secs: f64,
    running_since: Option<Instant>,
    started: bool,
}

struct AudioClock {
    inner: Mutex<ClockInner>,
}

impl AudioClock {
    fn new(start_secs: f64) -> Self {
        Self {
            inner: Mutex::new(ClockInner {
                accumulated_secs: start_secs.max(0.0),
                running_since: None,
                started: false,
            }),
        }
    }

    fn on_audio_pushed(&self, paused: bool) {
        let mut g = self.inner.lock().unwrap();
        if !g.started {
            g.started = true;
            if !paused {
                g.running_since = Some(Instant::now());
            }
        } else if !paused && g.running_since.is_none() {
            g.running_since = Some(Instant::now());
        }
    }

    fn set_paused(&self, paused: bool) {
        let mut g = self.inner.lock().unwrap();
        if paused {
            if let Some(since) = g.running_since.take() {
                g.accumulated_secs += since.elapsed().as_secs_f64();
            }
        } else if g.started && g.running_since.is_none() {
            g.running_since = Some(Instant::now());
        }
    }

    fn reset(&self, media_secs: f64) {
        let mut g = self.inner.lock().unwrap();
        g.accumulated_secs = media_secs.max(0.0);
        g.running_since = None;
        g.started = false;
    }

    fn current_secs(&self) -> f64 {
        let g = self.inner.lock().unwrap();
        let extra = g
            .running_since
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        g.accumulated_secs + extra
    }
}

fn select_avc_candidates(candidates: &[StreamInput]) -> Vec<StreamInput> {
    let mut avc: Vec<StreamInput> = candidates
        .iter()
        .filter(|c| c.mime.contains("avc1") || c.mime.contains("video/mp4"))
        .cloned()
        .collect();
    if avc.is_empty() {
        avc = candidates.to_vec();
    }
    avc
}

fn run_playback_session(
    req: PlayRequest,
    player: Option<Arc<Player>>,
    clock: Arc<AudioClock>,
    pause_flag: Arc<AtomicBool>,
    seek_rx: mpsc::Receiver<f64>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<MediaSnapshot>>,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let has_video = !req.video_candidates.is_empty() || !req.muxed_candidates.is_empty();

    // Build audio source candidates: prefer dedicated audio streams, fallback to muxed mp4 audio
    let mut audio_pool = req.audio_candidates.clone();
    for m in &req.muxed_candidates {
        audio_pool.push(m.clone());
    }

    let mut opened_audio: Option<(Box<dyn FormatReader>, u32, Option<TimeBase>, Option<f64>)> =
        None;
    let mut last_err = String::from("Stream audio tidak tersedia");

    for cand in &audio_pool {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match open_format_reader(&cand.url, &cand.mime, cancel.clone()) {
            Ok(reader) => {
                if let Some(track) = reader
                    .tracks()
                    .iter()
                    .find(|t| t.codec_params.as_ref().is_some_and(|cp| cp.is_audio()))
                {
                    let track_id = track.id;
                    let tb = track.time_base;
                    let dur = track
                        .duration
                        .and_then(|d| tb.and_then(|t| t.calc_duration(d)))
                        .map(|t| t.as_secs_f64());
                    opened_audio = Some((reader, track_id, tb, dur));
                    break;
                } else {
                    last_err = "Track audio tidak ditemukan di container".to_string();
                }
            }
            Err(e) => {
                last_err = e;
            }
        }
    }

    let Some((mut audio_reader, audio_track_id, _audio_tb, detected_dur)) = opened_audio else {
        let mut snap = shared.lock().unwrap();
        if snap.session_id == req.session_id {
            snap.loading = false;
            snap.playing = false;
            snap.error = Some(last_err);
        }
        notify_ui(&repaint);
        return;
    };

    let codec_params = match audio_reader
        .tracks()
        .iter()
        .find(|t| t.id == audio_track_id)
        .and_then(|t| t.codec_params.as_ref())
        .and_then(|cp| match cp {
            CodecParameters::Audio(a) => Some(a.clone()),
            _ => None,
        }) {
        Some(p) => p,
        None => {
            let mut snap = shared.lock().unwrap();
            if snap.session_id == req.session_id {
                snap.loading = false;
                snap.error = Some("Codec audio tidak didukung".to_string());
            }
            notify_ui(&repaint);
            return;
        }
    };

    let registry = build_codec_registry();
    let mut audio_decoder = match registry
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
    {
        Ok(d) => d,
        Err(e) => {
            let mut snap = shared.lock().unwrap();
            if snap.session_id == req.session_id {
                snap.loading = false;
                snap.error = Some(format!("Gagal membuat decoder audio: {e}"));
            }
            notify_ui(&repaint);
            return;
        }
    };

    if req.start_pos_secs > 0.05 {
        if let Some(secs_time) = f64_to_symphonia_time(req.start_pos_secs) {
            let _ = audio_reader.seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: secs_time,
                    track_id: Some(audio_track_id),
                },
            );
            audio_decoder.reset();
        }
    }

    clock.reset(req.start_pos_secs);

    // Spawn companion H.264 video decoder thread when want_video is enabled
    let (vid_seek_tx, vid_seek_rx) = mpsc::channel::<f64>();
    let mut video_active = false;
    if req.want_video && has_video {
        let mut vid_pool = select_avc_candidates(&req.video_candidates);
        vid_pool.extend(select_avc_candidates(&req.muxed_candidates));
        if !vid_pool.is_empty() {
            video_active = true;
            let clock_v = clock.clone();
            let pause_v = pause_flag.clone();
            let cancel_v = cancel.clone();
            let shared_v = shared.clone();
            let repaint_v = repaint.clone();
            let start_pos = req.start_pos_secs;
            let session_id = req.session_id;

            std::thread::Builder::new()
                .name(format!("nada-video-{session_id}"))
                .spawn(move || {
                    run_video_worker(
                        session_id,
                        vid_pool,
                        start_pos,
                        clock_v,
                        pause_v,
                        vid_seek_rx,
                        cancel_v,
                        shared_v,
                        repaint_v,
                    );
                })
                .ok();
        }
    }

    {
        let mut snap = shared.lock().unwrap();
        if snap.session_id == req.session_id {
            snap.loading = false;
            snap.playing = req.autoplay;
            snap.has_video = has_video;
            snap.video_active = video_active;
            if let Some(d) = detected_dur {
                if d > 0.0 {
                    snap.duration_secs = d;
                }
            }
        }
    }
    notify_ui(&repaint);

    let mut interleaved: Vec<f32> = Vec::new();
    let mut last_ui_tick = Instant::now();
    let mut audio_eof = false;

    while !cancel.load(Ordering::Acquire) {
        // Process any pending seek requests (keep only the latest)
        let mut latest_seek: Option<f64> = None;
        while let Ok(target) = seek_rx.try_recv() {
            latest_seek = Some(target);
        }
        if let Some(target_secs) = latest_seek {
            if let Some(p) = &player {
                p.clear();
                if pause_flag.load(Ordering::Acquire) {
                    p.pause();
                } else {
                    p.play();
                }
            }
            if let Some(t) = f64_to_symphonia_time(target_secs) {
                let _ = audio_reader.seek(
                    SeekMode::Coarse,
                    SeekTo::Time {
                        time: t,
                        track_id: Some(audio_track_id),
                    },
                );
                audio_decoder.reset();
            }
            clock.reset(target_secs);
            audio_eof = false;
            let _ = vid_seek_tx.send(target_secs);
            {
                let mut snap = shared.lock().unwrap();
                if snap.session_id == req.session_id {
                    snap.position_secs = target_secs;
                    snap.ended = false;
                }
            }
            notify_ui(&repaint);
        }

        let is_paused = pause_flag.load(Ordering::Acquire);
        let cur_pos = clock.current_secs();

        if last_ui_tick.elapsed() >= Duration::from_millis(120) {
            let mut snap = shared.lock().unwrap();
            if snap.session_id == req.session_id {
                snap.position_secs = cur_pos;
                snap.playing = !is_paused;
            }
            drop(snap);
            notify_ui(&repaint);
            last_ui_tick = Instant::now();
        }

        // Throttle decode queue so memory stays tiny (~8-12 chunks ahead, ~200ms)
        if let Some(p) = &player {
            if p.len() > 16 {
                std::thread::sleep(Duration::from_millis(15));
                continue;
            }
        } else if is_paused {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        }

        if audio_eof {
            let drained = player.as_ref().map(|p| p.empty()).unwrap_or(true);
            if drained {
                let mut snap = shared.lock().unwrap();
                if snap.session_id == req.session_id {
                    snap.playing = false;
                    snap.ended = true;
                }
                drop(snap);
                notify_ui(&repaint);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
            continue;
        }

        match audio_reader.next_packet() {
            Ok(Some(packet)) => {
                if packet.track_id != audio_track_id {
                    continue;
                }
                if let Ok(decoded) = audio_decoder.decode(&packet) {
                    let rate = decoded.spec().rate();
                    let ch = decoded.spec().channels().count() as u16;
                    if rate > 0 && ch > 0 {
                        interleaved.clear();
                        decoded.copy_to_vec_interleaved(&mut interleaved);
                        if !interleaved.is_empty() {
                            if let Some(p) = &player {
                                if let (Some(nz_ch), Some(nz_rate)) =
                                    (NonZero::new(ch), NonZero::new(rate))
                                {
                                    let buf =
                                        SamplesBuffer::new(nz_ch, nz_rate, interleaved.clone());
                                    p.append(buf);
                                    clock.on_audio_pushed(pause_flag.load(Ordering::Acquire));
                                }
                            } else {
                                clock.on_audio_pushed(pause_flag.load(Ordering::Acquire));
                                let dur_ms = (interleaved.len() as u64 * 1000)
                                    / (rate as u64 * ch as u64).max(1);
                                std::thread::sleep(Duration::from_millis(dur_ms.min(50)));
                            }
                        }
                    }
                }
            }
            Ok(None) => {
                audio_eof = true;
            }
            Err(SymphoniaError::IoError(err))
                if err.kind() == io::ErrorKind::UnexpectedEof =>
            {
                audio_eof = true;
            }
            Err(SymphoniaError::ResetRequired) => {
                audio_decoder.reset();
            }
            Err(_) => {
                audio_eof = true;
            }
        }
    }
}

fn run_video_worker(
    session_id: u64,
    candidates: Vec<StreamInput>,
    start_pos_secs: f64,
    clock: Arc<AudioClock>,
    pause_flag: Arc<AtomicBool>,
    seek_rx: mpsc::Receiver<f64>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<MediaSnapshot>>,
    repaint: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    let mut opened: Option<(Box<dyn FormatReader>, u32, Option<TimeBase>, Vec<u8>)> = None;

    for cand in &candidates {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let Ok(reader) = open_format_reader(&cand.url, &cand.mime, cancel.clone()) else {
            continue;
        };
        let found = reader.tracks().iter().find_map(|t| {
            let Some(CodecParameters::Video(vp)) = &t.codec_params else {
                return None;
            };
            if vp.codec != CODEC_ID_H264 {
                return None;
            }
            let avcc = vp
                .extra_data
                .iter()
                .find(|ed| ed.id == VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG)
                .map(|ed| ed.data.to_vec())?;
            Some((t.id, t.time_base, avcc))
        });

        if let Some((track_id, tb, avcc)) = found {
            opened = Some((reader, track_id, tb, avcc));
            break;
        }
    }

    let Some((mut reader, track_id, time_base, avcc_bytes)) = opened else {
        let mut snap = shared.lock().unwrap();
        if snap.session_id == session_id {
            snap.video_active = false;
        }
        notify_ui(&repaint);
        return;
    };

    let Ok(avcc) = parse_avcc_config(&avcc_bytes) else {
        return;
    };
    let length_size = avcc.length_size;

    let mut h264 = H264Decoder::new();
    let feed_sps_pps = |dec: &mut H264Decoder| {
        if let Ok(cfg) = parse_avcc_config(&avcc_bytes) {
            for sps in &cfg.sps_nals {
                let _ = dec.decode_nal(sps);
            }
            for pps in &cfg.pps_nals {
                let _ = dec.decode_nal(pps);
            }
        }
    };
    feed_sps_pps(&mut h264);

    if start_pos_secs > 0.05 {
        if let Some(t) = f64_to_symphonia_time(start_pos_secs) {
            let _ = reader.seek(
                SeekMode::Coarse,
                SeekTo::Time {
                    time: t,
                    track_id: Some(track_id),
                },
            );
        }
    }

    let mut frame_queue: VecDeque<(u64, i32, f64, H264Frame)> = VecDeque::new();
    let mut pts_history: VecDeque<f64> = VecDeque::new();
    let mut gop_id: u64 = 0;

    while !cancel.load(Ordering::Acquire) {
        let mut seek_target: Option<f64> = None;
        while let Ok(t) = seek_rx.try_recv() {
            seek_target = Some(t);
        }
        if let Some(target_secs) = seek_target {
            frame_queue.clear();
            pts_history.clear();
            gop_id = gop_id.wrapping_add(1);
            h264 = H264Decoder::new();
            feed_sps_pps(&mut h264);
            if let Some(t) = f64_to_symphonia_time(target_secs) {
                let _ = reader.seek(
                    SeekMode::Coarse,
                    SeekTo::Time {
                        time: t,
                        track_id: Some(track_id),
                    },
                );
            }
        }

        let paused = pause_flag.load(Ordering::Acquire);
        let audio_secs = clock.current_secs();

        // Present ready frames in display order (sorted by (gop_id, pic_order_cnt))
        if let Some((_, _, next_pts, _)) = frame_queue.front() {
            let diff = *next_pts - audio_secs;
            if diff <= 0.020 {
                let (_, _, pts, yuv) = frame_queue.pop_front().unwrap();
                // Skip converting late frames if > 250ms behind master audio clock and more frames queued
                if diff >= -0.250 || frame_queue.is_empty() {
                    let (w, h, rgba) = yuv_frame_to_rgba(&yuv);
                    let vf = VideoRgbaFrame {
                        width: w,
                        height: h,
                        rgba: Arc::from(rgba),
                        pts_secs: pts,
                    };
                    {
                        let mut snap = shared.lock().unwrap();
                        if snap.session_id == session_id {
                            snap.latest_frame = Some(vf);
                        }
                    }
                    notify_ui(&repaint);
                }
                continue;
            } else if paused || frame_queue.len() >= 6 {
                std::thread::sleep(Duration::from_millis((diff * 500.0).clamp(5.0, 25.0) as u64));
                continue;
            }
        } else if paused {
            std::thread::sleep(Duration::from_millis(25));
            continue;
        }

        // Decode next H.264 packet
        match reader.next_packet() {
            Ok(Some(packet)) => {
                if packet.track_id != track_id {
                    continue;
                }
                let pkt_secs = time_base
                    .and_then(|tb| tb.calc_time(packet.pts))
                    .map(|t| t.as_secs_f64())
                    .unwrap_or(audio_secs);
                pts_history.push_back(pkt_secs);
                // Keep sorted timestamps for B-frame reordering
                let mut pts_vec: Vec<f64> = pts_history.iter().copied().collect();
                pts_vec.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                pts_history = pts_vec.into();

                let nals = parse_avcc(&packet.data, length_size);
                for nal in &nals {
                    let is_idr = nal.nal_unit_type == NalUnitType::SliceIdr;
                    if is_idr {
                        gop_id = gop_id.wrapping_add(1);
                    }
                    if let Ok(Some(frame)) = h264.decode_nal(nal) {
                        let pts = pts_history.pop_front().unwrap_or(pkt_secs);
                        let poc = frame.pic_order_cnt;
                        frame_queue.push_back((gop_id, poc, pts, frame));
                        if is_idr || frame_queue.len() >= 4 {
                            let mut v: Vec<_> = frame_queue.drain(..).collect();
                            v.sort_by_key(|(g, p, _, _)| (*g, *p));
                            frame_queue.extend(v);
                        }
                    }
                }
            }
            Ok(None) => {
                if let Some(frame) = h264.flush() {
                    let pts = pts_history.pop_front().unwrap_or(audio_secs);
                    let (w, h, rgba) = yuv_frame_to_rgba(&frame);
                    let mut snap = shared.lock().unwrap();
                    if snap.session_id == session_id {
                        snap.latest_frame = Some(VideoRgbaFrame {
                            width: w,
                            height: h,
                            rgba: Arc::from(rgba),
                            pts_secs: pts,
                        });
                    }
                    drop(snap);
                    notify_ui(&repaint);
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(SymphoniaError::IoError(err))
                if err.kind() == io::ErrorKind::UnexpectedEof =>
            {
                if let Some(frame) = h264.flush() {
                    let pts = pts_history.pop_front().unwrap_or(audio_secs);
                    let (w, h, rgba) = yuv_frame_to_rgba(&frame);
                    let mut snap = shared.lock().unwrap();
                    if snap.session_id == session_id {
                        snap.latest_frame = Some(VideoRgbaFrame {
                            width: w,
                            height: h,
                            rgba: Arc::from(rgba),
                            pts_secs: pts,
                        });
                    }
                    drop(snap);
                    notify_ui(&repaint);
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(40));
            }
        }
    }
}

fn f64_to_symphonia_time(secs: f64) -> Option<Time> {
    let clamped = secs.max(0.0);
    let whole = clamped.floor() as i64;
    let nanos = ((clamped - clamped.floor()) * 1_000_000_000.0)
        .round()
        .clamp(0.0, 999_999_999.0) as u32;
    Time::try_new(whole, nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv420_to_rgba_converts_neutral_luma_and_chroma() {
        let w = 4u32;
        let h = 4u32;
        let frame = H264Frame {
            width: w,
            height: h,
            y: vec![128u8; (w * h) as usize],
            u: vec![128u8; ((w / 2) * (h / 2)) as usize],
            v: vec![128u8; ((w / 2) * (h / 2)) as usize],
            pic_order_cnt: 0,
            chroma_format: ChromaFormat::Yuv420,
            bit_depth: 8,
        };
        let (rw, rh, rgba) = yuv_frame_to_rgba(&frame);
        assert_eq!(rw, 4);
        assert_eq!(rh, 4);
        assert_eq!(rgba.len(), 64);
        // Neutral chroma (128,128) produces greyscale R == G == B
        assert_eq!(rgba[0], rgba[1]);
        assert_eq!(rgba[1], rgba[2]);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn parse_content_range_total_extracts_size() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_RANGE,
            "bytes 0-262143/4194304".parse().unwrap(),
        );
        assert_eq!(parse_total_from_headers(&headers), Some(4_194_304));
    }
}
