mod db_maintenance;

use pinyin::{ToPinyin, ToPinyinMulti};
use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Runtime, WebviewWindow,
    WindowEvent,
};
use tauri_plugin_window_state::{AppHandleExt, StateFlags, WindowExt};
use walkdir::WalkDir;
use windows::core::PCWSTR;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Media_Duration, PKEY_Video_EncodingBitrate, PKEY_Video_FrameHeight, PKEY_Video_FrameRate,
    PKEY_Video_FrameWidth, PKEY_Video_TotalBitrate,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToDouble, PropVariantToUInt32, PropVariantToUInt64, PROPVARIANT,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::PropertiesSystem::{
    IPropertyStore, SHGetPropertyStoreFromParsingName, GPS_DEFAULT,
};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
    GUITHREADINFO, GUI_INMOVESIZE, SW_SHOWNORMAL,
};

// Supported extensions
const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "flac", "ogg", "m4a", "aif", "aiff", "wma", "aac", "ape",
];
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v"];
const TRAY_SHOW_ID: &str = "show_main";
const TRAY_QUIT_ID: &str = "quit_app";
const SNAP_THRESHOLD: i32 = 16;
const EDGE_VISIBLE_PX: i32 = 6;
const EDGE_REVEAL_PX: f64 = 28.0;
const EDGE_REHIDE_GAP_PX: f64 = 36.0;
const EDGE_POLL_MS: u64 = 50;
const DRAG_POLL_MS: u64 = 8;
const DRAG_TIMEOUT_SECS: u64 = 60;
const PINYIN_INDEX_VERSION: i32 = 4;
const WINDOW_STATE_FILENAME: &str = tauri_plugin_window_state::DEFAULT_FILENAME;
const COMPACT_WINDOW_MIGRATION_MARKER: &str = ".compact-window-default-v2";

fn window_state_flags() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED
}

fn initial_window_size(work_area: PhysicalSize<u32>) -> PhysicalSize<u32> {
    PhysicalSize::new(
        (work_area.width / 4).max(1),
        (work_area.height * 3 / 4).max(1),
    )
}

fn has_custom_saved_window_size(contents: &[u8], initialized: bool) -> bool {
    if !initialized {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(contents)
        .ok()
        .and_then(|value| {
            let state = value.get("main")?;
            Some(state.get("width")?.as_u64()? > 0 && state.get("height")?.as_u64()? > 0)
        })
        .unwrap_or(false)
}

fn restore_initial_window<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    let config_directory = window.app_handle().path().app_config_dir().ok();
    let initialized = config_directory
        .as_ref()
        .is_some_and(|directory| directory.join(COMPACT_WINDOW_MIGRATION_MARKER).exists());
    let saved = config_directory
        .as_ref()
        .and_then(|directory| fs::read(directory.join(WINDOW_STATE_FILENAME)).ok())
        .is_some_and(|contents| has_custom_saved_window_size(&contents, initialized));
    if saved {
        window.restore_state(window_state_flags())?;
    } else if let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) {
        let area = monitor.work_area();
        let size = initial_window_size(area.size);
        window.set_size(size)?;
        window.set_position(PhysicalPosition::new(
            area.position.x + (area.size.width.saturating_sub(size.width) / 2) as i32,
            area.position.y + (area.size.height.saturating_sub(size.height) / 2) as i32,
        ))?;
    }
    window.show()?;
    window.set_focus()?;
    // Mark migration only after the chosen geometry has actually reached disk.
    // A user can subsequently resize to exactly 1200x800 and retain that choice.
    if window
        .app_handle()
        .save_window_state(window_state_flags())
        .is_ok()
    {
        if let Some(directory) = config_directory {
            let _ = fs::write(directory.join(COMPACT_WINDOW_MIGRATION_MARKER), b"1");
        }
    }
    Ok(())
}

