use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use rustypipe::client::{ClientType, RustyPipe};
use rustypipe::model::{TrackItem, TrackType};
use rustypipe::param::{Country, Language};
use serde::Serialize;
use serde_json::Value;
use single_instance::SingleInstance;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

pub mod bg_engine;
pub mod media;
pub mod proxy;
pub mod streams;
pub mod ui;

pub struct Backend {
    pub rp: RustyPipe,
    pub proxy: proxy::StreamProxy,
    pub bg: Arc<bg_engine::BgEngine>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub duration: Option<u32>,
    pub cover: Option<String>,
    pub video: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct HomeSection {
    pub items: Vec<HomeItem>,
}

#[derive(Serialize, Clone, Debug)]
pub struct HomeItem {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub cover: Option<String>,
    pub kind: String,
    pub duration: Option<u32>,
    pub video: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct AudioOption {
    pub url: String,
    pub mime: String,
    pub bitrate: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct VideoOption {
    pub url: String,
    pub mime: String,
    pub quality: String,
    pub height: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct StreamsResponse {
    pub muxed: Vec<VideoOption>,
    pub video_only: Vec<VideoOption>,
    pub audio: Vec<AudioOption>,
    pub expires_in_seconds: u32,
}

fn pick_cover(thumbs: &[rustypipe::model::Thumbnail]) -> Option<String> {
    thumbs
        .iter()
        .max_by_key(|t| t.width)
        .map(|t| t.url.clone())
}

fn map_track(t: &TrackItem, video: bool) -> Track {
    Track {
        id: t.id.clone(),
        name: t.name.clone(),
        artist: t
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", "),
        duration: t.duration,
        cover: pick_cover(&t.cover),
        video,
    }
}

fn runs_text(value: Option<&Value>) -> String {
    value
        .and_then(|v| v.get("runs"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect()
}

fn community_shelf(value: &Value) -> Option<&Value> {
    if let Some(shelf) = value.get("musicCarouselShelfRenderer") {
        let title = runs_text(shelf.pointer(
            "/header/musicCarouselShelfBasicHeaderRenderer/title",
        ));
        if title == "Trending community playlists" {
            return Some(shelf);
        }
    }
    match value {
        Value::Object(object) => object.values().find_map(community_shelf),
        Value::Array(array) => array.iter().find_map(community_shelf),
        _ => None,
    }
}

pub fn parse_community_home(value: &Value) -> Result<Vec<HomeItem>, String> {
    let shelf = community_shelf(value)
        .ok_or_else(|| "Trending community playlists shelf not found".to_string())?;
    let cards = shelf
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| "Trending community playlists shelf has no contents".to_string())?;
    Ok(cards
        .iter()
        .filter_map(|card| {
            let card = card.get("musicTwoRowItemRenderer")?;
            let id = card
                .pointer("/navigationEndpoint/browseEndpoint/browseId")?
                .as_str()?
                .strip_prefix("VL")?;
            let name = runs_text(card.get("title"));
            if id.is_empty() || name.trim().is_empty() {
                return None;
            }
            let cover = card
                .pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|thumbnail| {
                    let url = thumbnail.get("url")?.as_str()?;
                    if url.is_empty() {
                        return None;
                    }
                    let width = thumbnail.get("width").and_then(Value::as_u64).unwrap_or(0);
                    Some((width, url))
                })
                .max_by_key(|(width, _)| *width)
                .map(|(_, url)| url.to_string());
            Some(HomeItem {
                id: id.to_string(),
                name,
                artist: runs_text(card.get("subtitle")),
                cover,
                kind: "playlist".to_string(),
                duration: None,
                video: false,
            })
        })
        .take(8)
        .collect())
}

impl Backend {
    pub fn init(storage_dir: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&storage_dir).map_err(|e| e.to_string())?;
        let cache_dir = storage_dir.join("stream_cache");
        let _ = std::fs::remove_dir_all(&cache_dir);
        std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
        let proxy_log = storage_dir.join("proxy.log");
        let proxy = proxy::StreamProxy::start(proxy_log, cache_dir)?;
        let rp = RustyPipe::builder()
            .storage_dir(&storage_dir)
            .build()
            .map_err(|e| e.to_string())?;
        let host = bg_engine::start_default_js_host(Some(&storage_dir))?;
        let bg = Arc::new(bg_engine::BgEngine::new(host)?);

        Ok(Self { rp, proxy, bg })
    }

    pub async fn home_section(&self, section: &str) -> Result<HomeSection, String> {
        let q = self
            .rp
            .query()
            .unauthenticated()
            .lang(Language::En)
            .country(Country::Id);
        let items = match section {
            "community" => {
                let raw = q
                    .raw(
                        ClientType::DesktopMusic,
                        "browse",
                        &serde_json::json!({ "browseId": "FEmusic_home" }),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
                parse_community_home(&value)?
            }
            "releases" => q
                .music_new_albums()
                .await
                .map_err(|e| e.to_string())?
                .iter()
                .take(8)
                .map(|album| HomeItem {
                    id: album.id.clone(),
                    name: album.name.clone(),
                    artist: album
                        .artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    cover: pick_cover(&album.cover),
                    kind: "album".to_string(),
                    duration: None,
                    video: false,
                })
                .collect(),
            "indonesia" | "international" => {
                let region = if section == "indonesia" {
                    Country::Id
                } else {
                    Country::Zz
                };
                let charts = q.music_charts(Some(region)).await.map_err(|e| e.to_string())?;
                let name = if region == Country::Id {
                    "Daily Top Music Videos - Indonesia"
                } else {
                    "Daily Top Music Videos - Global"
                };
                let playlist = charts
                    .playlists
                    .iter()
                    .find(|playlist| playlist.name == name)
                    .ok_or_else(|| format!("Daily chart playlist unavailable: {name}"))?;
                q.music_playlist(&playlist.id)
                    .await
                    .map_err(|e| e.to_string())?
                    .tracks
                    .items
                    .iter()
                    .filter(|track| track.track_type != TrackType::Episode)
                    .take(8)
                    .map(|track| {
                        let track = map_track(track, false);
                        HomeItem {
                            id: track.id,
                            name: track.name,
                            artist: track.artist,
                            cover: track.cover,
                            kind: "track".to_string(),
                            duration: track.duration,
                            video: track.video,
                        }
                    })
                    .collect()
            }
            _ => return Err(format!("Unsupported home section: {section}")),
        };
        Ok(HomeSection { items })
    }

    pub async fn collection_tracks(&self, id: &str, kind: &str) -> Result<Vec<Track>, String> {
        let q = self
            .rp
            .query()
            .unauthenticated()
            .lang(Language::En)
            .country(Country::Id);
        let tracks = match kind {
            "album" => q.music_album(id).await.map_err(|e| e.to_string())?.tracks,
            "playlist" => q
                .music_playlist(id)
                .await
                .map_err(|e| e.to_string())?
                .tracks
                .items,
            _ => return Err(format!("Unsupported collection kind: {kind}")),
        };
        Ok(tracks
            .iter()
            .filter(|track| track.track_type != TrackType::Episode)
            .map(|track| map_track(track, track.track_type == TrackType::Video))
            .collect())
    }

    pub async fn search(&self, query: &str, kind: &str) -> Result<Vec<Track>, String> {
        let q = self.rp.query();
        let items = if kind == "videos" {
            q.music_search_videos(query)
                .await
                .map_err(|e| e.to_string())?
                .items
                .items
        } else {
            q.music_search_tracks(query)
                .await
                .map_err(|e| e.to_string())?
                .items
                .items
        };
        Ok(items
            .iter()
            .map(|t| map_track(t, kind == "videos"))
            .collect())
    }

    pub async fn streams(&self, video_id: &str) -> Result<StreamsResponse, String> {
        if let Ok(resolved) = streams::resolve_streams(&self.bg, &self.proxy, video_id).await {
            let mut audio: Vec<AudioOption> = resolved
                .audio
                .into_iter()
                .map(|s| AudioOption {
                    url: s.url,
                    mime: s.mime,
                    bitrate: s.bitrate,
                })
                .collect();
            audio.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));

            let mut muxed: Vec<VideoOption> = resolved
                .muxed
                .into_iter()
                .map(|s| VideoOption {
                    url: s.url,
                    mime: s.mime,
                    quality: s.quality,
                    height: s.height,
                })
                .collect();
            muxed.sort_by(|a, b| b.height.cmp(&a.height));

            let mut video_only: Vec<VideoOption> = resolved
                .video_only
                .into_iter()
                .map(|s| VideoOption {
                    url: s.url,
                    mime: s.mime,
                    quality: s.quality,
                    height: s.height,
                })
                .collect();
            video_only.sort_by(|a, b| b.height.cmp(&a.height));

            return Ok(StreamsResponse {
                muxed,
                video_only,
                audio,
                expires_in_seconds: resolved.expires_in_seconds,
            });
        }

        let player = self
            .rp
            .query()
            .player_from_client(video_id, ClientType::Ios)
            .await
            .map_err(|e| e.to_string())?;

        let mut audio: Vec<AudioOption> = player
            .audio_streams
            .iter()
            .filter(|s| s.drm_track_type.is_none())
            .map(|s| AudioOption {
                url: self.proxy.register(&s.url, Some(s.size), Some(&s.mime)),
                mime: s.mime.clone(),
                bitrate: s.bitrate,
            })
            .collect();
        audio.sort_by(|a, b| b.bitrate.cmp(&a.bitrate));

        let mut muxed: Vec<VideoOption> = player
            .video_streams
            .iter()
            .filter(|s| s.drm_track_type.is_none())
            .map(|s| VideoOption {
                url: self.proxy.register(&s.url, s.size, Some(&s.mime)),
                mime: s.mime.clone(),
                quality: s.quality.clone(),
                height: s.height,
            })
            .collect();
        muxed.sort_by(|a, b| b.height.cmp(&a.height));

        let mut video_only: Vec<VideoOption> = player
            .video_only_streams
            .iter()
            .filter(|s| s.drm_track_type.is_none())
            .map(|s| VideoOption {
                url: self.proxy.register(&s.url, s.size, Some(&s.mime)),
                mime: s.mime.clone(),
                quality: s.quality.clone(),
                height: s.height,
            })
            .collect();
        video_only.sort_by(|a, b| b.height.cmp(&a.height));

        Ok(StreamsResponse {
            muxed,
            video_only,
            audio,
            expires_in_seconds: player.expires_in_seconds,
        })
    }

