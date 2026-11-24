use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, State,
};
use rustypipe::client::{ClientType, RustyPipe};
use rustypipe::model::{TrackItem, TrackType};
use rustypipe::param::{Country, Language};
use serde_json::Value;

mod proxy;

struct AppState {
    rp: RustyPipe,
    proxy: proxy::StreamProxy,
}

#[derive(Serialize, Clone)]
struct Track {
    id: String,
    name: String,
    artist: String,
    duration: Option<u32>,
    cover: Option<String>,
    video: bool,
}

#[derive(Serialize)]
struct HomeSection {
    items: Vec<HomeItem>,
}

#[derive(Serialize)]
struct HomeItem {
    id: String,
    name: String,
    artist: String,
    cover: Option<String>,
    kind: String,
    duration: Option<u32>,
    video: bool,
}

#[derive(Serialize)]
struct AudioOption {
    url: String,
    mime: String,
    bitrate: u32,
}

#[derive(Serialize)]
struct VideoOption {
    url: String,
    mime: String,
    quality: String,
    height: u32,
}

#[derive(Serialize)]
struct StreamsResponse {
    muxed: Vec<VideoOption>,
    video_only: Vec<VideoOption>,
    audio: Vec<AudioOption>,
    expires_in_seconds: u32,
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

fn parse_community_home(value: &Value) -> Result<Vec<HomeItem>, String> {
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

#[tauri::command]
async fn home_section(
    state: State<'_, AppState>,
    section: String,
) -> Result<HomeSection, String> {
    let q = state
        .rp
        .query()
        .unauthenticated()
        .lang(Language::En)
        .country(Country::Id);
    let items = match section.as_str() {
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
            let playlist = charts.playlists.iter().find(|playlist| playlist.name == name)
                .ok_or_else(|| format!("Daily chart playlist unavailable: {name}"))?;
            q.music_playlist(&playlist.id).await.map_err(|e| e.to_string())?
                .tracks.items.iter()
                .filter(|track| track.track_type != TrackType::Episode)
                .take(8)
                .map(|track| {
                    // Daily video charts also provide playable audio streams.
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

#[tauri::command]
async fn collection_tracks(
    state: State<'_, AppState>,
    id: String,
    kind: String,
) -> Result<Vec<Track>, String> {
    let q = state
        .rp
        .query()
        .unauthenticated()
        .lang(Language::En)
        .country(Country::Id);
    let tracks = match kind.as_str() {
        "album" => q.music_album(&id).await.map_err(|e| e.to_string())?.tracks,
        "playlist" => q
            .music_playlist(&id)
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

#[tauri::command]
async fn search(
    state: State<'_, AppState>,
    query: String,
    kind: String,
) -> Result<Vec<Track>, String> {
    let q = state.rp.query();
    let items = if kind == "videos" {
        q.music_search_videos(&query)
            .await
            .map_err(|e| e.to_string())?
            .items
            .items
    } else {
        q.music_search_tracks(&query)
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

#[tauri::command]
async fn streams(state: State<'_, AppState>, video_id: String) -> Result<StreamsResponse, String> {
    let player = match state
        .rp
        .query()
        .player_from_client(&video_id, rustypipe::client::ClientType::Ios)
        .await
    {
        Ok(p) => p,
        Err(_) => state
            .rp
            .query()
            .player(&video_id)
            .await
            .map_err(|e| e.to_string())?,
    };

    let mut audio: Vec<AudioOption> = player
        .audio_streams
        .iter()
        .filter(|s| s.drm_track_type.is_none())
        .map(|s| AudioOption {
            url: state.proxy.register(&s.url, Some(s.size), Some(&s.mime)),
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
            url: state.proxy.register(&s.url, s.size, Some(&s.mime)),
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
            url: state.proxy.register(&s.url, s.size, Some(&s.mime)),
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

#[tauri::command]
async fn radio(state: State<'_, AppState>, video_id: String) -> Result<Vec<Track>, String> {
    let p = state
        .rp
        .query()
        .music_radio_track(&video_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(p.items.iter().map(|t| map_track(t, false)).collect())
}

#[tauri::command]
fn set_tray_title(app: tauri::AppHandle, title: String) {
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(format!("Nada — {title}")));
    }
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

fn force_bring_to_front(w: &tauri::WebviewWindow) {
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_always_on_top(true);
    let _ = w.set_focus();
    let _ = w.set_always_on_top(false);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "windows")]
    std::env::set_var(
        "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
        "--autoplay-policy=no-user-gesture-required",
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                force_bring_to_front(&w);
            }
        }))
        .setup(|app| {
            let storage_dir = app
                .path()
                .app_local_data_dir()
                .expect("failed to resolve app data dir");
            std::fs::create_dir_all(&storage_dir).ok();
            let cache_dir = storage_dir.join("stream_cache");
            let _ = std::fs::remove_dir_all(&cache_dir);
            std::fs::create_dir_all(&cache_dir).ok();
            let proxy_log = storage_dir.join("proxy.log");
            let proxy = proxy::StreamProxy::start(proxy_log, cache_dir)
                .expect("failed to start stream proxy");
            let rp = RustyPipe::builder()
                .storage_dir(storage_dir)
                .build()
                .expect("failed to build RustyPipe client");
            app.manage(AppState { rp, proxy });

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_maximizable(false);
                if let Some(monitor) = window.current_monitor()? {
                    let area = monitor.work_area();
                    let scale = monitor.scale_factor();
                    let fixed_width = (430.0 * scale).round() as u32;
                    let min_height = (520.0 * scale).round() as u32;
                    let max_height = area.size.height;
                    let initial_height = ((640.0 * scale).round() as u32).min(max_height);

                    window.set_min_size(Some(tauri::PhysicalSize::new(fixed_width, min_height)))?;
                    window.set_max_size(Some(tauri::PhysicalSize::new(fixed_width, max_height)))?;
                    window.set_size(tauri::PhysicalSize::new(fixed_width, initial_height))?;

                    let pos_x = area.position.x + (area.size.width.saturating_sub(fixed_width) / 2) as i32;
                    let pos_y = area.position.y + (area.size.height.saturating_sub(initial_height) / 2) as i32;
                    window.set_position(tauri::PhysicalPosition::new(pos_x, pos_y))?;
                }
                force_bring_to_front(&window);
            }

            let show = MenuItem::with_id(app, "show", "Buka Nada", true, None::<&str>)?;
            let toggle = MenuItem::with_id(app, "toggle", "Play / Pause", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Keluar", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &toggle, &quit])?;

            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Nada")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            force_bring_to_front(&w);
                        }
                    }
                    "toggle" => {
                        let _ = app.emit("nada://toggle-play", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            force_bring_to_front(&w);
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            search,
            streams,
            radio,
            set_tray_title,
            home_section,
            collection_tracks,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Nada");
}