fn persist_window_state<R: Runtime>(window: &WebviewWindow<R>) {
    // The plugin holds its cache lock while querying native window state.
    // Calling it from the edge worker can wait on the UI thread while that
    // thread is handling Moved/Resized and waiting for the same cache lock.
    // Queue the entire save on the UI thread; never wait for it in the worker.
    let app = window.app_handle().clone();
    if let Err(error) = window.run_on_main_thread(move || {
        if let Err(error) = app.save_window_state(window_state_flags()) {
            eprintln!("Cannot save window state: {error}");
        }
    }) {
        eprintln!("Cannot schedule window state save: {error}");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),
    #[error("Database error: {0}")]
    Database(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Shell command failed: {0}")]
    Shell(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}

type Result<T> = std::result::Result<T, AppError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DockSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
struct DockPlacement {
    side: DockSide,
    visible_position: PhysicalPosition<i32>,
    hidden_position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    monitor_left: i32,
    monitor_right: i32,
}

#[derive(Default, Debug)]
struct WindowDockState {
    placement: Option<DockPlacement>,
    is_hidden_on_edge: bool,
    geometry_changed_at: Option<Instant>,
    user_always_on_top: bool,
    allow_quit: bool,
    restore_focus_hwnd: Option<isize>,
    drag: Option<WindowDrag>,
}

// Polling the physical cursor avoids WM_SYSCOMMAND/SC_MOVE, so Windows Snap
// never starts its titlebar move loop. Native resizing/maximizing stays intact.
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(virtual_key: i32) -> i16;
}

#[derive(Clone, Copy, Debug)]
struct WindowDrag {
    cursor_origin: PhysicalPosition<f64>,
    window_origin: PhysicalPosition<i32>,
    initial_size: PhysicalSize<u32>,
    threshold: f64,
    was_maximized: bool,
    moving: bool,
    started_at: Instant,
}

fn key_is_down(virtual_key: i32) -> bool {
    unsafe { GetAsyncKeyState(virtual_key) < 0 }
}

fn drag_target(drag: WindowDrag, cursor: PhysicalPosition<f64>) -> PhysicalPosition<i32> {
    PhysicalPosition::new(
        (drag.window_origin.x as f64 + cursor.x - drag.cursor_origin.x).round() as i32,
        (drag.window_origin.y as f64 + cursor.y - drag.cursor_origin.y).round() as i32,
    )
}

fn exceeds_drag_threshold(drag: WindowDrag, cursor: PhysicalPosition<f64>) -> bool {
    let dx = cursor.x - drag.cursor_origin.x;
    let dy = cursor.y - drag.cursor_origin.y;
    dx * dx + dy * dy >= drag.threshold * drag.threshold
}

fn restored_drag_origin(drag: WindowDrag, size: PhysicalSize<u32>) -> PhysicalPosition<i32> {
    let relative_x = ((drag.cursor_origin.x - drag.window_origin.x as f64)
        / drag.initial_size.width.max(1) as f64)
        .clamp(0.0, 1.0);
    // threshold is four logical pixels; cap the pointer's vertical anchor at
    // the 44 logical pixel custom titlebar height using the same DPI scale.
    let title_offset =
        (drag.cursor_origin.y - drag.window_origin.y as f64).clamp(0.0, 11.0 * drag.threshold);
    PhysicalPosition::new(
        (drag.cursor_origin.x - relative_x * size.width as f64).round() as i32,
        (drag.cursor_origin.y - title_offset).round() as i32,
    )
}

type SharedDockState = Arc<Mutex<WindowDockState>>;

#[derive(Clone, Default)]
struct FileIndexWriteLock(Arc<tauri::async_runtime::Mutex<()>>);

impl FileIndexWriteLock {
    async fn run<T, F>(&self, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        // Acquire before scheduling work: a reset requested during a scan must
        // fail immediately, not wait and silently erase the completed scan.
        let permit = self
            .0
            .clone()
            .try_lock_owned()
            .map_err(|_| AppError::Database("扫描或索引重置进行中，请稍后重试".to_string()))?;
        tauri::async_runtime::spawn_blocking(move || {
            let _permit = permit;
            operation()
        })
        .await
        .map_err(|error| AppError::Database(format!("索引操作失败: {error}")))?
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct WindowPinState {
    docked: bool,
    always_on_top: bool,
}

impl WindowDockState {
    fn pin_state(&self) -> WindowPinState {
        WindowPinState {
            docked: self.placement.is_some(),
            always_on_top: self.placement.is_some() || self.user_always_on_top,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DockTick {
    Wait,
    ReconcileGeometry,
    Hover,
}

fn dock_tick(state: &WindowDockState, now: Instant) -> DockTick {
    if state.drag.is_some() {
        return DockTick::Wait;
    }
    match state.geometry_changed_at {
        Some(changed) if now.saturating_duration_since(changed) < Duration::from_millis(150) => {
            DockTick::Wait
        }
        Some(_) => DockTick::ReconcileGeometry,
        None => DockTick::Hover,
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Favorite {
    pub name: String,
    pub paths: Vec<String>,
    pub is_default: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileRecord {
    pub path: String,
    pub filename: String,
    pub size: i64,
    pub mtime: i64,
    pub file_type: String, // "audio" | "video"
    pub duration_secs: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FilePropertyItem {
    pub label: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FilePropertyInfo {
    pub filename: String,
    pub path: String,
    pub created_time: String,
    pub modified_time: String,
    pub size: String,
    pub duration: String,
    pub bitrate: String,
    pub sample_rate: String,
    pub channels: String,
    pub resolution: String,
    pub frame_rate: String,
    pub media_kind: String,
    pub fields: Vec<FilePropertyItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScanResult {
    pub added: usize,
    pub updated: usize,
    pub deleted: usize,
    pub total: usize,
}

use std::collections::VecDeque;
use std::sync::OnceLock;

type SearchUnit = Vec<Vec<char>>;

struct SearchName {
    lower: String,
    units: Vec<Arc<SearchUnit>>,
    ascii_characters: u128,
}

// Cached values depend only on the immutable filename, not on file metadata or
// favorite membership. Eviction affects speed only, never matching correctness.
#[derive(Default)]
struct SearchNameCache {
    names: HashMap<String, Arc<SearchName>>,
    order: VecDeque<String>,
    characters: HashMap<char, Arc<SearchUnit>>,
}

static SEARCH_NAME_CACHE: OnceLock<Mutex<SearchNameCache>> = OnceLock::new();

fn pinyin_variants_for_char(c: char) -> Vec<String> {
    let mut variants = vec![c.to_lowercase().collect::<String>()];
    if let Some(default) = c.to_pinyin() {
        variants.push(default.plain().to_owned());
        if let Some(multi) = c.to_pinyin_multi() {
            variants.extend(multi.into_iter().map(|p| p.plain().to_owned()));
        }
    }
    variants.sort();
    variants.dedup();
    variants
}

fn build_pinyin_search_index(text: &str) -> String {
    // A linear-size representation: alternatives for each character, never
    // Cartesian products of pronunciations or fixed-length substring windows.
    serde_json::to_string(
        &text
            .chars()
            .map(pinyin_variants_for_char)
            .collect::<Vec<_>>(),
    )
    .expect("character strings serialize")
}

fn cached_search_name(filename: &str) -> Arc<SearchName> {
    let mut cache = SEARCH_NAME_CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    if let Some(name) = cache.names.get(filename) {
        return name.clone();
    }
    let lower = filename.to_lowercase();
    let units: Vec<Arc<SearchUnit>> = lower
        .chars()
        .map(|c| {
            cache
                .characters
                .entry(c)
                .or_insert_with(|| {
                    Arc::new(
                        pinyin_variants_for_char(c)
                            .into_iter()
                            .map(|value| value.chars().collect())
                            .collect(),
                    )
                })
                .clone()
        })
        .collect();
    let ascii_characters = units
        .iter()
        .flat_map(|unit| unit.iter())
        .flat_map(|spelling| spelling.iter())
        .filter(|c| c.is_ascii())
        .fold(0u128, |bits, c| bits | (1u128 << *c as u32));
    let name = Arc::new(SearchName {
        lower,
        units,
        ascii_characters,
    });
    // About one entry per distinct filename; 65k names fit typical local
    // libraries. Old names leave FIFO rather than creating stale search rows.
    if cache.names.len() >= 65_536 {
        if let Some(oldest) = cache.order.pop_front() {
            cache.names.remove(&oldest);
        }
    }
    cache.order.push_back(filename.to_owned());
    cache.names.insert(filename.to_owned(), name.clone());
    name
}

struct SmallGlob {
    masks: HashMap<char, u128>,
    stars: u128,
    accept: u128,
    start: u128,
}

impl SmallGlob {
    fn closure(&self, states: u128) -> u128 {
        // Adjacent stars were collapsed by compile(), so one epsilon step
        // closes every star transition.
        states | ((states & self.stars) << 1)
    }

    fn advance(&self, states: u128, c: char) -> u128 {
        self.closure(
            ((states & self.masks.get(&c).copied().unwrap_or(0)) << 1) | (states & self.stars),
        )
    }

    fn matches(&self, units: &[Arc<SearchUnit>]) -> bool {
        if self.start & self.accept != 0 {
            return true;
        }
        let mut states = 0;
        for unit in units {
            // New matches begin at a filename character boundary. An ending
            // match may stop partway through its last pinyin syllable.
            states |= self.start;
            let mut joined = 0;
            for spelling in unit.iter() {
                let mut branch = states;
                for &c in spelling {
                    branch = self.advance(branch, c);
                    if branch & self.accept != 0 {
                        return true;
                    }
                    if branch == 0 {
                        break;
                    }
                }
                joined |= branch;
            }
            states = joined;
        }
        false
    }
}

struct SearchTerm {
    tokens: Vec<char>,
    literal: Option<String>,
    small: Option<SmallGlob>,
    required_ascii: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchSpan {
    start: usize,
    end: usize,
}

impl SearchSpan {
    fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

struct SearchSpanGroup {
    spans: Vec<SearchSpan>,
    remaining: usize,
    next_start: usize,
}

fn assign_search_spans(groups: &mut [SearchSpanGroup], occupied: &mut Vec<SearchSpan>) -> bool {
    let mut choice = None;
    let mut choices = Vec::new();
    let mut available_characters = Vec::new();
    let mut required_characters = 0usize;
    for (index, group) in groups.iter().enumerate() {
        if group.remaining == 0 {
            continue;
        }
        let available: Vec<_> = group
            .spans
            .iter()
            .copied()
            .filter(|span| {
                span.start >= group.next_start && !occupied.iter().any(|used| span.overlaps(*used))
            })
            .collect();
        // Intervals are ordered by their end. Greedy interval scheduling gives
        // an exact upper bound for repetitions of this particular search term.
        let mut end = group.next_start;
        let mut capacity = 0;
        for span in &available {
            if span.start >= end {
                capacity += 1;
                end = span.end;
            }
        }
        if capacity < group.remaining {
            return false;
        }
        let minimum_length = available
            .iter()
            .map(|span| span.end - span.start)
            .min()
            .unwrap();
        required_characters =
            required_characters.saturating_add(minimum_length.saturating_mul(group.remaining));
        for span in &available {
            if available_characters.len() < span.end {
                available_characters.resize(span.end, false);
            }
            available_characters[span.start..span.end].fill(true);
        }
        if choice.is_none() || available.len() < choices.len() {
            choice = Some(index);
            choices = available;
        }
    }
    let Some(index) = choice else {
        return true;
    };
    if required_characters
        > available_characters
            .into_iter()
            .filter(|&used| used)
            .count()
    {
        return false;
    }
    let previous_start = groups[index].next_start;
    groups[index].remaining -= 1;
    for span in choices {
        // Identical terms are interchangeable. Assign their occurrences in
        // source order to avoid exploring every permutation of the same set.
        groups[index].next_start = span.end;
        occupied.push(span);
        if assign_search_spans(groups, occupied) {
            return true;
        }
        occupied.pop();
    }
    groups[index].remaining += 1;
    groups[index].next_start = previous_start;
    false
}

impl SearchTerm {
    fn compile(term: &str) -> Self {
        let mut tokens = Vec::new();
        for c in term.chars() {
            if c != '*' || tokens.last() != Some(&'*') {
                tokens.push(c);
            }
        }
        let literal = (!tokens.contains(&'*')).then(|| tokens.iter().collect::<String>());
        let small = if tokens.len() < 128 {
            let mut masks = HashMap::new();
            let mut stars = 0;
            for (i, &c) in tokens.iter().enumerate() {
                if c == '*' {
                    stars |= 1u128 << i;
                } else {
                    *masks.entry(c).or_insert(0) |= 1u128 << i;
                }
            }
            Some(SmallGlob {
                masks,
                stars,
                accept: 1u128 << tokens.len(),
                start: 1 | if tokens.first() == Some(&'*') { 2 } else { 0 },
            })
        } else {
            None
        };
        let required_ascii = tokens
            .iter()
            .filter(|&&c| c != '*' && c.is_ascii())
            .fold(0u128, |bits, c| bits | (1u128 << *c as u32));
        Self {
            tokens,
            literal,
            small,
            required_ascii,
        }
    }

    fn close_large(&self, states: &mut [bool]) {
        for (i, &c) in self.tokens.iter().enumerate() {
            if c == '*' && states[i] {
                states[i + 1] = true;
            }
        }
    }

    fn spans(&self, name: &SearchName) -> Vec<SearchSpan> {
        if name.ascii_characters & self.required_ascii != self.required_ascii {
            return Vec::new();
        }
        let len = self.tokens.len();
        let mut states: Vec<Option<usize>> = vec![None; len + 1];
        let mut joined = vec![None; len + 1];
        let mut branch = vec![None; len + 1];
        let mut next: Vec<Option<usize>> = vec![None; len + 1];
        let mut spans: Vec<SearchSpan> = Vec::new();
        for (unit_index, unit) in name.units.iter().enumerate() {
            states[0] = Some(unit_index);
            for (i, &token) in self.tokens.iter().enumerate() {
                if token == '*' {
                    states[i + 1] = states[i + 1].max(states[i]);
                }
            }
            joined.fill(None);
            let mut latest_start = None;
            for spelling in unit.iter() {
                branch.clone_from(&states);
                for &c in spelling {
                    next.fill(None);
                    for i in 0..len {
                        if let Some(start) = branch[i] {
                            if self.tokens[i] == '*' {
                                next[i] = next[i].max(Some(start));
                            } else if self.tokens[i] == c {
                                next[i + 1] = next[i + 1].max(Some(start));
                            }
                        }
                    }
                    for (i, &token) in self.tokens.iter().enumerate() {
                        if token == '*' {
                            next[i + 1] = next[i + 1].max(next[i]);
                        }
                    }
                    latest_start = latest_start.max(next[len]);
                    std::mem::swap(&mut branch, &mut next);
                }
                for i in 0..=len {
                    joined[i] = joined[i].max(branch[i]);
                }
            }
            if let Some(start) = latest_start {
                // A later start dominates an earlier start at the same NFA
                // state. Keep only intervals that contain no shorter match;
                // longer intervals can never help a non-overlapping assignment.
                if spans.last().is_none_or(|previous| start > previous.start) {
                    spans.push(SearchSpan {
                        start,
                        end: unit_index + 1,
                    });
                }
            }
            std::mem::swap(&mut states, &mut joined);
        }
        spans
    }

    fn matches(&self, name: &SearchName) -> bool {
        // A necessary-character test rejects impossible names cheaply. It is
        // deliberately permissive across alternative readings; the NFA still
        // verifies adjacency and chooses one complete reading per character.
        if name.ascii_characters & self.required_ascii != self.required_ascii {
            return false;
        }
        if let Some(literal) = &self.literal {
            if name.lower.contains(literal) {
                return true;
            }
        }
        if let Some(small) = &self.small {
            return small.matches(&name.units);
        }
        // Arbitrarily long queries use the same NFA with a dynamic state set;
        // the small bitset above is an optimization, not a matching limit.
        let len = self.tokens.len();
        let mut states = vec![false; len + 1];
        for unit in &name.units {
            states[0] = true;
            self.close_large(&mut states);
            if states[len] {
                return true;
            }
            let mut joined = vec![false; len + 1];
            for spelling in unit.iter() {
                let mut branch = states.clone();
                for &c in spelling {
                    let mut next = vec![false; len + 1];
                    for i in 0..len {
                        if branch[i] {
                            if self.tokens[i] == '*' {
                                next[i] = true;
                            } else if self.tokens[i] == c {
                                next[i + 1] = true;
                            }
                        }
                    }
                    self.close_large(&mut next);
                    if next[len] {
                        return true;
                    }
                    branch = next;
                }
                for i in 0..=len {
                    joined[i] |= branch[i];
                }
            }
            states = joined;
        }
        false
    }
}

struct FilenameQuery {
    terms: Vec<SearchTerm>,
}

impl FilenameQuery {
    fn compile(query: &str) -> Self {
        Self {
            terms: query
                .to_lowercase()
                .split_whitespace()
                .map(SearchTerm::compile)
                .collect(),
        }
    }

    fn matches(&self, filename: &str) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let name = cached_search_name(filename);
        if self.terms.len() == 1 {
            return self.terms[0].matches(&name);
        }
        let mut representatives: Vec<&SearchTerm> = Vec::new();
        let mut groups: Vec<SearchSpanGroup> = Vec::new();
        for term in &self.terms {
            // A standalone wildcard can match zero characters and needs no
            // source span. Every other term must occupy its own source text.
            if term.tokens.iter().all(|&c| c == '*') {
                continue;
            }
            if let Some(index) = representatives
                .iter()
                .position(|previous| previous.tokens == term.tokens)
            {
                groups[index].remaining += 1;
                continue;
            }
            let spans = term.spans(&name);
            if spans.is_empty() {
                return false;
            }
            // Different spellings (for example zhong and chong on 重) can
            // expose exactly the same source intervals and are interchangeable
            // for assignment just like repeated identical terms.
            if let Some(group) = groups.iter_mut().find(|group| group.spans == spans) {
                group.remaining += 1;
                continue;
            }
            representatives.push(term);
            groups.push(SearchSpanGroup {
                spans,
                remaining: 1,
                next_start: 0,
            });
        }
        assign_search_spans(&mut groups, &mut Vec::new())
    }
}

// Get path to SQLite database inside app_data_dir
fn get_db_path<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let app_data = app.path().app_data_dir().map_err(|e| AppError::Tauri(e))?;
    std::fs::create_dir_all(&app_data).map_err(|e| AppError::Io(e.to_string()))?;
    Ok(app_data.join("otto_sampler.db"))
}

// Initialize database connection and schemas
fn init_db(db_path: &Path) -> SqlResult<Connection> {
    let conn = Connection::open(db_path)?;
    db_maintenance::configure_new_database(&conn)?;

    // Favorites table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS favorites (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT UNIQUE NOT NULL,
            is_default INTEGER DEFAULT 0
        );",
        [],
    )?;

    // Favorite paths table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS favorite_paths (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            favorite_id INTEGER,
            path TEXT NOT NULL,
            FOREIGN KEY(favorite_id) REFERENCES favorites(id) ON DELETE CASCADE,
            UNIQUE(favorite_id, path)
        );",
        [],
    )?;

    // Scanned folders table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS folders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT UNIQUE NOT NULL
        );",
        [],
    )?;

    // Files table (with sizes, mtime, file_type, and concatenated pinyin search key index)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            folder_id INTEGER,
            path TEXT UNIQUE NOT NULL,
            filename TEXT NOT NULL,
            filename_lower TEXT NOT NULL,
            pinyin_index TEXT NOT NULL,
            pinyin_index_version INTEGER DEFAULT 1,
            size INTEGER NOT NULL,
            mtime INTEGER NOT NULL,
            file_type TEXT NOT NULL,
            duration_secs REAL DEFAULT 0.0,
            FOREIGN KEY(folder_id) REFERENCES folders(id) ON DELETE CASCADE
        );",
        [],
    )?;

    // Attempt to add duration_secs for existing databases
    let _ = conn.execute(
        "ALTER TABLE files ADD COLUMN duration_secs REAL DEFAULT 0.0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE files ADD COLUMN pinyin_index_version INTEGER DEFAULT 1",
        [],
    );

    // Create indices to make searches lightning fast
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_filename_lower ON files(filename_lower);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_pinyin_index ON files(pinyin_index);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_file_type ON files(file_type);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_folder_id ON files(folder_id);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_favorite_paths_path ON favorite_paths(path);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_favorite_paths_favorite_id ON favorite_paths(favorite_id);",
        [],
    )?;

    Ok(conn)
}

fn main_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window("main")
}

fn sync_window_pin<R: Runtime>(window: &WebviewWindow<R>, dock_state: &SharedDockState) {
    let pin = dock_state.lock().unwrap().pin_state();
    let _ = window.set_always_on_top(pin.always_on_top);
    let _ = window.set_skip_taskbar(pin.docked);
    let _ = window.emit("window-pin-state", pin);
}

#[tauri::command]
fn get_window_pin_state(state: tauri::State<'_, SharedDockState>) -> WindowPinState {
    state.lock().unwrap().pin_state()
}

#[tauri::command]
fn begin_window_drag(
    window: WebviewWindow,
    state: tauri::State<'_, SharedDockState>,
) -> Result<()> {
    if !key_is_down(0x01) || window.is_fullscreen()? {
        return Ok(());
    }
    // Collect fallible native state before publishing a session. Concurrent
    // requests can read it, but only one may acquire the session slot below.
    let drag = WindowDrag {
        cursor_origin: window.app_handle().cursor_position()?,
        window_origin: window.outer_position()?,
        initial_size: window.outer_size()?,
        threshold: 4.0 * window.scale_factor()?,
        was_maximized: window.is_maximized()?,
        moving: false,
        started_at: Instant::now(),
    };
    let mut dock = state.lock().unwrap();
    if dock.drag.is_none() && key_is_down(0x01) {
        dock.drag = Some(drag);
    }
    Ok(())
}

fn finish_window_drag(dock_state: &SharedDockState) {
    let mut state = dock_state.lock().unwrap();
    if state.drag.take().map(|drag| drag.moving).unwrap_or(false) {
        state.geometry_changed_at = Some(Instant::now());
    }
}

// Returns true whenever a custom drag owns window geometry for this tick.
fn update_window_drag<R: Runtime>(window: &WebviewWindow<R>, dock_state: &SharedDockState) -> bool {
    let Some(mut drag) = dock_state.lock().unwrap().drag else {
        return false;
    };
    if drag.started_at.elapsed() >= Duration::from_secs(DRAG_TIMEOUT_SECS)
        || key_is_down(0x1B)
        || !window.is_visible().unwrap_or(false)
        || window.is_minimized().unwrap_or(true)
    {
        finish_window_drag(dock_state);
        return true;
    }
    let Ok(cursor) = window.app_handle().cursor_position() else {
        finish_window_drag(dock_state);
        return true;
    };
    let left_down = key_is_down(0x01);
    if !drag.moving {
        if !left_down {
            finish_window_drag(dock_state);
            return true;
        }
        if !exceeds_drag_threshold(drag, cursor) {
            return true;
        }
        drag.moving = true;
        // Mark before fallible native calls so every failure path schedules
        // geometry reconciliation and releases the drag slot.
        dock_state.lock().unwrap().drag = Some(drag);
        if drag.was_maximized {
            if window.unmaximize().is_err() {
                finish_window_drag(dock_state);
                return true;
            }
            let Ok(size) = window.outer_size() else {
                finish_window_drag(dock_state);
                return true;
            };
            drag.window_origin = restored_drag_origin(drag, size);
            dock_state.lock().unwrap().drag = Some(drag);
        }
    }
    let target = drag_target(drag, cursor);
    if window.outer_position().ok() != Some(target)
        && window.set_position(Position::Physical(target)).is_err()
    {
        finish_window_drag(dock_state);
        return true;
    }
    if !left_down {
        finish_window_drag(dock_state);
    }
    true
}

#[tauri::command]
fn toggle_window_pin(
    window: WebviewWindow,
    state: tauri::State<'_, SharedDockState>,
) -> Result<WindowPinState> {
    let requested = {
        let dock = state.lock().unwrap();
        if dock.placement.is_some() {
            None
        } else {
            Some(!dock.user_always_on_top)
        }
    };
    if let Some(requested) = requested {
        window.set_always_on_top(requested)?;
        let mut dock = state.lock().unwrap();
        dock.user_always_on_top = requested;
    }
    let pin = state.lock().unwrap().pin_state();
    // A concurrent move can dock the window while the native call is pending.
    if pin.docked {
        window.set_always_on_top(true)?;
    }
    window.emit("window-pin-state", pin)?;
    Ok(pin)
}

fn remember_foreground_window<R: Runtime>(window: &WebviewWindow<R>, dock_state: &SharedDockState) {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return;
    }

    let app_hwnd = window.hwnd().ok().map(|hwnd| hwnd.0 as isize);
    let foreground_hwnd = foreground as isize;
    if Some(foreground_hwnd) == app_hwnd {
        return;
    }

    if let Ok(mut state) = dock_state.lock() {
        state.restore_focus_hwnd = Some(foreground_hwnd);
    }
}

fn restore_foreground_window(dock_state: &SharedDockState) {
    let hwnd = dock_state
        .lock()
        .ok()
        .and_then(|mut state| state.restore_focus_hwnd.take());

    if let Some(hwnd) = hwnd {
        let hwnd = hwnd as HWND;
        if !hwnd.is_null() && unsafe { IsWindow(hwnd) != 0 } {
            let _ = unsafe { SetForegroundWindow(hwnd) };
        }
    }
}

fn move_window_to<R: Runtime>(
    window: &WebviewWindow<R>,
    target: PhysicalPosition<i32>,
    dock_state: &SharedDockState,
) {
    {
        let mut state = dock_state.lock().unwrap();
        state.geometry_changed_at = None;
    }
    let _ = window.set_position(Position::Physical(target));
}

fn move_edge_hidden<R: Runtime>(
    window: &WebviewWindow<R>,
    dock_state: &SharedDockState,
    placement: DockPlacement,
    restore_focus: bool,
) {
    {
        let mut state = dock_state.lock().unwrap();
        state.is_hidden_on_edge = true;
    }
    sync_window_pin(window, dock_state);
    move_window_to(window, placement.hidden_position, dock_state);
    if restore_focus {
        restore_foreground_window(dock_state);
    }
}

fn move_edge_visible<R: Runtime>(
    window: &WebviewWindow<R>,
    dock_state: &SharedDockState,
    placement: DockPlacement,
    take_focus: bool,
) {
    {
        let mut state = dock_state.lock().unwrap();
        state.is_hidden_on_edge = false;
    }
    sync_window_pin(window, dock_state);
    move_window_to(window, placement.visible_position, dock_state);
    if take_focus {
        let _ = window.set_focus();
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>, dock_state: &SharedDockState) {
    if let Some(window) = main_window(app) {
        let placement = dock_state.lock().ok().and_then(|state| state.placement);

        let _ = window.unminimize();
        let _ = window.show();

        if let Some(placement) = placement {
            move_edge_visible(&window, dock_state, placement, true);
        } else {
            sync_window_pin(&window, dock_state);
            let _ = window.set_focus();
        }
    }
}

fn hide_main_window<R: Runtime>(app: &AppHandle<R>, dock_state: &SharedDockState) {
    if let Some(window) = main_window(app) {
        save_current_window_position(&window, dock_state);
        persist_window_state(&window);
        let _ = window.hide();
    }
}

fn toggle_main_window<R: Runtime>(app: &AppHandle<R>, dock_state: &SharedDockState) {
    if let Some(window) = main_window(app) {
        if window.is_visible().unwrap_or(false) {
            hide_main_window(app, dock_state);
        } else {
            show_main_window(app, dock_state);
        }
    }
}

// Native move/resize runs its own message loop. Wait for it to finish before
// changing window geometry, otherwise subsequent drag events overwrite our move.
fn is_native_move_active<R: Runtime>(window: &WebviewWindow<R>) -> bool {
    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    let thread = unsafe { GetWindowThreadProcessId(hwnd.0 as HWND, std::ptr::null_mut()) };
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetGUIThreadInfo(thread, &mut info) != 0 && info.flags & GUI_INMOVESIZE != 0 }
}

fn dock_placement(
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    monitor_position: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
) -> Option<DockPlacement> {
    let left = monitor_position.x;
    let right = left + monitor_size.width as i32;
    let top = monitor_position.y;
    let bottom = top + monitor_size.height as i32;
    let y = position
        .y
        .clamp(top, (bottom - size.height as i32).max(top));
    let side = if position.x <= left + SNAP_THRESHOLD {
        DockSide::Left
    } else if position.x + size.width as i32 >= right - SNAP_THRESHOLD {
        DockSide::Right
    } else {
        return None;
    };
    let (visible_x, hidden_x) = match side {
        DockSide::Left => (left, left - size.width as i32 + EDGE_VISIBLE_PX),
        DockSide::Right => (right - size.width as i32, right - EDGE_VISIBLE_PX),
    };
    Some(DockPlacement {
        side,
        visible_position: PhysicalPosition::new(visible_x, y),
        hidden_position: PhysicalPosition::new(hidden_x, y),
        size,
        monitor_left: left,
        monitor_right: right,
    })
}

fn save_current_window_position<R: Runtime>(
    window: &WebviewWindow<R>,
    dock_state: &SharedDockState,
) {
    if dock_state.lock().unwrap().drag.is_some() || is_native_move_active(window) {
        return;
    }
    let Ok(position) = window.outer_position() else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let previous = {
        let state = dock_state.lock().unwrap();
        state.placement.map(|p| (p, state.is_hidden_on_edge))
    };
    // Read actual geometry instead of trusting an asynchronous Moved payload.
    // A stale reveal event can arrive after hiding (and vice versa).
    if let Some((placement, hidden)) = previous {
        let expected = if hidden {
            placement.hidden_position
        } else {
            placement.visible_position
        };
        if position == expected && size == placement.size {
            return;
        }
    }
    let monitors = window.available_monitors().unwrap_or_default();
    let center_x = position.x as i64 + size.width as i64 / 2;
    let center_y = position.y as i64 + size.height as i64 / 2;
    // current_monitor() can be None once a drag takes the entire window outside
    // the desktop. Keep the closest display available as a recovery anchor.
    let monitor = monitors.iter().min_by_key(|monitor| {
        let p = monitor.position();
        let s = monitor.size();
        let dx = center_x - center_x.clamp(p.x as i64, p.x as i64 + s.width as i64);
        let dy = center_y - center_y.clamp(p.y as i64, p.y as i64 + s.height as i64);
        dx * dx + dy * dy
    });
    let placement =
        if window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false) {
            None
        } else {
            monitor.and_then(|m| dock_placement(position, size, *m.position(), *m.size()))
        };
    {
        let mut state = dock_state.lock().unwrap();
        state.placement = placement;
        state.is_hidden_on_edge = false;
        state.geometry_changed_at = None;
    }
    if let Some(placement) = placement {
        move_edge_visible(window, dock_state, placement, false);
    } else if previous.is_some() {
        sync_window_pin(window, dock_state);
    }
}