    pub async fn radio(&self, video_id: &str) -> Result<Vec<Track>, String> {
        let p = self
            .rp
            .query()
            .music_radio_track(video_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(p.items.iter().map(|t| map_track(t, false)).collect())
    }
}

fn resolve_storage_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("com.anand.nada")
}

fn load_embedded_icons() -> (Option<egui::IconData>, Option<Icon>) {
    const WINDOW_PNG: &[u8] = include_bytes!("../icons/128x128@2x.png");
    const TRAY_PNG: &[u8] = include_bytes!("../icons/32x32.png");

    let egui_icon = image::load_from_memory(WINDOW_PNG).ok().map(|img| {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        egui::IconData {
            rgba: rgba.into_raw(),
            width: w,
            height: h,
        }
    });

    let tray_icon = image::load_from_memory(TRAY_PNG).ok().and_then(|img| {
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Icon::from_rgba(rgba.into_raw(), w, h).ok()
    });

    (egui_icon, tray_icon)
}

#[cfg(windows)]
fn apply_win32_window_icons(cc: &eframe::CreationContext<'_>) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, LoadImageW, SendMessageW, SetClassLongPtrW, GCLP_HICON,
        GCLP_HICONSM, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_DEFAULTSIZE,
        LR_SHARED, WM_SETICON,
    };