fn cursor_near_dock_edge(placement: DockPlacement, cursor: PhysicalPosition<f64>) -> bool {
    let in_y = cursor.y >= placement.visible_position.y as f64
        && cursor.y < (placement.visible_position.y + placement.size.height as i32) as f64;
    let in_x = match placement.side {
        DockSide::Left => {
            cursor.x >= placement.monitor_left as f64
                && cursor.x <= placement.monitor_left as f64 + EDGE_REVEAL_PX
        }
        DockSide::Right => {
            cursor.x < placement.monitor_right as f64
                && cursor.x >= placement.monitor_right as f64 - EDGE_REVEAL_PX
        }
    };
    in_x && in_y
}

fn start_edge_reveal_worker<R: Runtime>(app: AppHandle<R>, dock_state: SharedDockState) {
    thread::spawn(move || loop {
        let poll_ms = if dock_state.lock().unwrap().drag.is_some() {
            DRAG_POLL_MS
        } else {
            EDGE_POLL_MS
        };
        thread::sleep(Duration::from_millis(poll_ms));

        let Some(window) = main_window(&app) else {
            finish_window_drag(&dock_state);
            continue;
        };
        if update_window_drag(&window, &dock_state) {
            continue;
        }
        if !window.is_visible().unwrap_or(false) {
            continue;
        }
        if window.is_minimized().unwrap_or(false) || is_native_move_active(&window) {
            continue;
        }

        let tick = dock_tick(&dock_state.lock().unwrap(), Instant::now());
        match tick {
            // The old placement belongs to the geometry before this move.
            // Do not snap a freshly undocked window back before reconciliation.
            DockTick::Wait => continue,
            DockTick::ReconcileGeometry => {
                dock_state.lock().unwrap().geometry_changed_at = None;
                save_current_window_position(&window, &dock_state);
                persist_window_state(&window);
            }
            DockTick::Hover => {}
        }

        let Some(placement) = dock_state.lock().ok().and_then(|state| state.placement) else {
            continue;
        };
        let Ok(cursor) = app.cursor_position() else {
            continue;
        };

        let cursor_near_hidden_edge = cursor_near_dock_edge(placement, cursor);
        let cursor_away_from_panel = cursor.y
            < placement.visible_position.y as f64 - EDGE_REHIDE_GAP_PX
            || cursor.y
                >= (placement.visible_position.y + placement.size.height as i32) as f64
                    + EDGE_REHIDE_GAP_PX
            || match placement.side {
                DockSide::Left => {
                    cursor.x
                        > (placement.monitor_left + placement.size.width as i32) as f64
                            + EDGE_REHIDE_GAP_PX
                }
                DockSide::Right => {
                    cursor.x
                        < (placement.monitor_right - placement.size.width as i32) as f64
                            - EDGE_REHIDE_GAP_PX
                }
            };

        let state = dock_state.lock().unwrap();
        if state.is_hidden_on_edge && cursor_near_hidden_edge {
            drop(state);
            remember_foreground_window(&window, &dock_state);
            let _ = window.show();
            move_edge_visible(&window, &dock_state, placement, false);
        } else if !state.is_hidden_on_edge && cursor_away_from_panel {
            drop(state);
            move_edge_hidden(&window, &dock_state, placement, true);
        }
    });
}

fn setup_tray<R: Runtime>(
    app: &tauri::App<R>,
    dock_state: SharedDockState,
) -> std::result::Result<(), String> {
    let show_item = MenuItem::with_id(app, TRAY_SHOW_ID, "Show Main Panel", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit_item = MenuItem::with_id(app, TRAY_QUIT_ID, "Exit App", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item]).map_err(|e| e.to_string())?;
    let app_handle = app.handle().clone();
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "missing default window icon".to_string())?;

    TrayIconBuilder::new()
        .tooltip("Otto-Sampler")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event({
            let dock_state = dock_state.clone();
            move |app, event| match event.id().as_ref() {
                TRAY_SHOW_ID => show_main_window(app, &dock_state),
                TRAY_QUIT_ID => {
                    if let Ok(mut state) = dock_state.lock() {
                        state.allow_quit = true;
                    }
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(move |_tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(&app_handle, &dock_state);
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;

    Ok(())
}

// --- COMMANDS ---

#[tauri::command]
async fn add_favorite<R: Runtime>(
    app: AppHandle<R>,
    name: String,
    paths: Vec<String>,
) -> Result<()> {
    let db_path = get_db_path(&app)?;
    let mut conn = Connection::open(&db_path)?;
    let tx = conn.transaction()?;

    // Insert favorite
    tx.execute(
        "INSERT OR IGNORE INTO favorites (name, is_default) VALUES (?1, 0)",
        params![name],
    )?;

    let favorite_id: i64 = tx.query_row(
        "SELECT id FROM favorites WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )?;

    // Clear existing paths for this favorite
    tx.execute(
        "DELETE FROM favorite_paths WHERE favorite_id = ?1",
        params![favorite_id],
    )?;

    // Insert new paths
    for path in paths {
        tx.execute(
            "INSERT OR IGNORE INTO favorite_paths (favorite_id, path) VALUES (?1, ?2)",
            params![favorite_id, path],
        )?;
    }

    tx.commit()?;
    Ok(())
}

#[tauri::command]
async fn get_favorites<R: Runtime>(app: AppHandle<R>) -> Result<Vec<Favorite>> {
    let db_path = get_db_path(&app)?;
    let conn = Connection::open(&db_path)?;

    let mut stmt = conn.prepare("SELECT id, name, is_default FROM favorites ORDER BY name")?;
    let mut path_stmt = conn.prepare("SELECT path FROM favorite_paths WHERE favorite_id = ?1")?;

    let favorites_iter = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let is_default_int: i32 = row.get(2)?;

        let paths_rows = path_stmt.query_map(params![id], |r| r.get(0))?;
        let mut paths = Vec::new();
        for p in paths_rows {
            paths.push(p?);
        }

        Ok(Favorite {
            name,
            paths,
            is_default: is_default_int == 1,
        })
    })?;

    let mut favorites = Vec::new();
    for f in favorites_iter {
        favorites.push(f?);
    }

    Ok(favorites)
}

#[tauri::command]
async fn set_default_favorite<R: Runtime>(app: AppHandle<R>, name: String) -> Result<()> {
    let db_path = get_db_path(&app)?;
    let mut conn = Connection::open(&db_path)?;
    let tx = conn.transaction()?;

    // Reset all
    tx.execute("UPDATE favorites SET is_default = 0", [])?;

    // Set default
    tx.execute(
        "UPDATE favorites SET is_default = 1 WHERE name = ?1",
        params![name],
    )?;

    tx.commit()?;
    Ok(())
}

#[tauri::command]
async fn delete_favorite<R: Runtime>(app: AppHandle<R>, name: String) -> Result<()> {
    let db_path = get_db_path(&app)?;
    let conn = Connection::open(&db_path)?;

    // Cascading delete is enabled, so delete from favorites will clean up paths
    conn.execute("DELETE FROM favorites WHERE name = ?1", params![name])?;

    Ok(())
}

#[tauri::command]
async fn scan_favorite<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, FileIndexWriteLock>,
    name: String,
) -> Result<ScanResult> {
    let db_path = get_db_path(&app)?;
    state
        .run(move || {
            let conn = Connection::open(db_path)?;
            let result = scan_favorite_in_connection(&conn, &name)?;
            if let Err(error) = db_maintenance::reclaim_unused_space(&conn) {
                eprintln!("Index database maintenance skipped: {error}");
            }
            Ok(result)
        })
        .await
}

fn scan_favorite_in_connection(conn: &Connection, name: &str) -> Result<ScanResult> {
    scan_favorite_with_walker(conn, name, |root| WalkDir::new(root).into_iter())
}

fn scan_favorite_with_walker<F, I>(conn: &Connection, name: &str, walk: F) -> Result<ScanResult>
where
    F: Fn(&Path) -> I,
    I: IntoIterator<Item = walkdir::Result<walkdir::DirEntry>>,
{
    // 1. Get paths for this favorite
    let mut path_stmt = conn.prepare(
        "SELECT fp.path FROM favorite_paths fp
         JOIN favorites f ON f.id = fp.favorite_id
         WHERE f.name = ?1",
    )?;

    let path_rows = path_stmt.query_map(params![name], |row| row.get::<_, String>(0))?;
    let mut target_paths = Vec::new();
    for r in path_rows {
        target_paths.push(r?);
    }

    if target_paths.is_empty() {
        return Ok(ScanResult {
            added: 0,
            updated: 0,
            deleted: 0,
            total: 0,
        });
    }

    let mut added = 0;
    let mut updated = 0;
    let mut deleted = 0;

    // 2. Scan each path
    for root_path in &target_paths {
        let root_p = Path::new(root_path);
        if !root_p.exists() || !root_p.is_dir() {
            continue;
        }

        // Add/Get folder ID
        conn.execute(
            "INSERT OR IGNORE INTO folders (path) VALUES (?1)",
            params![root_path],
        )?;
        let folder_id: i64 = conn.query_row(
            "SELECT id FROM folders WHERE path = ?1",
            params![root_path],
            |row| row.get(0),
        )?;

        // Gather current DB records in this folder
        let mut db_stmt = conn.prepare(
            "SELECT path, mtime, size, pinyin_index_version FROM files WHERE folder_id = ?1",
        )?;
        let db_files_iter = db_stmt.query_map(params![folder_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i32>(3)?,
                ),
            ))
        })?;

        let mut db_files: HashMap<String, (i64, i64, i32)> = HashMap::new();
        for file in db_files_iter {
            let (p, m) = file?;
            db_files.insert(p, m);
        }

        let mut scanned_paths = HashMap::new();
        let mut traversal_complete = true;

        // Traverse filesystem using walkdir
        for entry in walk(root_p) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    traversal_complete = false;
                    continue;
                }
            };
            if entry.file_type().is_file() {
                let file_path = entry.path();
                let file_path_str = file_path.to_string_lossy().to_string();

                // Get extension
                if let Some(ext) = file_path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_lowercase())
                {
                    let file_type = if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                        Some("audio")
                    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
                        Some("video")
                    } else {
                        None
                    };

                    if let Some(ftype) = file_type {
                        // Gather file metadata
                        let metadata = match entry.metadata() {
                            Ok(metadata) => metadata,
                            Err(_) => {
                                traversal_complete = false;
                                continue;
                            }
                        };
                        let file_size = metadata.len() as i64;
                        let mtime = metadata
                            .modified()
                            .unwrap_or(SystemTime::now())
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;

                        let filename = file_path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        scanned_paths.insert(file_path_str.clone(), (mtime, file_size));

                        // Zero duration is a cached unknown/unsupported value, not
                        // evidence that a file changed. In particular Lofty cannot
                        // read our video formats; retrying zero on every scan made
                        // each unchanged video count as an update forever.
                        if let Some(&(db_mtime, db_size, db_pinyin_index_version)) =
                            db_files.get(&file_path_str)
                        {
                            if db_mtime != mtime || db_size != file_size {
                                // Extract duration
                                let mut duration_secs = 0.0;
                                if let Ok(tagged_file) =
                                    lofty::probe::Probe::open(&file_path_str).and_then(|p| p.read())
                                {
                                    use lofty::file::AudioFile;
                                    duration_secs =
                                        tagged_file.properties().duration().as_secs_f64();
                                }

                                // Generate pinyin index for search
                                let stem = file_path
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string();
                                let pinyin_index = build_pinyin_search_index(&stem);

                                conn.execute(
                                    "UPDATE files SET size = ?1, mtime = ?2, pinyin_index = ?3, duration_secs = ?4, pinyin_index_version = ?5 WHERE path = ?6",
                                    params![file_size, mtime, pinyin_index, duration_secs, PINYIN_INDEX_VERSION, file_path_str],
                                )?;
                                updated += 1;
                            } else if db_pinyin_index_version < PINYIN_INDEX_VERSION {
                                // A search-index upgrade is not a changed media
                                // file: do not probe duration or inflate updates.
                                let stem =
                                    file_path.file_stem().unwrap_or_default().to_string_lossy();
                                conn.execute(
                                    "UPDATE files SET pinyin_index = ?1, pinyin_index_version = ?2 WHERE path = ?3",
                                    params![build_pinyin_search_index(&stem), PINYIN_INDEX_VERSION, file_path_str],
                                )?;
                            }
                        } else {
                            // Extract duration
                            let mut duration_secs = 0.0;
                            if let Ok(tagged_file) =
                                lofty::probe::Probe::open(&file_path_str).and_then(|p| p.read())
                            {
                                use lofty::file::AudioFile;
                                duration_secs = tagged_file.properties().duration().as_secs_f64();
                            }

                            // New file
                            let stem = file_path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let pinyin_index = build_pinyin_search_index(&stem);
                            let filename_lower = filename.to_lowercase();

                            conn.execute(
                                "INSERT OR REPLACE INTO files (folder_id, path, filename, filename_lower, pinyin_index, pinyin_index_version, size, mtime, file_type, duration_secs)
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                                params![folder_id, file_path_str, filename, filename_lower, pinyin_index, PINYIN_INDEX_VERSION, file_size, mtime, ftype, duration_secs],
                            )?;
                            added += 1;
                        }
                    }
                }
            }
        }

        // A missing entry only proves deletion after a complete traversal.
        // An inaccessible subtree must not erase its existing index records.
        if traversal_complete {
            for (db_path, _) in db_files {
                if !scanned_paths.contains_key(&db_path) {
                    conn.execute("DELETE FROM files WHERE path = ?1", params![db_path])?;
                    deleted += 1;
                }
            }
        }
    }

    // Get total count
    let total_i64: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files f
         JOIN folders fold ON fold.id = f.folder_id
         WHERE fold.path IN (SELECT fp.path FROM favorite_paths fp
                             JOIN favorites f ON f.id = fp.favorite_id
                             WHERE f.name = ?1)",
        params![name],
        |row| row.get(0),
    )?;
    let total = total_i64 as usize;

    Ok(ScanResult {
        added,
        updated,
        deleted,
        total,
    })
}