    const WINDOW_PNG: &[u8] = include_bytes!("../icons/128x128@2x.png");
    let Ok(wh) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = wh.as_raw() else {
        return;
    };
    let hwnd_raw = h.hwnd.get();

    let apply_now = move |hwnd_val: isize| {
        let hwnd = hwnd_val as *mut core::ffi::c_void;
        unsafe {
            let hmod = GetModuleHandleW(std::ptr::null());
            let mut big = LoadImageW(
                hmod,
                1usize as *const u16,
                IMAGE_ICON,
                256,
                256,
                LR_SHARED,
            );
            if big.is_null() {
                big = LoadImageW(
                    hmod,
                    1usize as *const u16,
                    IMAGE_ICON,
                    0,
                    0,
                    LR_DEFAULTSIZE | LR_SHARED,
                );
            }
            if big.is_null() {
                big = CreateIconFromResourceEx(
                    WINDOW_PNG.as_ptr(),
                    WINDOW_PNG.len() as u32,
                    1,
                    0x0003_0000,
                    256,
                    256,
                    LR_DEFAULTCOLOR,
                );
            }

            let mut small = LoadImageW(
                hmod,
                1usize as *const u16,
                IMAGE_ICON,
                32,
                32,
                LR_SHARED,
            );
            if small.is_null() {
                small = big;
            }

            if !big.is_null() {
                SetClassLongPtrW(hwnd, GCLP_HICON, big as isize);
                SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
            }
            if !small.is_null() {
                SetClassLongPtrW(hwnd, GCLP_HICONSM, small as isize);
                SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
            }
        }
    };

    apply_now(hwnd_raw);
    // Re-apply shortly after first window show in case winit resets WM_SETICON during startup
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(250));
        apply_now(hwnd_raw);
    });
}