#[tauri::command]
async fn reset_file_index<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, FileIndexWriteLock>,
) -> Result<()> {
    let db_path = get_db_path(&app)?;
    state
        .run(move || {
            let mut conn = Connection::open(db_path)?;
            reset_file_index_in_connection(&mut conn)?;
            if let Err(error) = db_maintenance::reclaim_unused_space(&conn) {
                eprintln!("Index database maintenance skipped: {error}");
            }
            Ok(())
        })
        .await
}

fn reset_file_index_in_connection(conn: &mut Connection) -> Result<()> {
    // This transaction clears cached file metadata only. Favorite definitions,
    // their path mappings and the underlying material files are untouched.
    let transaction = conn.transaction()?;
    transaction.execute("DELETE FROM files", [])?;
    transaction.execute("DELETE FROM folders", [])?;
    transaction.commit()?;
    Ok(())
}

const SEARCH_PAGE_SIZE: usize = 200;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilePage {
    records: Vec<FileRecord>,
    has_more: bool,
}

#[tauri::command]
async fn search_files<R: Runtime>(
    app: AppHandle<R>,
    favorite_name: String,
    query: String,
    file_type: String,
    sort_field: String,
    sort_dir: String,
    offset: Option<u32>,
) -> Result<SearchFilePage> {
    let db_path = get_db_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let conn = Connection::open(&db_path)?;
        search_file_page(
            &conn,
            &favorite_name,
            &query,
            &file_type,
            &sort_field,
            &sort_dir,
            offset.unwrap_or(0),
        )
    })
    .await
    .map_err(|error| AppError::Database(format!("搜索任务失败: {error}")))?
}

fn search_file_page(
    conn: &Connection,
    favorite_name: &str,
    query: &str,
    file_type: &str,
    sort_field: &str,
    sort_dir: &str,
    offset: u32,
) -> Result<SearchFilePage> {
    let favorite_name = favorite_name.trim();
    if favorite_name.is_empty() {
        return Ok(SearchFilePage {
            records: Vec::new(),
            has_more: false,
        });
    }
    let sort_column = match sort_field {
        "duration" => "duration_secs",
        "mtime" => "mtime",
        _ => "filename_lower",
    };
    let sort_direction = if sort_dir == "desc" { "DESC" } else { "ASC" };
    let mut sql = String::from(
        "SELECT files.path, files.filename, files.size, files.mtime, files.file_type, files.duration_secs
         FROM files
         JOIN folders ON folders.id = files.folder_id
         JOIN favorite_paths fp ON fp.path = folders.path
         JOIN favorites fav ON fav.id = fp.favorite_id
         WHERE fav.name = ?"
    );
    let mut values = vec![rusqlite::types::Value::Text(favorite_name.to_owned())];
    if file_type != "all" {
        sql.push_str(" AND files.file_type = ?");
        values.push(rusqlite::types::Value::Text(file_type.to_owned()));
    }
    let query = FilenameQuery::compile(query);
    // Filtering happens before the page offset. SQLite supplies the requested
    // stable order, while the matcher never reads the old expanded pinyin blob.
    sql.push_str(&format!(
        " ORDER BY files.{} {}, files.filename_lower ASC, files.path ASC",
        sort_column, sort_direction
    ));
    if query.terms.is_empty() {
        sql.push_str(&format!(" LIMIT {} OFFSET ?", SEARCH_PAGE_SIZE + 1));
        values.push(rusqlite::types::Value::Integer(offset as i64));
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values), |row| {
        Ok(FileRecord {
            path: row.get(0)?,
            filename: row.get(1)?,
            size: row.get(2)?,
            mtime: row.get(3)?,
            file_type: row.get(4)?,
            duration_secs: row.get(5)?,
        })
    })?;
    let mut records = Vec::with_capacity(SEARCH_PAGE_SIZE + 1);
    let mut matching_offset = if query.terms.is_empty() {
        0
    } else {
        offset as usize
    };
    for record in rows {
        let record = record?;
        if !query.matches(&record.filename) {
            continue;
        }
        if matching_offset > 0 {
            matching_offset -= 1;
            continue;
        }
        records.push(record);
        if records.len() > SEARCH_PAGE_SIZE {
            break;
        }
    }
    let has_more = records.len() > SEARCH_PAGE_SIZE;
    if has_more {
        records.pop();
    }
    Ok(SearchFilePage { records, has_more })
}

#[tauri::command]
async fn select_directory() -> Result<Option<String>> {
    let result = rfd::FileDialog::new().pick_folder();
    Ok(result.map(|p| p.to_string_lossy().to_string()))
}

fn validate_file_path(path: &str) -> Result<PathBuf> {
    let path_buf = PathBuf::from(path);
    if !path_buf.is_file() {
        return Err(AppError::Io(format!("File does not exist: {}", path)));
    }

    if path_buf.is_absolute() {
        Ok(path_buf)
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(path_buf))
            .map_err(|e| AppError::Io(e.to_string()))
    }
}

fn wide_null(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn format_file_time(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0usize;
    while size >= 1024.0 && idx + 1 < units.len() {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", bytes, units[idx])
    } else {
        format!("{:.2} {}", size, units[idx])
    }
}

fn format_duration_secs(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "-".to_string();
    }
    let total = secs.round() as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

fn propvariant_to_u32(propvar: &PROPVARIANT) -> Option<u32> {
    unsafe {
        PropVariantToUInt32(propvar)
            .ok()
            .or_else(|| PropVariantToUInt64(propvar).ok().map(|v| v as u32))
    }
}

fn propvariant_to_u64(propvar: &PROPVARIANT) -> Option<u64> {
    unsafe {
        PropVariantToUInt64(propvar)
            .ok()
            .or_else(|| PropVariantToUInt32(propvar).ok().map(|v| v as u64))
    }
}

fn propvariant_to_f64(propvar: &PROPVARIANT) -> Option<f64> {
    unsafe { PropVariantToDouble(propvar).ok() }
}

struct PropVariantGuard(PROPVARIANT);

impl Drop for PropVariantGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = PropVariantClear(&mut self.0);
        }
    }
}

struct ComGuard {
    should_uninitialize: bool,
}

impl ComGuard {
    fn new() -> Result<Self> {
        unsafe {
            let result = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if result == RPC_E_CHANGED_MODE {
                return Ok(Self {
                    should_uninitialize: false,
                });
            }
            result
                .ok()
                .map_err(|e| AppError::Shell(format!("{:?}", e)))?;
        }
        Ok(Self {
            should_uninitialize: true,
        })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe {
                CoUninitialize();
            }
        }
    }
}

#[tauri::command]
async fn reveal_file(path: String) -> Result<()> {
    let file_path = validate_file_path(&path)?;
    let file = wide_null("explorer.exe");
    let operation = wide_null("open");
    let parameters = wide_null(format!("/select,\"{}\"", file_path.to_string_lossy()));

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        return Err(AppError::Shell(format!(
            "ShellExecuteW failed with code {}",
            result
        )));
    }

    Ok(())
}

#[tauri::command]
async fn open_file_with(path: String) -> Result<()> {
    let file_path = validate_file_path(&path)?;
    let file = wide_null("rundll32.exe");
    let operation = wide_null("open");
    let parameters = wide_null(format!(
        "shell32.dll,OpenAs_RunDLL {}",
        file_path.to_string_lossy()
    ));
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;

    if result <= 32 {
        return Err(AppError::Shell(format!(
            "ShellExecuteW failed with code {}",
            result
        )));
    }
    Ok(())
}

fn push_field(fields: &mut Vec<FilePropertyItem>, label: &str, value: impl Into<String>) {
    fields.push(FilePropertyItem {
        label: label.to_string(),
        value: value.into(),
    });
}