fn try_wake_existing_instance(wake_port_path: &std::path::Path) {
    let Ok(raw) = std::fs::read_to_string(wake_port_path) else {
        return;
    };
    let Ok(port) = raw.trim().parse::<u16>() else {
        return;
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    if let Ok(mut stream) =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500))
    {
        use std::io::Write;
        let _ = stream.write_all(b"SHOW\n");
        let _ = stream.flush();
    }
}

fn start_wake_listener(wake_port_path: &std::path::Path) -> Option<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    if let Some(parent) = wake_port_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(wake_port_path, port.to_string());
    Some(listener)
}

pub fn run() {
    let storage_dir = resolve_storage_dir();
    let wake_port_file = storage_dir.join("wake.port");

    let instance = SingleInstance::new("com.anand.nada.single_instance");
    if let Ok(inst) = &instance {
        if !inst.is_single() {
            try_wake_existing_instance(&wake_port_file);
            return;
        }
    }

    let wake_listener = start_wake_listener(&wake_port_file);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let backend = Arc::new(
        Backend::init(storage_dir).expect("failed to initialize Nada native backend"),
    );

    let bg_warm = backend.bg.clone();
    rt.spawn(async move {
        let _ = bg_warm.ensure_initialized().await;
    });

    let (egui_icon, tray_icon_img) = load_embedded_icons();
    let (tray_tx, tray_rx) = mpsc::channel::<ui::TrayCmd>();
    let allow_quit = Arc::new(AtomicBool::new(false));

    // Set up native OS system tray (Buka Nada / Play-Pause / Keluar).
    // IMPORTANT: `tray_handle` MUST live in `run()` for the entire lifetime of `eframe::run_native`
    // (NOT moved into the AppCreator closure where it would be dropped immediately after startup).
    let menu = Menu::new();
    let show_item = MenuItem::new("Buka Nada", true, None);
    let toggle_item = MenuItem::new("Play / Pause", true, None);
    let quit_item = MenuItem::new("Keluar", true, None);
    let show_id = show_item.id().clone();
    let toggle_id = toggle_item.id().clone();
    let quit_id = quit_item.id().clone();
    let _ = menu.append_items(&[&show_item, &toggle_item, &quit_item]);

    let mut tray_builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Nada");
    if let Some(ic) = tray_icon_img {
        tray_builder = tray_builder.with_icon(ic);
    }
    let tray_handle = tray_builder.build().ok();
    let has_tray = tray_handle.is_some();

    let rt_handle = rt.handle().clone();

    let mut viewport = egui::ViewportBuilder::default()
        .with_app_id("com.anand.nada")
        .with_title("Nada")
        .with_inner_size([430.0, 640.0])
        .with_min_inner_size([430.0, 520.0])
        .with_max_inner_size([430.0, 4096.0])
        .with_maximize_button(false);
    if let Some(ic) = egui_icon {
        viewport = viewport.with_icon(Arc::new(ic));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    };

    let run_res = eframe::run_native(
        "Nada",
        native_options,
        Box::new(move |cc| {
            #[cfg(windows)]
            apply_win32_window_icons(cc);

            // Wake-up listener thread: restores & focuses the window when a second instance is launched.
            if let Some(listener) = wake_listener {
                let ctx_wake = cc.egui_ctx.clone();
                let tx_wake = tray_tx.clone();
                std::thread::spawn(move || {
                    for stream in listener.incoming().flatten() {
                        drop(stream);
                        let _ = tx_wake.send(ui::TrayCmd::ShowWindow);
                        ctx_wake.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx_wake.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx_wake.send_viewport_cmd(egui::ViewportCommand::Focus);
                        ctx_wake.request_repaint();
                    }
                });
            }

            let ctx_menu = cc.egui_ctx.clone();
            let tx_menu = tray_tx.clone();
            let allow_quit_menu = allow_quit.clone();
            MenuEvent::set_event_handler(Some(Arc::new(move |event: MenuEvent| {
                if event.id == show_id {
                    let _ = tx_menu.send(ui::TrayCmd::ShowWindow);
                    ctx_menu.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx_menu.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx_menu.send_viewport_cmd(egui::ViewportCommand::Focus);
                } else if event.id == toggle_id {
                    let _ = tx_menu.send(ui::TrayCmd::TogglePlay);
                } else if event.id == quit_id {
                    allow_quit_menu.store(true, std::sync::atomic::Ordering::Release);
                    let _ = tx_menu.send(ui::TrayCmd::Quit);
                    ctx_menu.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ctx_menu.request_repaint();
            })));

            let ctx_tray = cc.egui_ctx.clone();
            let tx_tray = tray_tx.clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    let _ = tx_tray.send(ui::TrayCmd::ShowWindow);
                    ctx_tray.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx_tray.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx_tray.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx_tray.request_repaint();
                }
            }));

            let tooltip_cb: Option<Box<dyn Fn(&str) + Send>> = None;

            Ok(Box::new(ui::NadaApp::new(
                cc,
                backend,
                rt_handle,
                tray_rx,
                tooltip_cb,
                allow_quit,
                has_tray,
            )))
        }),
    );

    let _ = std::fs::remove_file(&wake_port_file);
    drop(tray_handle);
    drop(instance);
    rt.shutdown_background();

    run_res.expect("error while running Nada native desktop UI");
    std::process::exit(0);
}