#[tauri::command]
async fn get_file_properties(path: String) -> Result<FilePropertyInfo> {
    let file_path = validate_file_path(&path)?;
    let metadata = fs::metadata(&file_path).map_err(|e| AppError::Io(e.to_string()))?;
    let filename = file_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let created_time = metadata
        .created()
        .ok()
        .map(format_file_time)
        .unwrap_or_else(|| "-".to_string());
    let modified_time = metadata
        .modified()
        .ok()
        .map(format_file_time)
        .unwrap_or_else(|| "-".to_string());
    let size = format_size(metadata.len());

    let ext = file_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let media_kind = if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        "audio"
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        "video"
    } else {
        "file"
    };

    let mut duration = "-".to_string();
    let mut bitrate = "-".to_string();
    let mut sample_rate = "-".to_string();
    let mut channels = "-".to_string();
    let mut resolution = "-".to_string();
    let mut frame_rate = "-".to_string();

    if media_kind == "audio" {
        if let Ok(tagged_file) = lofty::probe::Probe::open(&file_path).and_then(|p| p.read()) {
            use lofty::file::AudioFile;
            let props = tagged_file.properties();
            duration = format_duration_secs(props.duration().as_secs_f64());
            if let Some(v) = props.audio_bitrate().or(props.overall_bitrate()) {
                bitrate = format!("{} kbps", v);
            }
            if let Some(v) = props.sample_rate() {
                sample_rate = format!("{} Hz", v);
            }
            if let Some(v) = props.channels() {
                channels = v.to_string();
            }
        }
    }

    let _com = ComGuard::new()?;
    unsafe {
        let path_wide = wide_null(file_path.as_os_str());
        let store: IPropertyStore = SHGetPropertyStoreFromParsingName(
            PCWSTR::from_raw(path_wide.as_ptr()),
            None,
            GPS_DEFAULT,
        )
        .map_err(|e| AppError::Shell(e.to_string()))?;

        let read_u32 = |key: &windows::Win32::Foundation::PROPERTYKEY| -> Option<u32> {
            let prop = store.GetValue(key).ok()?;
            let _guard = PropVariantGuard(prop);
            propvariant_to_u32(&_guard.0)
        };
        let read_f64 = |key: &windows::Win32::Foundation::PROPERTYKEY| -> Option<f64> {
            let prop = store.GetValue(key).ok()?;
            let _guard = PropVariantGuard(prop);
            propvariant_to_f64(&_guard.0)
        };
        let read_u64 = |key: &windows::Win32::Foundation::PROPERTYKEY| -> Option<u64> {
            let prop = store.GetValue(key).ok()?;
            let _guard = PropVariantGuard(prop);
            propvariant_to_u64(&_guard.0)
        };

        if media_kind == "video" {
            if let (Some(w), Some(h)) = (
                read_u32(&PKEY_Video_FrameWidth),
                read_u32(&PKEY_Video_FrameHeight),
            ) {
                resolution = format!("{}x{}", w, h);
            }
            if let Some(fr) = read_f64(&PKEY_Video_FrameRate) {
                let fps = if fr > 1000.0 { fr / 1000.0 } else { fr };
                frame_rate = format!("{:.3} fps", fps);
            } else if let Some(fr) = read_u32(&PKEY_Video_FrameRate) {
                frame_rate = format!("{:.3} fps", fr as f64 / 1000.0);
            }
            if let Some(v) =
                read_u32(&PKEY_Video_TotalBitrate).or_else(|| read_u32(&PKEY_Video_EncodingBitrate))
            {
                bitrate = format!("{} kbps", v / 1000);
            }
            if let Some(v) = read_u64(&PKEY_Media_Duration) {
                duration = format_duration_secs((v as f64) / 10_000_000.0);
            }
        }
    }

    let mut fields = Vec::new();
    push_field(&mut fields, "文件名", filename.clone());
    push_field(&mut fields, "路径", file_path.to_string_lossy().to_string());
    push_field(&mut fields, "创建时间", created_time.clone());
    push_field(&mut fields, "修改时间", modified_time.clone());
    push_field(&mut fields, "大小", size.clone());
    push_field(&mut fields, "时长", duration.clone());
    push_field(
        &mut fields,
        if media_kind == "video" {
            "码率"
        } else {
            "比特率"
        },
        bitrate.clone(),
    );
    if media_kind == "audio" {
        push_field(&mut fields, "采样率", sample_rate.clone());
        push_field(&mut fields, "声道", channels.clone());
    }
    if media_kind == "video" {
        push_field(&mut fields, "分辨率", resolution.clone());
        push_field(&mut fields, "帧速率", frame_rate.clone());
    }

    Ok(FilePropertyInfo {
        filename,
        path: file_path.to_string_lossy().to_string(),
        created_time,
        modified_time,
        size,
        duration,
        bitrate,
        sample_rate,
        channels,
        resolution,
        frame_rate,
        media_kind: media_kind.to_string(),
        fields,
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let dock_state: SharedDockState = Arc::new(Mutex::new(WindowDockState::default()));

    tauri::Builder::default()
        .manage(dock_state.clone())
        .manage(FileIndexWriteLock::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_filename(WINDOW_STATE_FILENAME)
                .with_state_flags(window_state_flags())
                .skip_initial_state("main")
                .build(),
        )
        .setup({
            let dock_state = dock_state.clone();
            move |app| {
                // Never restore a legacy system titlebar over our custom controls.
                if let Some(window) = main_window(app.handle()) {
                    window.set_decorations(false)?;
                    if let Some(icon) = app.default_window_icon() {
                        window.set_icon(icon.clone())?;
                    }
                    restore_initial_window(&window)?;
                }
                // Retrieve path to sqlite database and initialize schemas, mapping errors to String representation
                let db_path = get_db_path(&app.handle()).map_err(|e| e.to_string())?;
                init_db(&db_path).map_err(|e| e.to_string())?;
                setup_tray(app, dock_state.clone())?;
                dock_state.lock().unwrap().geometry_changed_at = Some(Instant::now());
                start_edge_reveal_worker(app.handle().clone(), dock_state.clone());
                Ok(())
            }
        })
        .on_window_event({
            let dock_state = dock_state.clone();
            move |window, event| {
                if window.label() != "main" {
                    return;
                }

                match event {
                    WindowEvent::CloseRequested { api, .. } => {
                        let allow_quit = dock_state
                            .lock()
                            .map(|state| state.allow_quit)
                            .unwrap_or(false);

                        if !allow_quit {
                            api.prevent_close();
                            if let Some(main) = main_window(window.app_handle()) {
                                save_current_window_position(&main, &dock_state);
                                persist_window_state(&main);
                            }
                            let _ = window.hide();
                        }
                    }
                    WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                        dock_state.lock().unwrap().geometry_changed_at = Some(Instant::now());
                    }
                    _ => {}
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            begin_window_drag,
            get_window_pin_state,
            toggle_window_pin,
            add_favorite,
            get_favorites,
            set_default_favorite,
            delete_favorite,
            scan_favorite,
            reset_file_index,
            search_files,
            select_directory,
            reveal_file,
            open_file_with,
            get_file_properties
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_window_default_is_exact_work_area_fraction() {
        for (width, height) in [(1920, 1040), (3200, 1904), (1280, 680), (320, 240)] {
            assert_eq!(
                initial_window_size(PhysicalSize::new(width, height)),
                PhysicalSize::new(width / 4, height * 3 / 4)
            );
        }
    }

    #[test]
    fn old_geometry_cannot_bypass_first_proportional_launch() {
        let state = |width, height| {
            serde_json::to_vec(&serde_json::json!({"main": {"width": width, "height": height}}))
                .unwrap()
        };
        for (width, height) in [(1596, 1900), (1200, 800), (900, 650)] {
            assert!(!has_custom_saved_window_size(&state(width, height), false));
            assert!(has_custom_saved_window_size(&state(width, height), true));
        }
        assert!(!has_custom_saved_window_size(&state(0, 800), true));
        assert!(!has_custom_saved_window_size(b"invalid", true));
    }

    struct TestMaterialDirectory(PathBuf);

    impl TestMaterialDirectory {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "otto-sampler-reset-index-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestMaterialDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn partial_traversal_keeps_unseen_index_records_but_indexes_accessible_files() {
        let directory = TestMaterialDirectory::new();
        for name in ["seen.wav", "inaccessible.wav", "deleted.wav"] {
            fs::write(directory.0.join(name), b"test-only media").unwrap();
        }
        let conn = init_db(Path::new(":memory:")).unwrap();
        conn.execute(
            "INSERT INTO favorites (id, name, is_default) VALUES (1, 'library', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO favorite_paths (favorite_id, path) VALUES (1, ?1)",
            params![directory.0.to_string_lossy().to_string()],
        )
        .unwrap();
        assert_eq!(
            scan_favorite_in_connection(&conn, "library").unwrap().added,
            3
        );
        fs::remove_file(directory.0.join("deleted.wav")).unwrap();
        fs::write(directory.0.join("new.wav"), b"new test-only media").unwrap();
        fs::write(directory.0.join("seen.wav"), b"changed test-only media").unwrap();

        let partial = scan_favorite_with_walker(&conn, "library", |root| {
            // Inject the shape of a failed subtree enumeration without relying
            // on OS-specific ACL setup or the privileges of the test account.
            let mut entries: Vec<_> = WalkDir::new(root)
                .into_iter()
                .filter(|entry| {
                    entry
                        .as_ref()
                        .map_or(true, |entry| entry.file_name() != "inaccessible.wav")
                })
                .collect();
            let error = WalkDir::new(root.join("missing-test-subtree"))
                .into_iter()
                .next()
                .unwrap()
                .unwrap_err();
            entries.push(Err(error));
            entries
        })
        .unwrap();
        assert_eq!(
            (
                partial.added,
                partial.updated,
                partial.deleted,
                partial.total
            ),
            (1, 1, 0, 4)
        );
        let retained: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE filename IN ('inaccessible.wav', 'deleted.wav')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 2);

        // Once traversal succeeds, actual deletion is safe to identify; the
        // temporarily unseen but still existing file remains indexed.
        let complete = scan_favorite_in_connection(&conn, "library").unwrap();
        assert_eq!(
            (
                complete.added,
                complete.updated,
                complete.deleted,
                complete.total
            ),
            (0, 0, 1, 3)
        );
        let retained: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE filename='inaccessible.wav'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 1);
    }

    #[test]
    fn unchanged_unknown_durations_do_not_repeat_updates() {
        let directory = TestMaterialDirectory::new();
        // Unsupported video containers and unreadable audio both legitimately
        // have an unknown duration. Neither should be rewritten on every scan.
        let filenames = [
            "one.mp4",
            "two.mkv",
            "three.avi",
            "four.mov",
            "five.wmv",
            "six.webm",
            "seven.wav",
            "eight.mp3",
        ];
        for filename in filenames {
            fs::write(directory.0.join(filename), b"test-only unsupported media").unwrap();
        }
        let conn = init_db(Path::new(":memory:")).unwrap();
        conn.execute(
            "INSERT INTO favorites (id, name, is_default) VALUES (1, 'library', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO favorite_paths (favorite_id, path) VALUES (1, ?1)",
            params![directory.0.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute_batch("CREATE TABLE update_audit(path TEXT); CREATE TRIGGER record_file_update AFTER UPDATE ON files BEGIN INSERT INTO update_audit VALUES (NEW.path); END;").unwrap();

        let first = scan_favorite_in_connection(&conn, "library").unwrap();
        assert_eq!((first.added, first.updated, first.total), (8, 0, 8));
        let unknown: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE duration_secs=0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unknown, 8);
        for _ in 0..3 {
            let unchanged = scan_favorite_in_connection(&conn, "library").unwrap();
            assert_eq!(
                (
                    unchanged.added,
                    unchanged.updated,
                    unchanged.deleted,
                    unchanged.total
                ),
                (0, 0, 0, 8)
            );
        }
        let writes: i64 = conn
            .query_row("SELECT COUNT(*) FROM update_audit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(writes, 0, "unchanged files must not even be rewritten");

        // Size changes remain detectable even on filesystems whose modification
        // timestamp resolution is coarser than the interval between scans.
        fs::write(
            directory.0.join("one.mp4"),
            b"changed test-only unsupported media",
        )
        .unwrap();
        let changed = scan_favorite_in_connection(&conn, "library").unwrap();
        assert_eq!((changed.added, changed.updated, changed.total), (0, 1, 8));
        assert_eq!(
            scan_favorite_in_connection(&conn, "library")
                .unwrap()
                .updated,
            0
        );
        let writes: i64 = conn
            .query_row("SELECT COUNT(*) FROM update_audit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(writes, 1);
    }

    #[test]
    fn reset_index_preserves_favorites_materials_and_allows_a_fresh_scan() {
        let directory = TestMaterialDirectory::new();
        let material = directory.0.join("reset-test.wav");
        let contents = b"test-only indexed material";
        fs::write(&material, contents).unwrap();
        let mut conn = init_db(Path::new(":memory:")).unwrap();
        conn.execute(
            "INSERT INTO favorites (id, name, is_default) VALUES (1, 'library', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO favorite_paths (favorite_id, path) VALUES (1, ?1)",
            params![directory.0.to_string_lossy().to_string()],
        )
        .unwrap();
        let original = scan_favorite_in_connection(&conn, "library").unwrap();
        assert_eq!(original.added, 1);
        assert_eq!(original.total, 1);

        reset_file_index_in_connection(&mut conn).unwrap();
        for table in ["files", "folders"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
        let favorite: (String, bool) = conn
            .query_row("SELECT name, is_default FROM favorites", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(favorite, ("library".to_owned(), true));
        let retained_path: String = conn
            .query_row(
                "SELECT path FROM favorite_paths WHERE favorite_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained_path, directory.0.to_string_lossy());
        assert_eq!(fs::read(&material).unwrap(), contents);

        let rebuilt = scan_favorite_in_connection(&conn, "library").unwrap();
        assert_eq!(rebuilt.added, 1);
        assert_eq!(rebuilt.updated, 0);
        assert_eq!(rebuilt.total, 1);
        let results =
            search_file_page(&conn, "library", "reset", "all", "filename", "asc", 0).unwrap();
        assert_eq!(results.records.len(), 1);
        assert_eq!(results.records[0].path, material.to_string_lossy());
        assert_eq!(fs::read(&material).unwrap(), contents);
    }

    #[test]
    fn reset_index_rolls_back_if_either_table_cannot_be_cleared() {
        let mut conn = search_fixture();
        conn.execute_batch("CREATE TRIGGER reject_folder_delete BEFORE DELETE ON folders BEGIN SELECT RAISE(ABORT, 'test rejection'); END;").unwrap();
        assert!(reset_file_index_in_connection(&mut conn).is_err());
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        let folders: i64 = conn
            .query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))
            .unwrap();
        assert_eq!(files, 902);
        assert_eq!(folders, 2);
    }

    #[test]
    fn index_writer_rejects_overlapping_operations_instead_of_queueing_them() {
        let writer = FileIndexWriteLock::default();
        let first_writer = writer.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first = tauri::async_runtime::spawn(async move {
            first_writer
                .run(move || {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                })
                .await
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let rejected = tauri::async_runtime::block_on(writer.run(|| -> Result<()> {
            panic!("an overlapping index operation must not be scheduled")
        }));
        // Release the first operation before asserting, including on failure.
        release_tx.send(()).unwrap();
        tauri::async_runtime::block_on(first).unwrap().unwrap();
        assert!(rejected
            .unwrap_err()
            .to_string()
            .contains("扫描或索引重置进行中"));
        tauri::async_runtime::block_on(writer.run(|| Ok(()))).unwrap();
        let failed: Result<()> = tauri::async_runtime::block_on(
            writer.run(|| Err(AppError::Database("test failure".into()))),
        );
        assert!(failed.is_err());
        tauri::async_runtime::block_on(writer.run(|| Ok(()))).unwrap();
    }

    fn search_fixture() -> Connection {
        let conn = init_db(Path::new(":memory:")).unwrap();
        conn.execute_batch(
            "INSERT INTO favorites (id, name) VALUES (1, 'library'), (2, 'other');
             INSERT INTO folders (id, path) VALUES (1, '/samples'), (2, '/outside');
             INSERT INTO favorite_paths (favorite_id, path) VALUES (1, '/samples'), (2, '/outside');
             BEGIN;",
        )
        .unwrap();
        for index in 0..901 {
            let filename = if index < 880 {
                "测试 match.wav"
            } else {
                "其他 other.wav"
            };
            let pinyin = if index < 880 { ";ceshi;" } else { ";qita;" };
            let file_type = if index % 2 == 0 { "audio" } else { "video" };
            conn.execute(
                "INSERT INTO files (folder_id, path, filename, filename_lower, pinyin_index, size, mtime, file_type, duration_secs)
                 VALUES (1, ?1, ?2, ?2, ?3, 1, 100, ?4, 2.0)",
                params![format!("/samples/{index:04}.wav"), filename, pinyin, file_type],
            ).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO files (folder_id, path, filename, filename_lower, pinyin_index, size, mtime, file_type, duration_secs)
             VALUES (2, '/outside/match.wav', 'match.wav', 'match.wav', ';ceshi;', 1, 100, 'audio', 2.0);
             COMMIT;"
        ).unwrap();
        conn
    }

    #[test]
    fn search_sql_pages_every_query_and_filter_branch_without_gaps_or_duplicates() {
        let conn = search_fixture();
        for (query, file_type, total) in [
            ("", "all", 901usize),
            ("match", "all", 880),
            ("", "audio", 451),
            ("match", "audio", 440),
            ("ceshi", "all", 880),
            ("ceshi", "audio", 440),
            ("missing", "all", 0),
        ] {
            for sort_field in ["filename", "duration", "mtime"] {
                for sort_dir in ["asc", "desc"] {
                    let mut paths = Vec::new();
                    let mut offset = 0;
                    loop {
                        let page = search_file_page(
                            &conn, "library", query, file_type, sort_field, sort_dir, offset,
                        )
                        .unwrap();
                        let remaining = total.saturating_sub(offset as usize);
                        assert_eq!(page.records.len(), remaining.min(SEARCH_PAGE_SIZE));
                        assert_eq!(page.has_more, remaining > SEARCH_PAGE_SIZE);
                        paths.extend(page.records.into_iter().map(|record| record.path));
                        if !page.has_more {
                            break;
                        }
                        offset += SEARCH_PAGE_SIZE as u32;
                    }
                    assert_eq!(paths.len(), total);
                    assert_eq!(paths.iter().collect::<HashSet<_>>().len(), total);
                    assert!(!paths.iter().any(|path| path.starts_with("/outside")));
                    if file_type == "audio" && !query.is_empty() {
                        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
                    }
                    let empty = search_file_page(
                        &conn,
                        "library",
                        query,
                        file_type,
                        sort_field,
                        sort_dir,
                        total as u32,
                    )
                    .unwrap();
                    assert!(empty.records.is_empty());
                    assert!(!empty.has_more);
                }
            }
        }
        let huge_offset =
            search_file_page(&conn, "library", "", "all", "mtime", "desc", u32::MAX).unwrap();
        assert!(huge_offset.records.is_empty());
        assert!(!huge_offset.has_more);
        let no_favorite = search_file_page(&conn, "  ", "", "all", "filename", "asc", 0).unwrap();
        assert!(no_favorite.records.is_empty());
        assert!(!no_favorite.has_more);
    }

    #[test]
    fn search_lookahead_distinguishes_an_exact_full_last_page() {
        let conn = search_fixture();
        conn.execute(
            "DELETE FROM files WHERE path >= '/samples/0400.wav' AND folder_id = 1",
            [],
        )
        .unwrap();
        let first = search_file_page(&conn, "library", "match", "all", "mtime", "desc", 0).unwrap();
        assert_eq!(first.records.len(), 200);
        assert!(first.has_more);
        let last =
            search_file_page(&conn, "library", "match", "all", "mtime", "desc", 200).unwrap();
        assert_eq!(last.records.len(), 200);
        assert!(!last.has_more);
        let end = search_file_page(&conn, "library", "match", "all", "mtime", "desc", 400).unwrap();
        assert!(end.records.is_empty());
        assert!(!end.has_more);
    }

    fn sample_drag() -> WindowDrag {
        WindowDrag {
            cursor_origin: PhysicalPosition::new(-960.0, 40.0),
            window_origin: PhysicalPosition::new(-1920, 0),
            initial_size: PhysicalSize::new(1920, 1080),
            threshold: 8.0,
            was_maximized: true,
            moving: false,
            started_at: Instant::now(),
        }
    }

    #[test]
    fn custom_drag_uses_physical_deltas_across_negative_display_coordinates() {
        let drag = sample_drag();
        assert!(!exceeds_drag_threshold(
            drag,
            PhysicalPosition::new(-955.0, 44.0)
        ));
        assert!(exceeds_drag_threshold(
            drag,
            PhysicalPosition::new(-952.0, 40.0)
        ));
        assert_eq!(
            drag_target(drag, PhysicalPosition::new(100.0, 80.0)),
            PhysicalPosition::new(-860, 40)
        );
        assert_eq!(
            restored_drag_origin(drag, PhysicalSize::new(1000, 700)),
            PhysicalPosition::new(-1460, 0)
        );
    }

    #[test]
    fn custom_drag_cleanup_releases_session_and_only_reconciles_after_movement() {
        let state = Arc::new(Mutex::new(WindowDockState::default()));
        state.lock().unwrap().drag = Some(sample_drag());
        finish_window_drag(&state);
        assert!(state.lock().unwrap().drag.is_none());
        assert!(state.lock().unwrap().geometry_changed_at.is_none());
        let mut drag = sample_drag();
        drag.moving = true;
        state.lock().unwrap().drag = Some(drag);
        finish_window_drag(&state);
        assert!(state.lock().unwrap().drag.is_none());
        assert!(state.lock().unwrap().geometry_changed_at.is_some());
    }

    #[test]
    fn releasing_a_docked_drag_waits_for_geometry_before_any_hover_transition() {
        let now = Instant::now();
        let mut state = WindowDockState::default();
        state.placement = dock_placement(
            PhysicalPosition::new(0, 100),
            PhysicalSize::new(800, 600),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1920, 1080),
        );
        let mut drag = sample_drag();
        drag.moving = true;
        state.drag = Some(drag);
        assert_eq!(dock_tick(&state, now), DockTick::Wait);
        state.drag = None;
        state.geometry_changed_at = Some(now);
        // The cursor may now be far from the old panel. No hide/reveal decision
        // may use that panel during the geometry settle period.
        assert_eq!(
            dock_tick(&state, now + Duration::from_millis(149)),
            DockTick::Wait
        );
        assert_eq!(
            dock_tick(&state, now + Duration::from_millis(150)),
            DockTick::ReconcileGeometry
        );
        state.placement = None;
        state.geometry_changed_at = None;
        assert_eq!(
            dock_tick(&state, now + Duration::from_millis(151)),
            DockTick::Hover
        );
    }

    #[test]
    fn dock_survives_a_drag_completely_past_either_edge() {
        let monitor = PhysicalPosition::new(0, 0);
        let screen = PhysicalSize::new(1920, 1080);
        let size = PhysicalSize::new(800, 600);
        let left =
            dock_placement(PhysicalPosition::new(-1200, -100), size, monitor, screen).unwrap();
        assert_eq!(left.side, DockSide::Left);
        assert_eq!(left.visible_position, PhysicalPosition::new(0, 0));
        assert_eq!(left.hidden_position.x + size.width as i32, EDGE_VISIBLE_PX);
        let right =
            dock_placement(PhysicalPosition::new(2500, 900), size, monitor, screen).unwrap();
        assert_eq!(right.side, DockSide::Right);
        assert_eq!(right.visible_position, PhysicalPosition::new(1120, 480));
        assert_eq!(right.hidden_position.x, 1920 - EDGE_VISIBLE_PX);
        assert!(dock_placement(PhysicalPosition::new(400, 100), size, monitor, screen).is_none());
    }

    #[test]
    fn edge_reveal_stays_inside_its_display_and_panel_height() {
        let placement = dock_placement(
            PhysicalPosition::new(-1920, 100),
            PhysicalSize::new(800, 600),
            PhysicalPosition::new(-1920, 0),
            PhysicalSize::new(1920, 1080),
        )
        .unwrap();
        assert!(cursor_near_dock_edge(
            placement,
            PhysicalPosition::new(-1920.0, 200.0)
        ));
        assert!(!cursor_near_dock_edge(
            placement,
            PhysicalPosition::new(-1921.0, 200.0)
        ));
        assert!(!cursor_near_dock_edge(
            placement,
            PhysicalPosition::new(-1920.0, 99.0)
        ));
        assert!(!cursor_near_dock_edge(
            placement,
            PhysicalPosition::new(-1920.0, 700.0)
        ));
    }

    #[test]
    fn dock_pin_does_not_overwrite_the_user_preference() {
        let mut state = WindowDockState::default();
        assert!(!state.pin_state().always_on_top);
        state.placement = dock_placement(
            PhysicalPosition::new(0, 100),
            PhysicalSize::new(800, 600),
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(1920, 1080),
        );
        assert_eq!(
            state.pin_state(),
            WindowPinState {
                docked: true,
                always_on_top: true
            }
        );
        state.placement = None;
        assert!(!state.pin_state().always_on_top);
        state.user_always_on_top = true;
        assert!(state.pin_state().always_on_top);
        assert!(!state.pin_state().docked);
    }

    #[test]
    fn pinyin_index_keeps_polyphone_substrings() {
        let filename =
            "他的这个打野的走位我就觉得你妈逼离谱儿，你老他妈想着刷你那b三狼干什么玩意啊我操你妈的";
        assert!(FilenameQuery::compile("xiangzhe").matches(filename));
        assert!(FilenameQuery::compile("shuanina").matches(filename));
    }

    #[test]
    fn pinyin_matching_accepts_all_character_readings_without_initials() {
        assert!(FilenameQuery::compile("chong").matches("重.wav"));
        assert!(FilenameQuery::compile("zhong").matches("重.wav"));
        assert!(!FilenameQuery::compile("zhongchong").matches("重.wav"));
        assert!(FilenameQuery::compile("bei").matches("北京.wav"));
        assert!(!FilenameQuery::compile("bj").matches("北京.wav"));
    }

    #[test]
    fn whitespace_terms_require_distinct_source_characters() {
        for (query, filename, expected) in [
            ("ou shou", "shou.wav", false),
            ("ou shou", "hanshou.wav", false),
            ("shou ou", "hanshou.wav", false),
            ("ou shou", "欧内手.wav", true),
            ("shou ou", "欧内手.wav", true),
            ("ou shou", "shou_ou.wav", true),
            ("ou shou", "oushou.wav", true),
            ("shou shou", "shou.wav", false),
            ("shou shou", "shoushou.wav", true),
            ("手 shou", "手.wav", false),
            ("sh shou", "手.wav", false),
            ("手 shou", "手手.wav", true),
            ("zhong chong", "重.wav", false),
            ("zhong chong", "重重.wav", true),
            ("ni ha", "你好.wav", true),
            ("n *i", "你.wav", false),
            ("a* b", "ab.wav", true),
            ("*a b*", "ab.wav", true),
            ("a*b b", "ab.wav", false),
            ("a*b b", "abb.wav", true),
            ("a*ba b", "ababa.wav", true),
            ("a ab", "aba.wav", true),
            ("aba ba", "ababa.wav", true),
            ("* *", "a.wav", true),
            ("* shou", "shou.wav", true),
            ("_ _", "one_under.wav", false),
            ("_ _", "two_under_scores.wav", true),
            ("% _", "100%_kick.wav", true),
        ] {
            assert_eq!(
                FilenameQuery::compile(query).matches(filename),
                expected,
                "query={query:?}, name={filename:?}"
            );
        }
        let long = "zhong".repeat(30);
        assert!(FilenameQuery::compile(&format!("{long} chong"))
            .matches(&format!("{}.wav", "重".repeat(31))));
        assert!(!FilenameQuery::compile(&format!("{long} chong"))
            .matches(&format!("{}.wav", "重".repeat(30))));
        assert!(!FilenameQuery::compile(&vec!["a"; 30].join(" "))
            .matches(&format!("{}.ogg", "a".repeat(29))));
        let equivalent = "z zh zho zhon zhong c ch cho chon chong";
        assert!(!FilenameQuery::compile(equivalent).matches(&"重".repeat(9)));
        assert!(FilenameQuery::compile(equivalent).matches(&"重".repeat(10)));
    }

    #[test]
    fn multi_term_spans_agree_with_exhaustive_source_substrings() {
        fn names(prefix: &str, depth: usize, output: &mut Vec<String>) {
            output.push(prefix.to_owned());
            if depth != 0 {
                for c in ['a', 'b', '手', '重'] {
                    names(&format!("{prefix}{c}"), depth - 1, output);
                }
            }
        }
        let patterns = [
            "a", "b", "sh", "shou", "手", "重", "chong", "zhong", "*", "a*", "*b", "a*b", "*h",
            "*u",
        ];
        let terms: Vec<_> = patterns
            .iter()
            .map(|pattern| SearchTerm::compile(pattern))
            .collect();
        let mut filenames = Vec::new();
        names("", 3, &mut filenames);
        for filename in filenames {
            let mut boundaries: Vec<_> = filename.char_indices().map(|(i, _)| i).collect();
            boundaries.push(filename.len());
            let all_spans: Vec<Vec<(usize, usize)>> = terms
                .iter()
                .map(|term| {
                    let mut spans = Vec::new();
                    if term.tokens.iter().all(|&c| c == '*') {
                        spans.push((0, 0));
                    }
                    for start in 0..boundaries.len() - 1 {
                        for end in start + 1..boundaries.len() {
                            if term.matches(&cached_search_name(
                                &filename[boundaries[start]..boundaries[end]],
                            )) {
                                spans.push((start, end));
                            }
                        }
                    }
                    spans
                })
                .collect();
            for (i, first) in patterns.iter().enumerate() {
                for (j, second) in patterns.iter().enumerate() {
                    // Independent oracle: enumerate every source substring,
                    // then every pair of disjoint intervals. It deliberately
                    // does not use the labeled NFA or assignment algorithm.
                    let expected = all_spans[i]
                        .iter()
                        .any(|&(a, b)| all_spans[j].iter().any(|&(c, d)| b <= c || d <= a));
                    let query = format!("{first} {second}");
                    assert_eq!(
                        FilenameQuery::compile(&query).matches(&filename),
                        expected,
                        "query={query:?}, name={filename:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn filename_search_preserves_mixed_text_punctuation_and_wildcards() {
        for (query, filename, expected) in [
            ("KICK", "Kick Drum.wav", true),
            ("drum kick", "drum_big_kick.wav", true),
            ("kick drum", "drum_big_kick.wav", true),
            ("drumkick", "drum_big_kick.wav", false),
            ("鼓kick", "Kick Drum.wav", false),
            ("鼓kick", "大鼓Kick.wav", true),
            ("你hao", "你好.wav", true),
            ("ni好", "你好.wav", true),
            ("n好", "你好.wav", false),
            ("nihao", "你好世界.wav", true),
            ("nihaosh", "你好世界.wav", true),
            ("ihao", "你好.wav", false),
            ("*ihao", "你好.wav", true),
            ("zhongwn", "中文.wav", false),
            ("ni-hao", "你-好.wav", true),
            ("ni-hao", "你好.wav", false),
            ("nihao", "你-好.wav", false),
            ("kick*drum", "kick_big_drum.wav", true),
            ("kick*drum", "kickdrum.wav", true),
            ("kick*drum", "drum_kick.wav", false),
            ("[live]", "[Live].wav", true),
            ("[live]", "Live.wav", false),
            ("_", "has_under.wav", true),
            ("_", "NoUnder.wav", false),
            ("%", "100%.wav", true),
            ("%", "anything.wav", false),
            ("*", "你好.wav", true),
            ("**", "anything.wav", true),
            ("一二三四五六七八九十", "一二三四五六七八九十.wav", true),
            ("ersansiwuliuqibajiushi", "一二三四五六七八九十.wav", true),
        ] {
            assert_eq!(
                FilenameQuery::compile(query).matches(filename),
                expected,
                "query={query:?}, name={filename:?}"
            );
        }
    }

    #[test]
    fn pinyin_search_has_no_pronunciation_product_or_query_length_cutoff() {
        let filename = format!("{}.wav", "重".repeat(40));
        let query = "zhongchong".repeat(20);
        assert!(query.len() > 128);
        assert!(FilenameQuery::compile(&query).matches(&filename));
        assert!(!FilenameQuery::compile(&(query + "错")).matches(&filename));
        let compact = build_pinyin_search_index(&"重".repeat(40));
        let decoded: Vec<Vec<String>> = serde_json::from_str(&compact).unwrap();
        assert_eq!(decoded.len(), 40);
        assert!(compact.len() < 2_000);
    }

    #[test]
    fn search_filters_before_pagination_and_ignores_legacy_pinyin_payload() {
        let conn = init_db(Path::new(":memory:")).unwrap();
        conn.execute_batch(
            "INSERT INTO favorites(id,name) VALUES(1,'library');
             INSERT INTO folders(id,path) VALUES(1,'/samples');
             INSERT INTO favorite_paths(favorite_id,path) VALUES(1,'/samples');",
        )
        .unwrap();
        for index in 0..850 {
            let filename = if index < 400 {
                format!("a不匹配{index:04}.wav")
            } else {
                format!("z你好世界{index:04}.wav")
            };
            conn.execute(
                "INSERT INTO files(folder_id,path,filename,filename_lower,pinyin_index,size,mtime,file_type,duration_secs)
                 VALUES(1,?1,?2,?2,';incorrect-old-pinyin;',1,1,'audio',1)",
                params![format!("/samples/{index:04}.wav"), filename],
            ).unwrap();
        }
        let mut all = Vec::new();
        for (offset, expected, more) in [(0, 200, true), (200, 200, true), (400, 50, false)] {
            let page = search_file_page(
                &conn, "library", "nihaosh", "all", "filename", "asc", offset,
            )
            .unwrap();
            assert_eq!(page.records.len(), expected);
            assert_eq!(page.has_more, more);
            all.extend(page.records.into_iter().map(|row| row.path));
        }
        assert_eq!(all.len(), 450);
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), 450);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn index_only_upgrade_preserves_duration_and_media_update_count() {
        let directory = TestMaterialDirectory::new();
        fs::write(directory.0.join("你好.wav"), b"unreadable audio").unwrap();
        let conn = init_db(Path::new(":memory:")).unwrap();
        conn.execute("INSERT INTO favorites(name) VALUES('library')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO favorite_paths(favorite_id,path) VALUES(1,?1)",
            params![directory.0.to_string_lossy()],
        )
        .unwrap();
        assert_eq!(
            scan_favorite_in_connection(&conn, "library").unwrap().added,
            1
        );
        conn.execute(
            "UPDATE files SET pinyin_index_version=3, duration_secs=123",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER forbid_duration_probe BEFORE UPDATE OF duration_secs ON files
             BEGIN SELECT RAISE(FAIL,'index upgrade must not overwrite duration'); END;",
        )
        .unwrap();
        let upgraded = scan_favorite_in_connection(&conn, "library").unwrap();
        assert_eq!(
            (upgraded.added, upgraded.updated, upgraded.deleted),
            (0, 0, 0)
        );
        let (duration, version, compact): (f64, i32, String) = conn
            .query_row(
                "SELECT duration_secs,pinyin_index_version,pinyin_index FROM files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(duration, 123.0);
        assert_eq!(version, PINYIN_INDEX_VERSION);
        assert!(serde_json::from_str::<Vec<Vec<String>>>(&compact).is_ok());
        assert_eq!(
            scan_favorite_in_connection(&conn, "library")
                .unwrap()
                .updated,
            0
        );
    }

    #[test]
    #[ignore = "synthetic benchmark; run explicitly with --ignored --nocapture"]
    fn benchmark_filename_search_cold_and_warm() {
        for count in [6_000, 50_000] {
            let conn = init_db(Path::new(":memory:")).unwrap();
            conn.execute_batch(
                "INSERT INTO favorites(id,name) VALUES(1,'library');
                 INSERT INTO folders(id,path) VALUES(1,'/samples');
                 INSERT INTO favorite_paths(favorite_id,path) VALUES(1,'/samples');
                 BEGIN;",
            )
            .unwrap();
            for index in 0..count {
                let filename = format!("bench{count}编号{index:05}重庆你好世界 kick drum.wav");
                conn.execute(
                    "INSERT INTO files(folder_id,path,filename,filename_lower,pinyin_index,size,mtime,file_type,duration_secs)
                     VALUES(1,?1,?2,?2,'old large payload unused',1,1,'audio',1)",
                    params![format!("/samples/{index:05}.wav"), filename],
                ).unwrap();
            }
            conn.execute_batch("COMMIT;").unwrap();
            for query in [
                "missing_all_abcdefgh",
                "chongqingnihaoshijiekickdrum",
                "chongqingnihaosh",
                "kick drum",
            ] {
                *SEARCH_NAME_CACHE
                    .get_or_init(Default::default)
                    .lock()
                    .unwrap() = SearchNameCache::default();
                for pass in 0..2 {
                    let start = Instant::now();
                    let page =
                        search_file_page(&conn, "library", query, "all", "filename", "asc", 0)
                            .unwrap();
                    println!("BENCH count={count} query={query:?} pass={pass} elapsed_ms={:.2} records={}",
                        start.elapsed().as_secs_f64()*1000.0,page.records.len());
                }
            }
        }
    }
}