#[cfg(test)]
mod home_tests {
    use super::parse_community_home;
    use serde_json::json;

    #[test]
    fn ignores_unrelated_shelf() {
        let unrelated = json!({
            "musicCarouselShelfRenderer": {
                "header": { "musicCarouselShelfBasicHeaderRenderer": {
                    "title": { "runs": [{
                        "text": "Recommended playlists",
                        "navigationEndpoint": { "text": "Trending community playlists" }
                    }] }
                } },
                "contents": [{ "musicTwoRowItemRenderer": {
                    "title": { "runs": [{ "text": "Not trending" }] },
                    "navigationEndpoint": { "browseEndpoint": { "browseId": "VLwrong" } }
                } }]
            }
        });
        assert!(parse_community_home(&unrelated).is_err());
        let response = json!({ "contents": { "nested": [unrelated, {
            "musicCarouselShelfRenderer": {
                "header": { "musicCarouselShelfBasicHeaderRenderer": {
                    "title": { "runs": [
                        { "text": "Trending community " }, { "text": "playlists" }
                    ] }
                } },
                "contents": []
            }
        }] } });
        assert!(parse_community_home(&response).unwrap().is_empty());
    }

    #[test]
    fn maps_playlist_card_safely() {
        let card = json!({ "musicTwoRowItemRenderer": {
            "title": { "runs": [
                { "text": "Community ", "navigationEndpoint": { "text": "Ignore me" } },
                { "text": "mix" }
            ] },
            "subtitle": { "runs": [{ "text": "Playlist" }, { "text": " • " }, { "text": "Curator" }] },
            "navigationEndpoint": { "browseEndpoint": { "browseId": "VLPLcommunity" } },
            "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                null,
                { "width": 9999, "url": 7 },
                { "width": 9998, "url": "" },
                { "width": 120, "url": "https://example.com/small.jpg" },
                { "width": 600, "url": "https://example.com/large.jpg" }
            ] } } }
        } });
        let mut cards = vec![
            json!(null),
            json!({ "musicTwoRowItemRenderer": {} }),
            json!({ "musicTwoRowItemRenderer": {
                "title": { "runs": [{ "text": "Empty ID" }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": "VL" } }
            } }),
            json!({ "musicTwoRowItemRenderer": {
                "title": { "runs": [{ "text": "Album" }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREalbum" } }
            } }),
            card.clone(),
            json!({ "musicTwoRowItemRenderer": {
                "title": { "runs": [null, { "text": 42 }, { "text": "No cover" }] },
                "subtitle": { "runs": "invalid" },
                "navigationEndpoint": { "browseEndpoint": { "browseId": "VLPLbare" } },
                "thumbnailRenderer": null
            } }),
        ];
        cards.extend(std::iter::repeat(card).take(8));
        let response = json!({ "musicCarouselShelfRenderer": {
            "header": { "musicCarouselShelfBasicHeaderRenderer": {
                "title": { "runs": [{ "text": "Trending community playlists" }] }
            } },
            "contents": cards
        } });
        let items = parse_community_home(&response).unwrap();
        assert_eq!(items.len(), 8);
        let item = items.first().unwrap();
        assert_eq!(item.id, "PLcommunity");
        assert_eq!(item.name, "Community mix");
        assert_eq!(item.artist, "Playlist • Curator");
        assert_eq!(item.cover.as_deref(), Some("https://example.com/large.jpg"));
        assert_eq!(item.kind, "playlist");
        assert_eq!(item.duration, None);
        assert!(!item.video);
        let bare = items.get(1).unwrap();
        assert_eq!(bare.id, "PLbare");
        assert_eq!(bare.name, "No cover");
        assert!(bare.artist.is_empty());
        assert!(bare.cover.is_none());
    }
}