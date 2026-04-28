// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::config;
use crate::cover_picture::CoverPicture;
use crate::file_item::FileItem;
use crate::grid_cover::{GridCover, GRID_TILE_SIZE};
use crate::library_db::{self, LibraryDB, COVER_NAMES};
use crate::player::AudioPlayer;

const PLAYER_WIDTH: i32 = 450;
const THUMB_SIZE: i32 = 44;

const REPEAT_CONSECUTIVE: u32 = 0;
const REPEAT_ONCE: u32 = 1;
const REPEAT_LOOP: u32 = 2;
static REPEAT_ICONS: &[&str] = &[
    "fp-playlist-consecutive-symbolic",
    "fp-playlist-repeat-song-symbolic",
    "fp-playlist-repeat-symbolic",
];
static LOSSLESS_FORMATS: &[&str] = &["FLAC", "WAV", "AIFF", "AIF", "APE", "WV"];

/// Schedule a closure on the default main context from a background thread.
/// Unlike `idle_add_local_once`, this does **not** panic when the calling
/// thread is not the main thread.
///
/// # Safety
/// The captured values must only be accessed from one thread at a time.
/// In our case the spawning thread creates them, moves them into the
/// closure, and never touches them again — the main thread runs the
/// closure exclusively.  GObject ref-counting is atomic.
unsafe fn idle_add_once_raw<F: FnOnce() + 'static>(f: F) {
    struct SendFn<F>(F);
    unsafe impl<F> Send for SendFn<F> {}
    impl<F: FnOnce()> SendFn<F> {
        fn run(self) { (self.0)() }
    }
    let wrapped = SendFn(f);
    glib::idle_add_once(move || wrapped.run());
}

// ── Display-name helpers ───────────────────────────────────────────
fn display_path(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && path.starts_with(&home) {
        return format!("~{}", &path[home.len()..]);
    }
    path.to_string()
}

/// Return the XDG Music directory (e.g. /home/user/Music).
fn xdg_music_dir() -> Option<String> {
    glib::user_special_dir(glib::enums::UserDirectory::Music)
        .map(|p| p.to_string_lossy().to_string())
}

/// List mount-points directly under /mnt that are readable directories.
fn list_mnt_entries() -> Vec<String> {
    let mnt = Path::new("/mnt");
    if !mnt.is_dir() { return vec![]; }
    let mut entries: Vec<String> = std::fs::read_dir(mnt)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.ok()?;
            let p = e.path();
            if p.is_dir() { Some(p.to_string_lossy().to_string()) } else { None }
        })
        .collect();
    entries.sort();
    entries
}

fn is_bad_bunny(artist: &str) -> bool {
    !artist.is_empty() && artist.to_lowercase().contains("bad bunny")
}

fn format_time(secs: f64) -> String {
    let m = secs as u64 / 60;
    let s = secs as u64 % 60;
    format!("{m}:{s:02}")
}

// ────────────────────────────────────────────────────────────────────
// ObjectSubclass
// ────────────────────────────────────────────────────────────────────

#[derive(gtk::CompositeTemplate, Default)]
#[template(resource = "/io/github/juancarlosbernal/FolderPlay/window.ui")]
pub struct FolderplayWindow {
    #[template_child]
    pub main_box: TemplateChild<gtk::Box>,

    // Constructed at runtime
    pub player: RefCell<Option<AudioPlayer>>,
    pub db: RefCell<Option<Arc<LibraryDB>>>,
    pub shutdown: Arc<AtomicBool>,

    // State
    pub playlist: RefCell<Vec<String>>,
    pub current_index: Cell<i32>,
    pub cover_texture: RefCell<Option<gdk::Texture>>,
    pub last_seek_time: Cell<i64>,
    pub nav_stack: RefCell<Vec<(Option<String>, String)>>,
    pub current_folder: RefCell<Option<String>>,
    pub root_folder: RefCell<Option<String>>,

    #[allow(clippy::type_complexity)]
    pub bound_rows: RefCell<HashMap<String, Vec<(gtk::Box, gtk::Image, gtk::Image)>>>,
    pub list_store: RefCell<Option<gio::ListStore>>,
    pub grid_store: RefCell<Option<gio::ListStore>>,
    pub current_items: RefCell<Vec<FileItem>>,
    pub repeat_mode: Cell<u32>,
    pub browse_visible: Cell<bool>,
    pub browse_manual_closed: Cell<bool>,
    pub auto_hide_busy: Cell<bool>,
    pub current_tags: RefCell<HashMap<String, String>>,
    pub pending_scroll_path: RefCell<Option<String>>,
    pub playing_path: RefCell<Option<String>>,
    pub external_file: Cell<bool>,
    pub filter_model: RefCell<Option<gtk::FilterListModel>>,

    // Widget refs (set in build_ui)
    pub browse_revealer: RefCell<Option<gtk::Revealer>>,
    pub search_btn: RefCell<Option<gtk::ToggleButton>>,
    pub search_entry: RefCell<Option<gtk::SearchEntry>>,
    pub search_revealer: RefCell<Option<gtk::Revealer>>,
    pub home_btn: RefCell<Option<gtk::Button>>,
    pub back_btn: RefCell<Option<gtk::Button>>,
    pub folder_label: RefCell<Option<gtk::Label>>,
    pub song_count_label: RefCell<Option<gtk::Label>>,
    pub scan_count_label: RefCell<Option<gtk::Label>>,
    pub grid_toggle: RefCell<Option<gtk::ToggleButton>>,
    pub browse_stack: RefCell<Option<gtk::Stack>>,
    pub list_view: RefCell<Option<gtk::ListView>>,
    pub grid_view: RefCell<Option<gtk::GridView>>,
    pub sep: RefCell<Option<gtk::Separator>>,
    pub player_panel: RefCell<Option<gtk::Box>>,
    pub dock_btn: RefCell<Option<gtk::Button>>,
    pub locate_btn: RefCell<Option<gtk::Button>>,
    pub hires_icon_player: RefCell<Option<gtk::Picture>>,
    pub tag_btn: RefCell<Option<gtk::Button>>,
    pub menu_btn: RefCell<Option<gtk::MenuButton>>,
    pub cover_stack: RefCell<Option<gtk::Stack>>,
    pub cover_picture: RefCell<Option<CoverPicture>>,
    pub title_label: RefCell<Option<gtk::Label>>,
    pub subtitle_label: RefCell<Option<gtk::Label>>,
    pub format_label: RefCell<Option<gtk::Label>>,
    pub seek_scale: RefCell<Option<gtk::Scale>>,
    pub pos_label: RefCell<Option<gtk::Label>>,
    pub dur_label: RefCell<Option<gtk::Label>>,
    pub play_btn: RefCell<Option<gtk::Button>>,
    pub prev_btn: RefCell<Option<gtk::Button>>,
    pub next_btn: RefCell<Option<gtk::Button>>,
    pub repeat_btn: RefCell<Option<gtk::Button>>,
    pub vol_scale: RefCell<Option<gtk::Scale>>,
    pub vol_btn: RefCell<Option<gtk::MenuButton>>,
    pub dynamic_css: RefCell<Option<gtk::CssProvider>>,
}

#[glib::object_subclass]
impl ObjectSubclass for FolderplayWindow {
    const NAME: &'static str = "FolderplayWindow";
    type Type = super::FolderplayWindow;
    type ParentType = adw::ApplicationWindow;

    fn class_init(klass: &mut Self::Class) {
        klass.bind_template();
    }

    fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
        obj.init_template();
    }
}

impl ObjectImpl for FolderplayWindow {
    fn dispose(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(p) = self.player.borrow().as_ref() {
            p.cleanup();
        }
    }

    fn constructed(&self) {
        self.parent_constructed();

        self.browse_visible.set(true);
        self.current_index.set(-1);

        // Init player + db
        let player = AudioPlayer::new();
        let db = Arc::new(LibraryDB::new(None));
        *self.player.borrow_mut() = Some(player);
        *self.db.borrow_mut() = Some(db);

        let ls = gio::ListStore::new::<FileItem>();
        *self.list_store.borrow_mut() = Some(ls);
        let gs = gio::ListStore::new::<FileItem>();
        *self.grid_store.borrow_mut() = Some(gs);

        self.apply_audio_output();
        self.setup_dynamic_css();
        self.build_ui();
        self.setup_actions();
        self.connect_signals();

        let settings = gio::Settings::new(config::APP_ID);
        let mut folders: Vec<String> = settings
            .strv("music-folders")
            .iter()
            .map(|s| s.to_string())
            .filter(|f| !f.is_empty())
            .collect();

        // Migrate legacy
        let legacy = settings.string("music-folder").to_string();
        if !legacy.is_empty() && Path::new(&legacy).is_dir() {
            if !folders.contains(&legacy) {
                folders.insert(0, legacy.clone());
                let v: Vec<&str> = folders.iter().map(|s| s.as_str()).collect();
                settings.set_strv("music-folders", v).ok();
            }
            settings.set_string("music-folder", "").ok();
        }

        // Auto-add XDG Music if enabled in settings
        if settings.boolean("xdg-music-enabled") {
            if let Some(music_dir) = xdg_music_dir() {
                if Path::new(&music_dir).is_dir() && !folders.contains(&music_dir) {
                    folders.push(music_dir.clone());
                    let v: Vec<&str> = folders.iter().map(|s| s.as_str()).collect();
                    settings.set_strv("music-folders", v).ok();
                }
            }
        }

        // Remove folders that no longer exist on disk
        let accessible: Vec<String> = folders.into_iter()
            .filter(|f| Path::new(f).is_dir())
            .collect();

        if !accessible.is_empty() {
            self.load_all_folders();
        }
    }
}

impl WidgetImpl for FolderplayWindow {}
impl WindowImpl for FolderplayWindow {}
impl ApplicationWindowImpl for FolderplayWindow {}
impl AdwApplicationWindowImpl for FolderplayWindow {}

// ────────────────────────────────────────────────────────────────────
// Implementation methods
// ────────────────────────────────────────────────────────────────────
impl FolderplayWindow {
    // ── CSS ────────────────────────────────────────────────────────
    fn setup_dynamic_css(&self) {
        let Some(display) = gdk::Display::default() else { return };
        let css = gtk::CssProvider::new();
        css.load_from_resource("/io/github/juancarlosbernal/FolderPlay/style.css");
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let dynamic = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &display,
            &dynamic,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        *self.dynamic_css.borrow_mut() = Some(dynamic);
    }

    fn apply_audio_output(&self) {
        let settings = gio::Settings::new(config::APP_ID);
        let output = settings.string("audio-output");
        if let Some(p) = self.player.borrow().as_ref() {
            p.set_audio_output(&output);
        }
    }

    // ── Build entire UI ────────────────────────────────────────────
    fn build_ui(&self) {
        let content = self.main_box.get();
        content.add_css_class("album-bg");

        // ── Browse panel ───────────────────────────────────────────
        let browse_revealer = gtk::Revealer::builder()
            .reveal_child(true)
            .transition_type(gtk::RevealerTransitionType::SlideRight)
            .transition_duration(250)
            .hexpand(true)
            .build();

        let browse = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        browse.add_css_class("browse-panel");
        browse_revealer.set_child(Some(&browse));
        content.append(&browse_revealer);

        // Browse toolbar
        let browse_handle = gtk::WindowHandle::new();
        let browse_bar = gtk::Box::builder()
            .spacing(4)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .margin_bottom(2)
            .build();

        let search_btn = gtk::ToggleButton::builder()
            .icon_name("fp-edit-find-symbolic")
            .tooltip_text(gettext("Search"))
            .build();
        search_btn.add_css_class("flat");
        search_btn.add_css_class("circular");
        browse_bar.append(&search_btn);

        let home_btn = gtk::Button::builder()
            .icon_name("fp-folder-open-symbolic")
            .tooltip_text(gettext("Go to root folder"))
            .build();
        home_btn.add_css_class("flat");
        home_btn.add_css_class("circular");
        browse_bar.append(&home_btn);

        browse_bar.append(&gtk::Box::builder().hexpand(true).build());

        let song_count_label = gtk::Label::builder().label("").margin_end(6).build();
        song_count_label.add_css_class("dim-label");
        song_count_label.add_css_class("caption");
        browse_bar.append(&song_count_label);

        let grid_toggle = gtk::ToggleButton::builder()
            .icon_name("fp-grid-filled-symbolic")
            .tooltip_text(gettext("Grid view"))
            .active(true)
            .build();
        grid_toggle.add_css_class("flat");
        grid_toggle.add_css_class("circular");
        browse_bar.append(&grid_toggle);

        browse_handle.set_child(Some(&browse_bar));
        browse.append(&browse_handle);

        // Search revealer
        let search_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .build();
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(gettext("Search songs, artist or album…"))
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        search_revealer.set_child(Some(&search_entry));
        browse.append(&search_revealer);

        search_btn
            .bind_property("active", &search_revealer, "reveal-child")
            .sync_create()
            .build();

        // Navigation bar
        let nav = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(4)
            .build();
        let back_btn = gtk::Button::builder()
            .icon_name("fp-arrow3-left-symbolic")
            .visible(false)
            .build();
        back_btn.add_css_class("flat");
        back_btn.add_css_class("circular");
        nav.append(&back_btn);

        let folder_label = gtk::Label::builder()
            .label(gettext("Folders"))
            .hexpand(true)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        folder_label.add_css_class("title-4");
        nav.append(&folder_label);
        browse.append(&nav);

        // Browse stack
        let browse_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .vexpand(true)
            .build();

        // Empty state
        let empty = adw::StatusPage::builder()
            .icon_name("fp-folder-music-symbolic")
            .title(gettext("No Music Folder"))
            .description(gettext("Select a folder to start playing"))
            .build();
        let add_btn = gtk::Button::builder()
            .label(gettext("Add Music Folder"))
            .halign(gtk::Align::Center)
            .build();
        add_btn.add_css_class("pill");
        add_btn.add_css_class("suggested-action");
        empty.set_child(Some(&add_btn));
        browse_stack.add_named(&empty, Some("empty"));

        // Search empty
        let search_empty = adw::StatusPage::builder()
            .icon_name("fp-edit-find-symbolic")
            .title(gettext("No results found"))
            .description(gettext("Try another title, artist, or album"))
            .build();
        browse_stack.add_named(&search_empty, Some("search-empty"));

        // Loading
        let spinner_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .spacing(12)
            .build();
        let spinner = adw::Spinner::builder().build();
        spinner.set_size_request(32, 32);
        spinner_box.append(&spinner);
        let scan_count_lbl = gtk::Label::builder()
            .label("")
            .halign(gtk::Align::Center)
            .build();
        scan_count_lbl.add_css_class("dim-label");
        scan_count_lbl.add_css_class("caption");
        spinner_box.append(&scan_count_lbl);
        *self.scan_count_label.borrow_mut() = Some(scan_count_lbl);
        browse_stack.add_named(&spinner_box, Some("loading"));

        // List view
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        let list_view = gtk::ListView::builder()
            .single_click_activate(true)
            .build();
        list_view.add_css_class("browse-list");
        scroll.set_child(Some(&list_view));
        browse_stack.add_named(&scroll, Some("list"));

        // Grid view
        let grid_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        let grid_factory = gtk::SignalListItemFactory::new();
        let gs = self.grid_store.borrow().as_ref().unwrap().clone();
        let grid_sel = gtk::SingleSelection::builder()
            .model(&gs)
            .autoselect(false)
            .build();
        let grid_view = gtk::GridView::builder()
            .model(&grid_sel)
            .factory(&grid_factory)
            .single_click_activate(true)
            .max_columns(50)
            .min_columns(1)
            .build();
        grid_view.add_css_class("browse-grid");
        grid_scroll.set_child(Some(&grid_view));
        browse_stack.add_named(&grid_scroll, Some("grid"));

        browse.append(&browse_stack);

        // Separator
        let sep = gtk::Separator::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        sep.add_css_class("thin-separator");
        content.append(&sep);

        // ── Player panel ───────────────────────────────────────────
        let player_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .hexpand(false)
            .build();
        player_panel.set_size_request(PLAYER_WIDTH, -1);
        player_panel.add_css_class("player-panel");

        // Player top bar
        let player_handle = gtk::WindowHandle::new();
        let player_top = gtk::Box::builder()
            .spacing(4)
            .margin_start(8)
            .margin_end(8)
            .margin_top(6)
            .build();

        let dock_btn = gtk::Button::builder()
            .icon_name("fp-sidebar-show-symbolic")
            .tooltip_text(gettext("Toggle Browse Panel"))
            .valign(gtk::Align::Center)
            .build();
        dock_btn.add_css_class("flat");
        dock_btn.add_css_class("circular");
        player_top.append(&dock_btn);

        let locate_btn = gtk::Button::builder()
            .icon_name("fp-playlist-symbolic")
            .tooltip_text(gettext("Go to Playing Folder"))
            .visible(false)
            .valign(gtk::Align::Center)
            .build();
        locate_btn.add_css_class("flat");
        locate_btn.add_css_class("circular");
        player_top.append(&locate_btn);

        player_top.append(&gtk::Box::builder().hexpand(true).build());

        let hires_icon_player = gtk::Picture::for_resource(
            "/io/github/juancarlosbernal/FolderPlay/icons/scalable/actions/hires-22.png",
        );
        hires_icon_player.set_size_request(22, 22);
        hires_icon_player.set_can_shrink(false);
        hires_icon_player.set_halign(gtk::Align::Center);
        hires_icon_player.set_valign(gtk::Align::Center);
        hires_icon_player.set_visible(false);
        player_top.append(&hires_icon_player);

        let tag_btn = gtk::Button::builder()
            .icon_name("fp-tag-outline-symbolic")
            .tooltip_text(gettext("Song Info"))
            .visible(false)
            .valign(gtk::Align::Center)
            .build();
        tag_btn.add_css_class("flat");
        tag_btn.add_css_class("circular");
        player_top.append(&tag_btn);

        let menu_btn = gtk::MenuButton::builder()
            .icon_name("fp-open-menu-symbolic")
            .tooltip_text(gettext("Menu"))
            .valign(gtk::Align::Center)
            .build();
        menu_btn.add_css_class("flat");
        menu_btn.add_css_class("circular");
        self.build_app_menu(&menu_btn);
        player_top.append(&menu_btn);

        let win_minimize = gtk::Button::builder()
            .icon_name("fp-window-minimize-symbolic")
            .tooltip_text(gettext("Minimize"))
            .valign(gtk::Align::Center)
            .build();
        win_minimize.add_css_class("circular");
        win_minimize.add_css_class("windowcontrol-btn");
        player_top.append(&win_minimize);

        let win_close = gtk::Button::builder()
            .icon_name("fp-window-close-symbolic")
            .tooltip_text(gettext("Close"))
            .valign(gtk::Align::Center)
            .build();
        win_close.add_css_class("circular");
        win_close.add_css_class("windowcontrol-btn");
        player_top.append(&win_close);

        player_handle.set_child(Some(&player_top));
        player_panel.append(&player_handle);

        // Cover art
        let cover_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(300)
            .build();

        let ph = gtk::Box::builder()
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .build();
        ph.set_size_request(300, 300);
        ph.add_css_class("cover-placeholder");
        let ph_icon = gtk::Image::builder()
            .icon_name("fp-folder-music-symbolic")
            .pixel_size(64)
            .hexpand(true)
            .vexpand(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        ph.append(&ph_icon);
        cover_stack.add_named(&ph, Some("placeholder"));

        let cover_picture = CoverPicture::new();
        cover_stack.add_named(&cover_picture, Some("art"));

        let wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        wrap.set_overflow(gtk::Overflow::Hidden);
        wrap.add_css_class("cover-container");
        wrap.set_halign(gtk::Align::Center);
        wrap.set_valign(gtk::Align::Center);
        wrap.append(&cover_stack);

        let cover_box = gtk::Box::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .vexpand(true)
            .margin_top(12)
            .build();
        cover_box.append(&wrap);
        player_panel.append(&cover_box);

        // Song info
        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .halign(gtk::Align::Center)
            .build();
        let title_label = gtk::Label::builder()
            .label("FolderPlay")
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(30)
            .justify(gtk::Justification::Center)
            .build();
        title_label.add_css_class("title-3");
        info.append(&title_label);

        let subtitle_label = gtk::Label::builder()
            .label(gettext("Select a song"))
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(35)
            .justify(gtk::Justification::Center)
            .build();
        subtitle_label.add_css_class("dim-label");
        info.append(&subtitle_label);

        let format_label = gtk::Label::builder()
            .label("")
            .visible(false)
            .justify(gtk::Justification::Center)
            .build();
        format_label.add_css_class("caption");
        format_label.add_css_class("dim-label");
        info.append(&format_label);
        player_panel.append(&info);

        // Seek bar
        let seek_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .margin_start(24)
            .margin_end(24)
            .build();
        let seek_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        seek_scale.set_draw_value(false);
        seek_scale.add_css_class("seek-slider");
        seek_box.append(&seek_scale);

        let times = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let pos_label = gtk::Label::builder().label("0:00").build();
        pos_label.add_css_class("caption");
        pos_label.add_css_class("dim-label");
        let dur_label = gtk::Label::builder().label("0:00").build();
        dur_label.add_css_class("caption");
        dur_label.add_css_class("dim-label");
        times.append(&pos_label);
        times.append(&gtk::Box::builder().hexpand(true).build());
        times.append(&dur_label);
        seek_box.append(&times);
        player_panel.append(&seek_box);

        // Transport
        let transport = gtk::Box::builder()
            .spacing(16)
            .halign(gtk::Align::Center)
            .margin_top(4)
            .margin_bottom(24)
            .build();

        let repeat_btn = gtk::Button::builder()
            .icon_name(REPEAT_ICONS[REPEAT_CONSECUTIVE as usize])
            .tooltip_text(gettext("Consecutive"))
            .valign(gtk::Align::Center)
            .build();
        repeat_btn.add_css_class("circular");
        repeat_btn.add_css_class("flat");
        repeat_btn.add_css_class("transport-btn");
        transport.append(&repeat_btn);

        let prev_btn = gtk::Button::builder()
            .icon_name("fp-media-skip-backward-symbolic")
            .valign(gtk::Align::Center)
            .build();
        prev_btn.add_css_class("circular");
        prev_btn.add_css_class("flat");
        prev_btn.add_css_class("transport-btn");

        let play_btn = gtk::Button::builder()
            .icon_name("fp-media-playback-start-symbolic")
            .valign(gtk::Align::Center)
            .build();
        play_btn.add_css_class("circular");
        play_btn.add_css_class("play-button");

        let next_btn = gtk::Button::builder()
            .icon_name("fp-media-skip-forward-symbolic")
            .valign(gtk::Align::Center)
            .build();
        next_btn.add_css_class("circular");
        next_btn.add_css_class("flat");
        next_btn.add_css_class("transport-btn");

        let vol_btn = gtk::MenuButton::builder()
            .icon_name("fp-speaker-2-symbolic")
            .tooltip_text(gettext("Volume"))
            .valign(gtk::Align::Center)
            .build();
        vol_btn.add_css_class("circular");
        vol_btn.add_css_class("flat");
        vol_btn.add_css_class("transport-btn");

        let vol_popover = gtk::Popover::new();
        vol_popover.add_css_class("volume-popover");
        let vol_scale = gtk::Scale::with_range(gtk::Orientation::Vertical, 0.0, 1.0, 0.05);
        vol_scale.set_inverted(true);
        vol_scale.set_value(0.7);
        vol_scale.set_draw_value(false);
        vol_scale.set_size_request(-1, 150);
        vol_popover.set_child(Some(&vol_scale));
        vol_btn.set_popover(Some(&vol_popover));

        transport.append(&prev_btn);
        transport.append(&play_btn);
        transport.append(&next_btn);
        transport.append(&vol_btn);
        player_panel.append(&transport);

        content.append(&player_panel);

        // Setup list/grid factories
        self.setup_list_view(&list_view);
        self.setup_grid_factory(&grid_factory, &grid_view);

        // Store refs
        *self.browse_revealer.borrow_mut() = Some(browse_revealer);
        *self.search_btn.borrow_mut() = Some(search_btn);
        *self.search_entry.borrow_mut() = Some(search_entry);
        *self.search_revealer.borrow_mut() = Some(search_revealer);
        *self.home_btn.borrow_mut() = Some(home_btn);
        *self.back_btn.borrow_mut() = Some(back_btn);
        *self.folder_label.borrow_mut() = Some(folder_label);
        *self.song_count_label.borrow_mut() = Some(song_count_label);
        *self.grid_toggle.borrow_mut() = Some(grid_toggle);
        *self.browse_stack.borrow_mut() = Some(browse_stack);
        *self.list_view.borrow_mut() = Some(list_view);
        *self.grid_view.borrow_mut() = Some(grid_view);
        *self.sep.borrow_mut() = Some(sep);
        *self.player_panel.borrow_mut() = Some(player_panel);
        *self.dock_btn.borrow_mut() = Some(dock_btn);
        *self.locate_btn.borrow_mut() = Some(locate_btn);
        *self.hires_icon_player.borrow_mut() = Some(hires_icon_player);
        *self.tag_btn.borrow_mut() = Some(tag_btn);
        *self.menu_btn.borrow_mut() = Some(menu_btn);
        *self.cover_stack.borrow_mut() = Some(cover_stack);
        *self.cover_picture.borrow_mut() = Some(cover_picture);
        *self.title_label.borrow_mut() = Some(title_label);
        *self.subtitle_label.borrow_mut() = Some(subtitle_label);
        *self.format_label.borrow_mut() = Some(format_label);
        *self.seek_scale.borrow_mut() = Some(seek_scale);
        *self.pos_label.borrow_mut() = Some(pos_label);
        *self.dur_label.borrow_mut() = Some(dur_label);
        *self.play_btn.borrow_mut() = Some(play_btn);
        *self.prev_btn.borrow_mut() = Some(prev_btn);
        *self.next_btn.borrow_mut() = Some(next_btn);
        *self.repeat_btn.borrow_mut() = Some(repeat_btn);
        *self.vol_scale.borrow_mut() = Some(vol_scale);
        *self.vol_btn.borrow_mut() = Some(vol_btn);

        // Connect add-folder button from empty state
        let obj_weak = self.obj().downgrade();
        add_btn.connect_clicked(move |_| {
            if let Some(obj) = obj_weak.upgrade() {
                obj.imp().on_open_folder();
            }
        });

        // Connect minimize/close
        let obj_weak = self.obj().downgrade();
        win_minimize.connect_clicked(move |_| {
            if let Some(obj) = obj_weak.upgrade() {
                obj.minimize();
            }
        });
        let obj_weak = self.obj().downgrade();
        win_close.connect_clicked(move |_| {
            if let Some(obj) = obj_weak.upgrade() {
                obj.close();
            }
        });
    }

    fn build_app_menu(&self, menu_btn: &gtk::MenuButton) {
        let menu = gio::Menu::new();

        let folder_section = gio::Menu::new();
        folder_section.append(Some(&gettext("Manage Folders…")), Some("win.manage-folders"));
        menu.append_section(None, &folder_section);

        let bunny_section = gio::Menu::new();
        bunny_section.append(Some(&gettext("Anti Bad Bunny")), Some("win.anti-bad-bunny"));
        menu.append_section(None, &bunny_section);

        let app_section = gio::Menu::new();
        app_section.append(Some(&gettext("Preferences")), Some("app.preferences"));
        app_section.append(Some(&gettext("Keyboard Shortcuts")), Some("app.shortcuts"));
        app_section.append(Some(&gettext("About FolderPlay")), Some("app.about"));
        menu.append_section(None, &app_section);

        menu_btn.set_menu_model(Some(&menu));
    }

    // ── List view factory ──────────────────────────────────────────
    fn setup_list_view(&self, list_view: &gtk::ListView) {
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(|_, list_item| {
            let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::builder()
                .spacing(10)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .build();

            let thumb_stack = gtk::Stack::new();
            thumb_stack.set_size_request(THUMB_SIZE, THUMB_SIZE);
            thumb_stack.set_halign(gtk::Align::Center);
            thumb_stack.set_valign(gtk::Align::Center);
            let icon = gtk::Image::builder().pixel_size(32).build();
            thumb_stack.add_named(&icon, Some("icon"));
            let thumb = gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Cover)
                .can_shrink(true)
                .build();
            thumb.set_size_request(THUMB_SIZE, THUMB_SIZE);
            thumb.add_css_class("song-thumb");
            thumb_stack.add_named(&thumb, Some("thumb"));
            row.append(&thumb_stack);

            let info_col = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .hexpand(true)
                .spacing(2)
                .build();
            let title_lbl = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .build();
            title_lbl.add_css_class("song-title");
            info_col.append(&title_lbl);
            let artist_lbl = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(pango::EllipsizeMode::End)
                .build();
            artist_lbl.add_css_class("dim-label");
            artist_lbl.add_css_class("caption");
            info_col.append(&artist_lbl);
            row.append(&info_col);

            let playing_icon = gtk::Image::builder()
                .icon_name("fp-media-playback-start-symbolic")
                .pixel_size(18)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            playing_icon.add_css_class("now-playing-icon");
            row.append(&playing_icon);

            let repeat_icon = gtk::Image::builder()
                .pixel_size(14)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .visible(false)
                .build();
            repeat_icon.add_css_class("now-playing-icon");
            row.append(&repeat_icon);

            let hires_box = gtk::Picture::for_resource(
                "/io/github/juancarlosbernal/FolderPlay/icons/scalable/actions/hires-22.png",
            );
            hires_box.set_size_request(22, 22);
            hires_box.set_can_shrink(false);
            hires_box.set_halign(gtk::Align::Center);
            hires_box.set_valign(gtk::Align::Center);
            hires_box.set_visible(false);
            row.append(&hires_box);

            let meta_col = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .spacing(2)
                .build();
            let fmt_lbl = gtk::Label::new(None);
            fmt_lbl.add_css_class("caption");
            fmt_lbl.add_css_class("format-badge");
            meta_col.append(&fmt_lbl);
            let quality_lbl = gtk::Label::new(None);
            quality_lbl.add_css_class("caption");
            quality_lbl.add_css_class("dim-label");
            meta_col.append(&quality_lbl);
            row.append(&meta_col);

            let dur_lbl = gtk::Label::builder()
                .halign(gtk::Align::End)
                .valign(gtk::Align::Center)
                .build();
            dur_lbl.add_css_class("caption");
            dur_lbl.add_css_class("dim-label");
            row.append(&dur_lbl);

            let arrow = gtk::Image::builder()
                .icon_name("fp-arrow3-right-symbolic")
                .opacity(0.4)
                .valign(gtk::Align::Center)
                .build();
            row.append(&arrow);

            li.set_child(Some(&row));
        });

        let obj_weak = self.obj().downgrade();
        factory.connect_bind(move |_, list_item| {
            let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let item = li.item().and_downcast::<FileItem>().unwrap();
            let row = li.child().and_downcast::<gtk::Box>().unwrap();

            // Navigate children directly via first_child/next_sibling (O(1) per widget)
            let thumb_stack: gtk::Stack = row.first_child().unwrap().downcast().unwrap();
            let info_col: gtk::Box = thumb_stack.next_sibling().unwrap().downcast().unwrap();
            let playing_icon: gtk::Image = info_col.next_sibling().unwrap().downcast().unwrap();
            let repeat_icon: gtk::Image = playing_icon.next_sibling().unwrap().downcast().unwrap();
            let hires_box: gtk::Picture = repeat_icon.next_sibling().unwrap().downcast().unwrap();
            let meta_col: gtk::Box = hires_box.next_sibling().unwrap().downcast().unwrap();
            let dur_lbl: gtk::Label = meta_col.next_sibling().unwrap().downcast().unwrap();
            let arrow: gtk::Image = dur_lbl.next_sibling().unwrap().downcast().unwrap();

            let title_lbl: gtk::Label = info_col.first_child().unwrap().downcast().unwrap();
            let artist_lbl: gtk::Label = title_lbl.next_sibling().unwrap().downcast().unwrap();

            let fmt_lbl: gtk::Label = meta_col.first_child().unwrap().downcast().unwrap();
            let quality_lbl: gtk::Label = fmt_lbl.next_sibling().unwrap().downcast().unwrap();

            if item.is_folder() {
                if let Some(tex) = item.cover_thumb() {
                    let thumb = thumb_stack.child_by_name("thumb").and_downcast::<gtk::Picture>().unwrap();
                    thumb.set_paintable(Some(&tex));
                    thumb_stack.set_visible_child_name("thumb");
                } else {
                    let icon = thumb_stack.child_by_name("icon").and_downcast::<gtk::Image>().unwrap();
                    icon.set_icon_name(Some("fp-folder-open-symbolic"));
                    thumb_stack.set_visible_child_name("icon");
                }
                title_lbl.set_label(&item.name());
                artist_lbl.set_visible(false);
                playing_icon.set_visible(false);
                repeat_icon.set_visible(false);
                hires_box.set_visible(false);
                meta_col.set_visible(false);
                dur_lbl.set_visible(false);
                arrow.set_visible(true);
                row.remove_css_class("now-playing-row");
            } else {
                arrow.set_visible(false);
                if let Some(tex) = item.cover_thumb() {
                    let thumb = thumb_stack.child_by_name("thumb").and_downcast::<gtk::Picture>().unwrap();
                    thumb.set_paintable(Some(&tex));
                    thumb_stack.set_visible_child_name("thumb");
                } else {
                    let icon = thumb_stack.child_by_name("icon").and_downcast::<gtk::Image>().unwrap();
                    icon.set_icon_name(Some("fp-folder-music-symbolic"));
                    thumb_stack.set_visible_child_name("icon");
                }

                // Now-playing indicator
                let (is_np, rmode) = if let Some(obj) = obj_weak.upgrade() {
                    let imp = obj.imp();
                    let pl = imp.playlist.borrow();
                    let idx = imp.current_index.get();
                    let np = idx >= 0 && (idx as usize) < pl.len() && pl[idx as usize] == item.path();
                    (np, imp.repeat_mode.get())
                } else {
                    (false, REPEAT_CONSECUTIVE)
                };
                playing_icon.set_visible(is_np);
                if is_np && rmode != REPEAT_CONSECUTIVE {
                    repeat_icon.set_icon_name(Some(REPEAT_ICONS[rmode as usize]));
                    repeat_icon.set_visible(true);
                } else {
                    repeat_icon.set_visible(false);
                }
                if is_np { row.add_css_class("now-playing-row"); } else { row.remove_css_class("now-playing-row"); }

                let title = item.title();
                let display_title = if title.is_empty() {
                    Path::new(&item.name()).file_stem().unwrap_or_default().to_string_lossy().to_string()
                } else {
                    title
                };
                title_lbl.set_label(&display_title);

                let artist = item.artist();
                if artist.is_empty() {
                    artist_lbl.set_visible(false);
                } else {
                    artist_lbl.set_label(&artist);
                    artist_lbl.set_visible(true);
                }

                meta_col.set_visible(true);
                fmt_lbl.set_label(&item.format_type());
                hires_box.set_visible(item.sample_rate() >= 48000);

                let ft = item.format_type();
                let sr = item.sample_rate();
                if (ft == "DSF" || ft == "DFF") && sr > 0 {
                    let dsd_level = (sr as f64 * 8.0 / 44100.0).round() as i32;
                    quality_lbl.set_label(&format!("DSD{dsd_level}"));
                } else if LOSSLESS_FORMATS.contains(&ft.as_str()) && sr > 0 {
                    let bits = if item.bits_per_sample() > 0 { item.bits_per_sample() } else { 16 };
                    let sr_k = sr as f64 / 1000.0;
                    quality_lbl.set_label(&format!("{bits}-bit/{sr_k}kHz"));
                } else if item.bitrate() > 0 {
                    quality_lbl.set_label(&format!("{} kbps", item.bitrate()));
                } else {
                    quality_lbl.set_label("");
                }

                dur_lbl.set_visible(true);
                if item.duration() > 0.0 {
                    dur_lbl.set_label(&format_time(item.duration()));
                } else {
                    dur_lbl.set_label("\u{2014}");
                }
            }

            // Track bound rows
            if let Some(obj) = obj_weak.upgrade() {
                obj.imp().bound_rows.borrow_mut()
                    .entry(item.path())
                    .or_default()
                    .push((row.clone(), playing_icon, repeat_icon));
            }
        });

        let obj_weak = self.obj().downgrade();
        factory.connect_unbind(move |_, list_item| {
            let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            if let (Some(item), Some(obj)) = (li.item().and_downcast::<FileItem>(), obj_weak.upgrade()) {
                let path = item.path();
                let row = li.child().and_downcast::<gtk::Box>();
                let mut bound = obj.imp().bound_rows.borrow_mut();
                if let Some(entries) = bound.get_mut(&path) {
                    entries.retain(|(r, _, _)| row.as_ref() != Some(r));
                    if entries.is_empty() {
                        bound.remove(&path);
                    }
                }
            }
        });

        let ls = self.list_store.borrow().as_ref().unwrap().clone();
        let filter_model = gtk::FilterListModel::new(Some(ls), None::<gtk::Filter>);
        let selection = gtk::SingleSelection::builder()
            .model(&filter_model)
            .autoselect(false)
            .build();
        list_view.set_factory(Some(&factory));
        list_view.set_model(Some(&selection));

        *self.filter_model.borrow_mut() = Some(filter_model);

        let obj_weak = self.obj().downgrade();
        list_view.connect_activate(move |_, position| {
            if let Some(obj) = obj_weak.upgrade() {
                obj.imp().on_list_activated(position);
            }
        });
    }

    fn setup_grid_factory(&self, factory: &gtk::SignalListItemFactory, grid_view: &gtk::GridView) {
        factory.connect_setup(|_, list_item| {
            let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let outer = gtk::Box::builder()
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .margin_start(10)
                .margin_end(10)
                .margin_top(10)
                .margin_bottom(10)
                .build();
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Start)
                .build();
            card.set_size_request(GRID_TILE_SIZE, -1);
            card.add_css_class("grid-card");

            let cover_stack = gtk::Stack::new();
            cover_stack.set_halign(gtk::Align::Center);
            cover_stack.add_css_class("grid-cover-box");
            let cover = GridCover::new();
            cover.add_css_class("grid-cover-box");
            cover_stack.add_named(&cover, Some("cover"));
            let icon = gtk::Image::builder()
                .icon_name("fp-folder-music-symbolic")
                .pixel_size(64)
                .valign(gtk::Align::Center)
                .halign(gtk::Align::Center)
                .build();
            icon.set_size_request(GRID_TILE_SIZE, GRID_TILE_SIZE);
            icon.add_css_class("grid-folder-icon");
            cover_stack.add_named(&icon, Some("icon"));
            card.append(&cover_stack);

            let lbl = gtk::Label::builder()
                .ellipsize(pango::EllipsizeMode::End)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .lines(2)
                .max_width_chars(1)
                .xalign(0.5)
                .halign(gtk::Align::Center)
                .build();
            lbl.add_css_class("grid-label");
            card.append(&lbl);
            outer.append(&card);
            li.set_child(Some(&outer));
        });

        factory.connect_bind(|_, list_item| {
            let li = list_item.downcast_ref::<gtk::ListItem>().unwrap();
            let item = li.item().and_downcast::<FileItem>().unwrap();
            let outer = li.child().and_downcast::<gtk::Box>().unwrap();
            let card = outer.first_child().and_downcast::<gtk::Box>().unwrap();
            let cover_stack = card.first_child().and_downcast::<gtk::Stack>().unwrap();
            let lbl = cover_stack.next_sibling().and_downcast::<gtk::Label>().unwrap();

            if let Some(tex) = item.cover_thumb() {
                let cover = cover_stack.child_by_name("cover").and_downcast::<GridCover>().unwrap();
                cover.set_paintable(Some(tex.upcast_ref()));
                cover_stack.set_visible_child_name("cover");
            } else {
                cover_stack.set_visible_child_name("icon");
            }

            let name = item.name();
            let display = if name.chars().count() > 20 {
                let truncated: String = name.chars().take(20).collect();
                format!("{truncated}…")
            } else {
                name
            };
            lbl.set_label(&display);
        });

        let obj_weak = self.obj().downgrade();
        grid_view.connect_activate(move |_, position| {
            if let Some(obj) = obj_weak.upgrade() {
                let gs = obj.imp().grid_store.borrow();
                if let Some(item) = gs.as_ref().and_then(|s| s.item(position)).and_then(|o| o.downcast::<FileItem>().ok()) {
                    if item.is_folder() {
                        obj.imp().navigate_into(&item.path(), &item.name());
                    }
                }
            }
        });
    }

    // ── Actions ────────────────────────────────────────────────────
    fn setup_actions(&self) {
        let obj = self.obj();

        let action = gio::SimpleAction::new("play-pause", None);
        let p = self.player.borrow().as_ref().unwrap().clone();
        action.connect_activate(move |_, _| p.toggle_play());
        obj.add_action(&action);

        let action = gio::SimpleAction::new("next-track", None);
        let obj_weak = obj.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(o) = obj_weak.upgrade() { o.imp().play_next(); }
        });
        obj.add_action(&action);

        let action = gio::SimpleAction::new("prev-track", None);
        let obj_weak = obj.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(o) = obj_weak.upgrade() { o.imp().play_prev(); }
        });
        obj.add_action(&action);

        let action = gio::SimpleAction::new("open-folder", None);
        let obj_weak = obj.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(o) = obj_weak.upgrade() { o.imp().on_open_folder(); }
        });
        obj.add_action(&action);

        let action = gio::SimpleAction::new("manage-folders", None);
        let obj_weak = obj.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(o) = obj_weak.upgrade() { o.imp().on_manage_folders(); }
        });
        obj.add_action(&action);

        let action = gio::SimpleAction::new("anti-bad-bunny", None);
        let obj_weak = obj.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(o) = obj_weak.upgrade() { o.imp().on_anti_bad_bunny(); }
        });
        obj.add_action(&action);

        let action = gio::SimpleAction::new("apply-audio-output", None);
        let obj_weak = obj.downgrade();
        action.connect_activate(move |_, _| {
            if let Some(o) = obj_weak.upgrade() { o.imp().apply_audio_output(); }
        });
        obj.add_action(&action);
    }

    // ── Signal wiring ──────────────────────────────────────────────
    fn connect_signals(&self) {
        let obj = self.obj().clone();

        // Player signals
        let player = self.player.borrow().as_ref().unwrap().clone();
        let w = obj.downgrade();
        player.connect_local("state-changed", false, move |args| {
            let is_playing = args[1].get::<bool>().unwrap();
            if let Some(o) = w.upgrade() { o.imp().on_player_state(is_playing); }
            None
        });
        let w = obj.downgrade();
        player.connect_local("position-updated", false, move |args| {
            let pos = args[1].get::<f64>().unwrap();
            let dur = args[2].get::<f64>().unwrap();
            if let Some(o) = w.upgrade() { o.imp().on_position(pos, dur); }
            None
        });
        let w = obj.downgrade();
        player.connect_local("song-finished", false, move |_| {
            if let Some(o) = w.upgrade() { o.imp().on_song_finished(); }
            None
        });
        let w = obj.downgrade();
        player.connect_local("cover-art-changed", false, move |args| {
            let data = args[1].get::<glib::Bytes>().unwrap();
            if let Some(o) = w.upgrade() { o.imp().on_cover_art(&data); }
            None
        });
        let w = obj.downgrade();
        player.connect_local("tags-updated", false, move |args| {
            let title = args[1].get::<String>().unwrap();
            let artist = args[2].get::<String>().unwrap();
            let album = args[3].get::<String>().unwrap();
            let year = args[4].get::<String>().unwrap();
            if let Some(o) = w.upgrade() { o.imp().on_tags(&title, &artist, &album, &year); }
            None
        });

        // Button signals
        let p = self.player.borrow().as_ref().unwrap().clone();
        self.play_btn.borrow().as_ref().unwrap().connect_clicked(move |_| p.toggle_play());

        let w = obj.downgrade();
        self.prev_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().play_prev(); }
        });
        let w = obj.downgrade();
        self.next_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().play_next(); }
        });

        let w = obj.downgrade();
        self.vol_scale.borrow().as_ref().unwrap().connect_value_changed(move |scale| {
            if let Some(o) = w.upgrade() { o.imp().on_volume_changed(scale.value()); }
        });

        let w = obj.downgrade();
        self.seek_scale.borrow().as_ref().unwrap().connect_change_value(move |_, _, value| {
            if let Some(o) = w.upgrade() { o.imp().on_seek(value); }
            glib::Propagation::Proceed
        });

        let w = obj.downgrade();
        self.back_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().navigate_back(); }
        });
        let w = obj.downgrade();
        self.home_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().navigate_home(); }
        });
        let w = obj.downgrade();
        self.tag_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().show_tag_info(); }
        });
        let w = obj.downgrade();
        self.search_entry.borrow().as_ref().unwrap().connect_search_changed(move |entry| {
            if let Some(o) = w.upgrade() { o.imp().on_search_changed(&entry.text()); }
        });

        // Search focus → disable/enable transport accels
        let se = self.search_entry.borrow().as_ref().unwrap().clone();
        let focus_ctrl = gtk::EventControllerFocus::new();
        let w = obj.downgrade();
        focus_ctrl.connect_enter(move |_| {
            if let Some(o) = w.upgrade() { o.imp().on_search_focus(true); }
        });
        let w = obj.downgrade();
        focus_ctrl.connect_leave(move |_| {
            if let Some(o) = w.upgrade() { o.imp().on_search_focus(false); }
        });
        se.add_controller(focus_ctrl);

        let w = obj.downgrade();
        self.dock_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().on_toggle_browse(false); }
        });
        let w = obj.downgrade();
        self.locate_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().on_locate_playing(); }
        });
        let w = obj.downgrade();
        self.repeat_btn.borrow().as_ref().unwrap().connect_clicked(move |_| {
            if let Some(o) = w.upgrade() { o.imp().on_cycle_repeat(); }
        });

        let w = obj.downgrade();
        self.grid_toggle.borrow().as_ref().unwrap().connect_toggled(move |btn| {
            if let Some(o) = w.upgrade() { o.imp().on_view_toggled(btn.is_active()); }
        });

        // Browse revealer notify
        let w = obj.downgrade();
        self.browse_revealer.borrow().as_ref().unwrap().connect_child_revealed_notify(move |rev| {
            if !rev.is_child_revealed() {
                if let Some(o) = w.upgrade() {
                    let imp = o.imp();
                    imp.browse_revealer.borrow().as_ref().unwrap().set_hexpand(false);
                    imp.player_panel.borrow().as_ref().unwrap().set_hexpand(true);
                    imp.player_panel.borrow().as_ref().unwrap().set_size_request(-1, -1);
                }
            }
        });

        // Auto-hide
        let w = obj.downgrade();
        obj.connect_default_width_notify(move |_| {
            if let Some(o) = w.upgrade() {
                glib::idle_add_local_once(glib::clone!(
                    #[weak] o,
                    move || o.imp().check_auto_hide()
                ));
            }
        });

        // Mouse back button
        let back_click = gtk::GestureClick::new();
        back_click.set_button(8);
        let w = obj.downgrade();
        back_click.connect_pressed(move |_, _, _, _| {
            if let Some(o) = w.upgrade() { o.imp().navigate_back(); }
        });
        self.browse_revealer.borrow().as_ref().unwrap().add_controller(back_click);

        // Keyboard navigation in browse panel
        let key_ctrl = gtk::EventControllerKey::new();
        key_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = obj.downgrade();
        key_ctrl.connect_key_pressed(move |_, keyval, _, _| {
            let Some(o) = w.upgrade() else { return glib::Propagation::Proceed; };
            let imp = o.imp();
            if !imp.browse_visible.get() { return glib::Propagation::Proceed; }

            let search_has_focus = imp.search_entry.borrow()
                .as_ref()
                .is_some_and(|e| e.has_focus());

            // Backspace: navigate back only when search bar is empty/closed
            if keyval == gdk::Key::BackSpace && !search_has_focus {
                let search_empty = imp.search_entry.borrow()
                    .as_ref()
                    .is_none_or(|e| e.text().is_empty());
                if search_empty && !imp.nav_stack.borrow().is_empty() {
                    imp.navigate_back();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            // Alphanumeric: open search bar, focus it, and insert the character
            if !search_has_focus {
                if let Some(ch) = keyval.to_unicode() {
                    if ch.is_alphanumeric() {
                        let search_btn = imp.search_btn.borrow();
                        let search_btn = search_btn.as_ref().unwrap();
                        if !search_btn.is_active() {
                            search_btn.set_active(true);
                        }
                        let entry = imp.search_entry.borrow();
                        let entry = entry.as_ref().unwrap();
                        entry.grab_focus();
                        // Insert the character at cursor position
                        let current = entry.text().to_string();
                        let new_text = format!("{}{}", current, ch);
                        entry.set_text(&new_text);
                        // Move cursor to end
                        entry.set_position(-1);
                        return glib::Propagation::Stop;
                    }
                }
            }
            glib::Propagation::Proceed
        });
        obj.add_controller(key_ctrl);
    }

    // ── View toggle ────────────────────────────────────────────────
    fn on_view_toggled(&self, active: bool) {
        let gt = self.grid_toggle.borrow();
        let gt = gt.as_ref().unwrap();
        let icon = if active { "fp-view-list-symbolic" } else { "fp-grid-filled-symbolic" };
        gt.set_icon_name(icon);
        let tooltip = if active { gettext("List view") } else { gettext("Grid view") };
        gt.set_tooltip_text(Some(&tooltip));
        let items = self.current_items.borrow().clone();
        if !items.is_empty() {
            self.populate_list(&items);
        }
    }

    fn has_folders_only(items: &[FileItem]) -> bool {
        let mut has_folder = false;
        for item in items {
            if item.is_folder() { has_folder = true; } else { return false; }
        }
        has_folder
    }

    // ── Navigation ─────────────────────────────────────────────────
    fn navigate_into(&self, folder_path: &str, folder_name: &str) {
        let prev = (self.current_folder.borrow().clone(), self.folder_label.borrow().as_ref().unwrap().label().to_string());
        self.nav_stack.borrow_mut().push(prev);
        *self.current_folder.borrow_mut() = Some(folder_path.to_string());
        self.folder_label.borrow().as_ref().unwrap().set_label(folder_name);
        self.back_btn.borrow().as_ref().unwrap().set_visible(true);
        self.load_folder_contents(folder_path);
    }

    fn navigate_back(&self) {
        let popped = self.nav_stack.borrow_mut().pop();
        if let Some((prev_path, prev_name)) = popped {
            *self.current_folder.borrow_mut() = prev_path.clone();
            self.folder_label.borrow().as_ref().unwrap().set_label(&prev_name);
            if let Some(p) = prev_path {
                self.back_btn.borrow().as_ref().unwrap().set_visible(!self.nav_stack.borrow().is_empty());
                self.load_folder_contents(&p);
            } else {
                self.back_btn.borrow().as_ref().unwrap().set_visible(false);
                let settings = gio::Settings::new(config::APP_ID);
                let folders: Vec<String> = settings.strv("music-folders").iter()
                    .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
                self.load_virtual_root(&folders);
            }
        } else {
            let settings = gio::Settings::new(config::APP_ID);
            let folders: Vec<String> = settings.strv("music-folders").iter()
                .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
            if folders.len() > 1 {
                *self.current_folder.borrow_mut() = None;
                self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
                self.back_btn.borrow().as_ref().unwrap().set_visible(false);
                self.load_virtual_root(&folders);
            }
        }
    }

    fn navigate_home(&self) {
        let settings = gio::Settings::new(config::APP_ID);
        let folders: Vec<String> = settings.strv("music-folders").iter()
            .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
        if folders.is_empty() {
            self.on_manage_folders();
            return;
        }
        self.search_btn.borrow().as_ref().unwrap().set_active(false);
        self.search_entry.borrow().as_ref().unwrap().set_text("");
        self.nav_stack.borrow_mut().clear();
        self.return_to_root();
    }

    // ── Folder operations ──────────────────────────────────────────
    fn on_open_folder(&self) {
        self.on_manage_folders();
    }

    fn on_anti_bad_bunny(&self) {
        let settings = gio::Settings::new(config::APP_ID);
        let dlg = adw::Dialog::builder()
            .title(gettext("Anti Bad Bunny"))
            .content_width(460)
            .content_height(280)
            .build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());

        let page = adw::PreferencesPage::new();
        let group = adw::PreferencesGroup::builder()
            .title(gettext("Anti Bad Bunny"))
            .description(gettext(
                "This function will remove from your collection any song \
                 by the artist Bad Bunny (to protect your mental health). \
                 Let's promote real music! (It doesn't delete the files \
                 from disk, it just hides them, in case you have a relapse...)",
            ))
            .build();

        let row = adw::SwitchRow::builder()
            .title(gettext("Anti Bad Bunny"))
            .active(settings.boolean("anti-bad-bunny"))
            .build();

        let obj_weak = self.obj().downgrade();
        row.connect_active_notify(move |r| {
            gio::Settings::new(config::APP_ID)
                .set_boolean("anti-bad-bunny", r.is_active())
                .ok();
            if let Some(o) = obj_weak.upgrade() {
                o.imp().reload_current_view();
            }
        });
        group.add(&row);
        page.add(&group);
        toolbar.set_content(Some(&page));
        dlg.set_child(Some(&toolbar));
        dlg.present(Some(&*self.obj()));
    }

    fn reload_current_view(&self) {
        if let Some(folder) = self.current_folder.borrow().clone() {
            self.load_folder_contents(&folder);
        } else {
            let settings = gio::Settings::new(config::APP_ID);
            let folders: Vec<String> = settings.strv("music-folders").iter()
                .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
            if folders.len() > 1 {
                self.load_virtual_root(&folders);
            } else if folders.len() == 1 {
                self.load_folder_contents(&folders[0]);
            }
        }
    }

    fn on_manage_folders(&self) {
        let dlg = adw::Dialog::builder()
            .title(gettext("Manage Folders"))
            .content_width(500)
            .content_height(600)
            .build();
        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&adw::HeaderBar::new());

        let scroll = gtk::ScrolledWindow::builder().vexpand(true).build();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let clamp = adw::Clamp::builder()
            .maximum_size(460)
            .margin_top(12).margin_bottom(12).margin_start(12).margin_end(12)
            .build();

        let page = adw::PreferencesPage::new();

        let settings = gio::Settings::new(config::APP_ID);
        let initial_folders: Vec<String> = settings.strv("music-folders").iter()
            .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();

        // Shared map: path → add button (so remove can restore it)
        let add_btn_map: std::rc::Rc<RefCell<HashMap<String, gtk::Button>>> =
            std::rc::Rc::new(RefCell::new(HashMap::new()));

        // ── Section 1: XDG Music toggle ────────────────────────────
        let music_group = adw::PreferencesGroup::builder()
            .title(gettext("Music Directory"))
            .build();

        let music_dir_label = xdg_music_dir().unwrap_or_else(|| String::from("~/Music"));
        let music_row = adw::SwitchRow::builder()
            .title(display_path(&music_dir_label))
            .subtitle(gettext("Include your personal music directory"))
            .build();
        music_row.add_prefix(&gtk::Image::from_icon_name("fp-folder-music-symbolic"));
        music_row.set_active(settings.boolean("xdg-music-enabled"));
        music_group.add(&music_row);
        page.add(&music_group);

        // ── Section 2: Current folders (with remove) ───────────────
        let current_group = adw::PreferencesGroup::builder()
            .title(gettext("Selected Folders"))
            .description(gettext("Folders currently in your library"))
            .build();

        // Skip XDG Music dir from the manual list if present
        let xdg_dir = xdg_music_dir();
        for f in &initial_folders {
            if let Some(ref xd) = xdg_dir {
                if f == xd { continue; }
            }
            let row = adw::ActionRow::builder()
                .title(display_path(f))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("fp-folder-music-symbolic"));
            let remove_btn = gtk::Button::builder()
                .icon_name("fp-minus-circle-outline-symbolic")
                .tooltip_text(gettext("Remove this folder"))
                .valign(gtk::Align::Center)
                .build();
            remove_btn.add_css_class("flat");
            remove_btn.add_css_class("circular");
            let path = f.clone();
            let current_group_ref = current_group.clone();
            let row_ref = row.clone();
            let map_ref = add_btn_map.clone();
            remove_btn.connect_clicked(move |_| {
                current_group_ref.remove(&row_ref);
                let s = gio::Settings::new(config::APP_ID);
                let mut cur: Vec<String> = s.strv("music-folders").iter().map(|s| s.to_string()).collect();
                cur.retain(|p| p != &path);
                let v: Vec<&str> = cur.iter().map(|s| s.as_str()).collect();
                s.set_strv("music-folders", v).ok();
                // Restore the add button in the /mnt tree if present
                if let Some(btn) = map_ref.borrow().get(&path) {
                    btn.set_sensitive(true);
                    btn.set_icon_name("fp-plus-circle-outline-symbolic");
                }
            });
            row.add_suffix(&remove_btn);
            current_group.add(&row);
        }
        page.add(&current_group);

        // ── Section 3: /mnt disk browser ───────────────────────────
        let mnt_entries = list_mnt_entries();
        if !mnt_entries.is_empty() {
            let mnt_group = adw::PreferencesGroup::builder()
                .title(gettext("External Disks"))
                .description(gettext("Browse disks mounted in /mnt\n(To add a network disk or NAS, you must mount it in /mnt/ and it will appear in the options below)"))
                .build();

            for mnt in &mnt_entries {
                let expander = adw::ExpanderRow::builder()
                    .title(Path::new(mnt).file_name().unwrap_or_default().to_string_lossy().to_string())
                    .subtitle(mnt.clone())
                    .build();
                expander.add_prefix(&gtk::Image::from_icon_name("drive-harddisk-symbolic"));

                // Populate children on first expand
                let mnt_path = mnt.clone();
                let current_group_ref = current_group.clone();
                let obj_weak = self.obj().downgrade();
                let map_ref = add_btn_map.clone();
                let expanded_flag = std::rc::Rc::new(Cell::new(false));
                let expanded_flag2 = expanded_flag.clone();
                expander.connect_expanded_notify(move |exp| {
                    if exp.is_expanded() && !expanded_flag2.get() {
                        expanded_flag2.set(true);
                        let add_fn = |w: &gtk::Widget| exp.add_row(w);
                        Self::populate_mnt_children(&add_fn, &mnt_path, &current_group_ref, &obj_weak, &map_ref, 1);
                    }
                });

                mnt_group.add(&expander);
            }
            page.add(&mnt_group);
        } else {
            let mnt_info_msg = gettext("This application has default access to the local Music folder ({}) and can have full access to any disk mounted at /mnt (which grants access to secondary drives, or network drives / shared folders from a NAS). It is therefore recommended to mount your drives at /mnt and configure them to auto-mount at system startup, for immediate access to your folders.\nAny drives mounted at /mnt will appear in this space to be added.")
                .replace("{}", &display_path(&music_dir_label));
            let mnt_empty_group = adw::PreferencesGroup::builder()
                .title(gettext("External Disks"))
                .description(mnt_info_msg)
                .build();
            page.add(&mnt_empty_group);
        }

        clamp.set_child(Some(&page));
        scroll.set_child(Some(&clamp));
        toolbar_view.set_content(Some(&scroll));
        dlg.set_child(Some(&toolbar_view));

        // Wire XDG Music toggle
        let obj_weak = self.obj().downgrade();
        music_row.connect_active_notify(move |r| {
            let s = gio::Settings::new(config::APP_ID);
            s.set_boolean("xdg-music-enabled", r.is_active()).ok();
            let mut folders: Vec<String> = s.strv("music-folders").iter()
                .map(|s| s.to_string()).collect();
            if let Some(music_dir) = xdg_music_dir() {
                if r.is_active() {
                    if Path::new(&music_dir).is_dir() && !folders.contains(&music_dir) {
                        folders.push(music_dir);
                        let v: Vec<&str> = folders.iter().map(|s| s.as_str()).collect();
                        s.set_strv("music-folders", v).ok();
                    }
                } else {
                    folders.retain(|f| f != &music_dir);
                    let v: Vec<&str> = folders.iter().map(|s| s.as_str()).collect();
                    s.set_strv("music-folders", v).ok();
                    // Remove the folder's songs from the DB
                    if let Some(obj) = obj_weak.upgrade() {
                        obj.imp().db.borrow().as_ref().unwrap().remove_root(&music_dir);
                    }
                }
            }
        });

        let obj_weak = self.obj().downgrade();
        dlg.connect_closed(move |_| {
            if let Some(o) = obj_weak.upgrade() {
                let settings = gio::Settings::new(config::APP_ID);
                let current: Vec<String> = settings.strv("music-folders").iter()
                    .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
                let has_new = current.iter().any(|f| !initial_folders.contains(f));
                let has_removed = initial_folders.iter().any(|f| !current.contains(f));
                if (has_new || has_removed) && !current.is_empty() {
                    o.imp().start_scan_all_folders(current);
                } else if current.is_empty() {
                    o.imp().reset_library();
                } else {
                    o.imp().load_all_folders();
                }
            }
        });
        dlg.present(Some(&*self.obj()));
    }

    /// Populate the subtree under `parent_path`.
    /// `add_row_fn` abstracts over ExpanderRow::add_row (first level) and
    /// ListBox::append (deeper levels), avoiding nested ExpanderRows whose
    /// CSS arrow state misbehaves in libadwaita.
    fn populate_mnt_children(
        add_row_fn: &dyn Fn(&gtk::Widget),
        parent_path: &str,
        current_group: &adw::PreferencesGroup,
        obj_weak: &glib::WeakRef<super::FolderplayWindow>,
        add_btn_map: &std::rc::Rc<RefCell<HashMap<String, gtk::Button>>>,
        depth: u32,
    ) {
        const MAX_DEPTH: u32 = 4;

        let Ok(entries) = std::fs::read_dir(parent_path) else { return; };
        let mut dirs: Vec<String> = entries
            .filter_map(|e| {
                let e = e.ok()?;
                let p = e.path();
                // Skip hidden directories
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') { return None; }
                if p.is_dir() { Some(p.to_string_lossy().to_string()) } else { None }
            })
            .collect();
        dirs.sort();

        if dirs.is_empty() {
            let empty_row = adw::ActionRow::builder()
                .title(gettext("No subfolders"))
                .sensitive(false)
                .build();
            add_row_fn(empty_row.upcast_ref());
            return;
        }

        for dir in dirs {
            let name = Path::new(&dir).file_name().unwrap_or_default().to_string_lossy().to_string();

            // "Add" button (shared between row types)
            let add_btn = gtk::Button::builder()
                .icon_name("fp-plus-circle-outline-symbolic")
                .tooltip_text(gettext("Add this folder"))
                .valign(gtk::Align::Center)
                .build();
            add_btn.add_css_class("flat");
            add_btn.add_css_class("circular");

            // Register in the shared map so remove can restore it
            add_btn_map.borrow_mut().insert(dir.clone(), add_btn.clone());

            let dir_clone = dir.clone();
            let current_group_ref = current_group.clone();
            let add_btn_ref = add_btn.clone();
            let map_for_add = add_btn_map.clone();
            add_btn.connect_clicked(move |_| {
                let s = gio::Settings::new(config::APP_ID);
                let mut folders: Vec<String> = s.strv("music-folders").iter()
                    .map(|s| s.to_string()).collect();
                if !folders.contains(&dir_clone) {
                    folders.push(dir_clone.clone());
                    let v: Vec<&str> = folders.iter().map(|s| s.as_str()).collect();
                    s.set_strv("music-folders", v).ok();
                    // Add to the "Selected Folders" group
                    let new_row = adw::ActionRow::builder()
                        .title(display_path(&dir_clone))
                        .build();
                    new_row.add_prefix(&gtk::Image::from_icon_name("fp-folder-music-symbolic"));
                    let path_for_remove = dir_clone.clone();
                    let gr = current_group_ref.clone();
                    let nr = new_row.clone();
                    let map_for_remove = map_for_add.clone();
                    let remove_btn = gtk::Button::builder()
                        .icon_name("fp-minus-circle-outline-symbolic")
                        .tooltip_text(gettext("Remove this folder"))
                        .valign(gtk::Align::Center)
                        .build();
                    remove_btn.add_css_class("flat");
                    remove_btn.add_css_class("circular");
                    remove_btn.connect_clicked(move |_| {
                        gr.remove(&nr);
                        let s2 = gio::Settings::new(config::APP_ID);
                        let mut cur: Vec<String> = s2.strv("music-folders").iter().map(|s| s.to_string()).collect();
                        cur.retain(|p| p != &path_for_remove);
                        let v: Vec<&str> = cur.iter().map(|s| s.as_str()).collect();
                        s2.set_strv("music-folders", v).ok();
                        // Restore the add button in the /mnt tree
                        if let Some(btn) = map_for_remove.borrow().get(&path_for_remove) {
                            btn.set_sensitive(true);
                            btn.set_icon_name("fp-plus-circle-outline-symbolic");
                        }
                    });
                    new_row.add_suffix(&remove_btn);
                    current_group_ref.add(&new_row);
                    // Visual feedback: disable the add button
                    add_btn_ref.set_sensitive(false);
                    add_btn_ref.set_icon_name("object-select-symbolic");
                }
            });

            // Check if already added
            {
                let s = gio::Settings::new(config::APP_ID);
                let folders: Vec<String> = s.strv("music-folders").iter().map(|s| s.to_string()).collect();
                if folders.contains(&dir) {
                    add_btn.set_sensitive(false);
                    add_btn.set_icon_name("object-select-symbolic");
                }
            }

            if depth < MAX_DEPTH {
                // Header row for this folder
                let header_row = adw::ActionRow::builder()
                    .title(&name)
                    .subtitle(&dir)
                    .build();
                header_row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));

                // add_btn on the left of arrow so arrow is the rightmost suffix
                header_row.add_suffix(&add_btn);

                let arrow_btn = gtk::Button::from_icon_name("fp-down-smaller-symbolic");
                arrow_btn.add_css_class("flat");
                arrow_btn.add_css_class("circular");
                arrow_btn.set_valign(gtk::Align::Center);
                header_row.add_suffix(&arrow_btn);

                // Children go into a plain gtk::Box — no card borders, no
                // extra list separators.  Indented to hint at nesting depth.
                let children_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                children_box.set_margin_start(24);

                let revealer = gtk::Revealer::builder()
                    .transition_type(gtk::RevealerTransitionType::SlideDown)
                    .reveal_child(false)
                    .build();
                revealer.set_child(Some(&children_box));

                // Wrap header + revealer in ONE widget so the parent row list
                // sees a single entry → single separator, no double line.
                let folder_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                folder_box.append(&header_row);
                folder_box.append(&revealer);

                add_row_fn(folder_box.upcast_ref());

                let d = dir.clone();
                let cgr = current_group.clone();
                let ow = obj_weak.clone();
                let map_ref = add_btn_map.clone();
                let next_depth = depth + 1;
                let populated = std::rc::Rc::new(Cell::new(false));
                let is_exp = std::rc::Rc::new(Cell::new(false));
                let rev_ref = revealer;
                let cb_ref = children_box;

                arrow_btn.connect_clicked(move |btn| {
                    let opening = !is_exp.get();
                    if opening && !populated.get() {
                        populated.set(true);
                        Self::populate_mnt_children(
                            &|w| cb_ref.append(w),
                            &d, &cgr, &ow, &map_ref, next_depth,
                        );
                    }
                    is_exp.set(opening);
                    rev_ref.set_reveal_child(opening);
                    btn.set_icon_name(if opening { "fp-up-smaller-symbolic" } else { "fp-down-smaller-symbolic" });
                });
            } else {
                // Leaf level: plain ActionRow (no further expansion)
                let row = adw::ActionRow::builder()
                    .title(&name)
                    .subtitle(&dir)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
                row.add_suffix(&add_btn);
                add_row_fn(row.upcast_ref());
            }
        }
    }

    fn reset_library(&self) {
        *self.root_folder.borrow_mut() = None;
        *self.current_folder.borrow_mut() = None;
        self.nav_stack.borrow_mut().clear();
        *self.playlist.borrow_mut() = vec![];
        self.current_index.set(-1);
        self.list_store.borrow().as_ref().unwrap().remove_all();
        *self.current_items.borrow_mut() = vec![];
        self.db.borrow().as_ref().unwrap().clear_all();
        self.back_btn.borrow().as_ref().unwrap().set_visible(false);
        self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
        self.song_count_label.borrow().as_ref().unwrap().set_label("");
        self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("empty");
    }

    fn return_to_root(&self) {
        let settings = gio::Settings::new(config::APP_ID);
        let folders: Vec<String> = settings.strv("music-folders").iter()
            .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
        if folders.is_empty() {
            self.reset_library();
            return;
        }
        if folders.len() == 1 {
            let path = folders[0].clone();
            *self.root_folder.borrow_mut() = Some(path.clone());
            *self.current_folder.borrow_mut() = Some(path.clone());
            self.nav_stack.borrow_mut().clear();
            self.back_btn.borrow().as_ref().unwrap().set_visible(false);
            self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folder"));
            self.load_folder_contents(&path);
        } else {
            *self.root_folder.borrow_mut() = None;
            *self.current_folder.borrow_mut() = None;
            self.nav_stack.borrow_mut().clear();
            self.back_btn.borrow().as_ref().unwrap().set_visible(false);
            self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
            self.load_virtual_root(&folders);
        }
    }

    fn load_all_folders(&self) {
        let settings = gio::Settings::new(config::APP_ID);
        let folders: Vec<String> = settings.strv("music-folders").iter()
            .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
        if folders.is_empty() {
            self.reset_library();
            return;
        }
        if folders.len() == 1 {
            self.load_root_folder(&folders[0].clone());
        } else {
            *self.root_folder.borrow_mut() = None;
            *self.current_folder.borrow_mut() = None;
            self.nav_stack.borrow_mut().clear();
            self.back_btn.borrow().as_ref().unwrap().set_visible(false);
            self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
            *self.playlist.borrow_mut() = vec![];
            self.current_index.set(-1);
            self.load_virtual_root(&folders);
            let db = self.db.borrow().as_ref().unwrap().clone();
            let shutdown = self.shutdown.clone();
            let folders2 = folders.clone();
            let obj_weak: glib::SendWeakRef<super::FolderplayWindow> = self.obj().downgrade().into();
            std::thread::spawn(move || {
                deep_scan_and_index(&db, &folders2, &shutdown, obj_weak, false);
            });
        }
    }

    /// Show the loading screen and launch a deep scan with progress reporting
    /// for multiple folders (used when the Manage Folders dialog adds new ones).
    fn start_scan_all_folders(&self, folders: Vec<String>) {
        *self.root_folder.borrow_mut() = None;
        *self.current_folder.borrow_mut() = None;
        self.nav_stack.borrow_mut().clear();
        self.back_btn.borrow().as_ref().unwrap().set_visible(false);
        self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
        *self.playlist.borrow_mut() = vec![];
        self.current_index.set(-1);

        self.update_scan_progress(0);
        self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("loading");
        self.filter_model.borrow().as_ref().unwrap().set_filter(None::<&gtk::Filter>);
        self.search_entry.borrow().as_ref().unwrap().set_text("");

        let db = self.db.borrow().as_ref().unwrap().clone();
        let shutdown = self.shutdown.clone();
        let obj_weak: glib::SendWeakRef<super::FolderplayWindow> = self.obj().downgrade().into();
        std::thread::spawn(move || {
            deep_scan_and_index(&db, &folders, &shutdown, obj_weak, true);
        });
    }

    /// Show the loading screen and launch a deep scan with progress reporting.
    /// Called when the user adds a brand-new folder.
    #[allow(dead_code)]
    fn start_scan_new_folder(&self, path: String) {
        // Reset navigation to root
        *self.root_folder.borrow_mut() = None;
        *self.current_folder.borrow_mut() = None;
        self.nav_stack.borrow_mut().clear();
        self.back_btn.borrow().as_ref().unwrap().set_visible(false);
        self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
        *self.playlist.borrow_mut() = vec![];
        self.current_index.set(-1);

        // Show loading screen with count label reset
        self.update_scan_progress(0);
        self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("loading");
        self.filter_model.borrow().as_ref().unwrap().set_filter(None::<&gtk::Filter>);
        self.search_entry.borrow().as_ref().unwrap().set_text("");

        let db = self.db.borrow().as_ref().unwrap().clone();
        let shutdown = self.shutdown.clone();
        let obj_weak: glib::SendWeakRef<super::FolderplayWindow> = self.obj().downgrade().into();
        std::thread::spawn(move || {
            deep_scan_and_index(&db, &[path], &shutdown, obj_weak, true);
        });
    }

    /// Called from the background scan thread (via idle_add) to update the count label.
    pub fn update_scan_progress(&self, count: u32) {
        if let Some(lbl) = self.scan_count_label.borrow().as_ref() {
            if count == 0 {
                lbl.set_label(&gettext("Scanning…"));
            } else {
                lbl.set_label(&ngettext(
                    "Found {n} song…",
                    "Found {n} songs…",
                    count,
                ).replace("{n}", &count.to_string()));
            }
        }
    }

    pub fn update_scan_covers_label(&self) {
        if let Some(lbl) = self.scan_count_label.borrow().as_ref() {
            lbl.set_label(&gettext("Reading covers…"));
        }
    }

    /// Called from the background scan thread (via idle_add) once scan + covers are done.
    pub fn finish_scan_and_reload(&self) {
        if let Some(lbl) = self.scan_count_label.borrow().as_ref() {
            lbl.set_label("");
        }
        self.load_all_folders();
    }

    pub fn open_file(&self, path: &str) {
        if !Path::new(path).is_file() { return; }
        if let Some(ext) = Path::new(path).extension() {
            if !library_db::is_audio_ext(&ext.to_string_lossy()) { return; }
        } else { return; }
        self.external_file.set(true);
        self.play_file(path);
        self.locate_btn.borrow().as_ref().unwrap().set_visible(false);
    }

    fn load_virtual_root(&self, folders: &[String]) {
        self.filter_model.borrow().as_ref().unwrap().set_filter(None::<&gtk::Filter>);
        self.search_entry.borrow().as_ref().unwrap().set_text("");

        let db = self.db.borrow().as_ref().unwrap().clone();
        let hide_bb = gio::Settings::new(config::APP_ID).boolean("anti-bad-bunny");
        let mut items = Vec::new();
        for f in folders {
            if hide_bb && is_bad_bunny(&Path::new(f).file_name().unwrap_or_default().to_string_lossy()) {
                continue;
            }
            let fi = FileItem::new(f, &Path::new(f).file_name().unwrap_or_default().to_string_lossy(), true);
            fi.set_cover_thumb(get_folder_preview(&db, f, 0));
            items.push(fi);
        }
        self.populate_list(&items);
    }

    fn load_root_folder(&self, path: &str) {
        let path = path.to_string();
        *self.root_folder.borrow_mut() = Some(path.clone());
        *self.current_folder.borrow_mut() = Some(path.clone());
        self.nav_stack.borrow_mut().clear();
        self.back_btn.borrow().as_ref().unwrap().set_visible(false);
        self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folder"));
        *self.playlist.borrow_mut() = vec![];
        self.current_index.set(-1);
        self.load_folder_contents(&path);
        let db = self.db.borrow().as_ref().unwrap().clone();
        let shutdown = self.shutdown.clone();
        let obj_weak: glib::SendWeakRef<super::FolderplayWindow> = self.obj().downgrade().into();
        let p = path.clone();
        std::thread::spawn(move || {
            deep_scan_and_index(&db, &[p], &shutdown, obj_weak, false);
        });
    }

    fn load_folder_contents(&self, path: &str) {
        self.filter_model.borrow().as_ref().unwrap().set_filter(None::<&gtk::Filter>);
        self.search_entry.borrow().as_ref().unwrap().set_text("");

        let db = self.db.borrow().as_ref().unwrap().clone();
        let needs_rescan = db.folder_needs_rescan(path);

        if needs_rescan {
            // Slow path: folder changed on disk — scan then show results
            self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("loading");
            let root = self.root_folder.borrow().clone();
            let path = path.to_string();
            let obj_weak: glib::SendWeakRef<super::FolderplayWindow> = self.obj().downgrade().into();
            let shutdown = self.shutdown.clone();
            std::thread::spawn(move || {
                if shutdown.load(Ordering::Relaxed) { return; }
                let parent = if root.as_deref() != Some(&path) { root.as_deref() } else { None };
                db.scan_folder(&path, parent);
                db.update_has_audio_for_folder(&path);
                let (items, has_unscanned) = build_folder_items(&db, &path);
                unsafe {
                    idle_add_once_raw(move || {
                        if let Some(obj) = obj_weak.upgrade() {
                            obj.imp().populate_list(&items);
                        }
                    });
                }
                if !shutdown.load(Ordering::Relaxed) && has_unscanned {
                    enrich_folder_metadata(&db, &path);
                }
            });
        } else {
            // Fast path: folder already in DB — build items synchronously (no spinner flash)
            let (items, has_unscanned) = build_folder_items(&db, path);
            self.populate_list(&items);

            // Enrich unscanned songs in background if needed
            if has_unscanned {
                let db2 = db.clone();
                let path2 = path.to_string();
                std::thread::spawn(move || {
                    enrich_folder_metadata(&db2, &path2);
                });
            }
        }
    }

    fn populate_list(&self, items: &[FileItem]) {
        *self.current_items.borrow_mut() = items.to_vec();

        if items.is_empty() {
            self.list_store.borrow().as_ref().unwrap().remove_all();
            self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("empty");
            return;
        }

        let use_grid = self.grid_toggle.borrow().as_ref().unwrap().is_active()
            && Self::has_folders_only(items);

        self.grid_toggle.borrow().as_ref().unwrap().set_visible(Self::has_folders_only(items));

        let gobjects: Vec<glib::Object> = items.iter().map(|i| i.clone().upcast::<glib::Object>()).collect();

        if use_grid {
            self.list_store.borrow().as_ref().unwrap().remove_all();
            let gs = self.grid_store.borrow();
            let gs = gs.as_ref().unwrap();
            gs.splice(0, gs.n_items(), &gobjects);
            self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("grid");
        } else {
            self.grid_store.borrow().as_ref().unwrap().remove_all();
            let ls = self.list_store.borrow();
            let ls = ls.as_ref().unwrap();
            ls.splice(0, ls.n_items(), &gobjects);
            self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("list");

            let mut scroll_pos = 0u32;
            let pending = self.pending_scroll_path.borrow().clone();
            if let Some(ref pending_path) = pending {
                for (i, item) in items.iter().enumerate() {
                    if item.path() == *pending_path {
                        scroll_pos = i as u32;
                        break;
                    }
                }
            }
            *self.pending_scroll_path.borrow_mut() = None;
            self.list_view.borrow().as_ref().unwrap().scroll_to(scroll_pos, gtk::ListScrollFlags::FOCUS, None);
        }
    }

    fn on_list_activated(&self, position: u32) {
        let fm = self.filter_model.borrow();
        let item = fm.as_ref()
            .and_then(|fm| fm.item(position))
            .and_then(|o| o.downcast::<FileItem>().ok());
        if let Some(item) = item {
            if item.is_folder() {
                self.navigate_into(&item.path(), &item.name());
            } else {
                self.external_file.set(false);
                self.play_file(&item.path());
            }
        }
    }

    // ── Search ─────────────────────────────────────────────────────
    fn on_search_changed(&self, query: &str) {
        let query_raw = query.trim().to_string();
        let query = query_raw.to_lowercase();
        if query.is_empty() {
            if let Some(folder) = self.current_folder.borrow().as_ref() {
                self.folder_label.borrow().as_ref().unwrap()
                    .set_label(&Path::new(folder).file_name().unwrap_or_default().to_string_lossy());
            } else if self.root_folder.borrow().is_some() {
                self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folder"));
            } else {
                self.folder_label.borrow().as_ref().unwrap().set_label(&gettext("Folders"));
            }
            let items = self.current_items.borrow().clone();
            if !items.is_empty() {
                self.populate_list(&items);
            } else {
                self.filter_model.borrow().as_ref().unwrap().set_filter(None::<&gtk::Filter>);
            }
            return;
        }

        // Single character: jump to first matching item without filtering
        if query.chars().count() == 1 {
            let target = query.chars().next().unwrap();
            let items = self.current_items.borrow();
            let pos = items.iter().position(|item| {
                item.name()
                    .chars()
                    .next()
                    .map(|c| c.to_lowercase().next().unwrap_or(c) == target)
                    .unwrap_or(false)
            });
            drop(items);
            if let Some(pos) = pos {
                let pos = pos as u32;
                let is_grid = self.grid_toggle.borrow().as_ref().is_some_and(|t| t.is_active());
                if is_grid {
                    if let Some(gv) = self.grid_view.borrow().as_ref() {
                        gv.scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
                    }
                } else if let Some(lv) = self.list_view.borrow().as_ref() {
                    lv.scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
                }
            }
            return;
        }

        self.folder_label.borrow().as_ref().unwrap()
            .set_label(&format!("{}: {}", gettext("Search"), query));

        let words: Vec<String> = query.split_whitespace().map(|s| s.to_string()).collect();
        let settings = gio::Settings::new(config::APP_ID);
        let roots: Vec<String> = settings.strv("music-folders").iter()
            .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();
        let hide_bb = settings.boolean("anti-bad-bunny");

        let db = self.db.borrow().as_ref().unwrap().clone();
        let results = db.search_songs(&words, &roots);

        self.list_store.borrow().as_ref().unwrap().remove_all();
        let mut items = Vec::new();
        for rec in &results {
            if hide_bb && is_bad_bunny(&rec.artist) { continue; }
            let item = FileItem::new(&rec.path, &rec.name, false);
            let title_str = if rec.title.is_empty() {
                Path::new(&rec.name).file_stem().unwrap_or_default().to_string_lossy().to_string()
            } else {
                rec.title.clone()
            };
            item.set_title(&title_str);
            item.set_artist(&rec.artist);
            item.set_album(&rec.album);
            item.set_format_type(&rec.format_type);
            item.set_sample_rate(rec.sample_rate);
            item.set_bits_per_sample(rec.bits_per_sample);
            item.set_bitrate(rec.bitrate);
            item.set_duration(rec.duration);

            let song_cover = db.get_song_cover(&rec.path).or_else(|| db.get_cover(&rec.folder));
            if let Some(data) = song_cover {
                if let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(&data)) {
                    item.set_cover_thumb(Some(tex));
                }
            }
            items.push(item);
        }
        for item in &items {
            self.list_store.borrow().as_ref().unwrap().append(item);
        }
        self.filter_model.borrow().as_ref().unwrap().set_filter(None::<&gtk::Filter>);
        self.grid_toggle.borrow().as_ref().unwrap().set_visible(false);
        if items.is_empty() {
            self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("search-empty");
        } else {
            self.browse_stack.borrow().as_ref().unwrap().set_visible_child_name("list");
        }
    }

    fn on_search_focus(&self, focused: bool) {
        if let Some(app) = self.obj().application() {
            let app = app.downcast::<gtk::Application>().unwrap();
            if focused {
                app.set_accels_for_action("win.play-pause", &[]);
                app.set_accels_for_action("win.next-track", &[]);
                app.set_accels_for_action("win.prev-track", &[]);
            } else {
                app.set_accels_for_action("win.play-pause", &["space"]);
                app.set_accels_for_action("win.next-track", &["Right"]);
                app.set_accels_for_action("win.prev-track", &["Left"]);
            }
        }
    }

    // ── Playback ───────────────────────────────────────────────────
    fn play_file(&self, path: &str) {
        let Ok(uri) = glib::filename_to_uri(path, None) else {
            eprintln!("play_file: invalid path: {path}");
            return;
        };
        let uri = uri.to_string();
        self.player.borrow().as_ref().unwrap().play_uri(&uri);

        let old_path = self.playing_path.borrow().clone();
        *self.playing_path.borrow_mut() = Some(path.to_string());
        {
            let pl = self.playlist.borrow();
            if let Some(pos) = pl.iter().position(|p| p == path) {
                self.current_index.set(pos as i32);
            }
        }

        self.update_now_playing_widgets(old_path.as_deref(), Some(path));
        if !self.external_file.get() {
            let w = self.obj().width();
            self.locate_btn.borrow().as_ref().unwrap().set_visible(w >= 770);
        }

        *self.cover_texture.borrow_mut() = None;
        self.cover_stack.borrow().as_ref().unwrap().set_visible_child_name("placeholder");
        self.update_background_colors(None);

        let name = Path::new(path).file_stem().unwrap_or_default().to_string_lossy().into_owned();
        self.title_label.borrow().as_ref().unwrap().set_label(&name);
        self.subtitle_label.borrow().as_ref().unwrap().set_label("");
        self.format_label.borrow().as_ref().unwrap().set_visible(false);
        self.hires_icon_player.borrow().as_ref().unwrap().set_visible(false);
        let mut tags = HashMap::new();
        tags.insert("path".to_string(), path.to_string());
        *self.current_tags.borrow_mut() = tags;
        self.tag_btn.borrow().as_ref().unwrap().set_visible(true);

        self.discover_audio_info(path);

        let folder = Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(cover_file) = get_cover_file(&folder) {
            if let Ok(tex) = gdk::Texture::from_filename(&cover_file) {
                self.set_cover_art(&tex);
            }
        }
    }

    fn play_next(&self) {
        let pl = self.playlist.borrow();
        if pl.is_empty() { return; }
        let idx = (self.current_index.get() + 1) % pl.len() as i32;
        let path = pl[idx as usize].clone();
        drop(pl);
        self.current_index.set(idx);
        self.play_file(&path);
    }

    fn play_prev(&self) {
        let pl = self.playlist.borrow();
        if pl.is_empty() { return; }
        let mut idx = self.current_index.get() - 1;
        if idx < 0 { idx = pl.len() as i32 - 1; }
        let path = pl[idx as usize].clone();
        drop(pl);
        self.current_index.set(idx);
        self.play_file(&path);
    }

    fn discover_audio_info(&self, path: &str) {
        let path = path.to_string();
        let obj_weak: glib::SendWeakRef<super::FolderplayWindow> = self.obj().downgrade().into();
        std::thread::spawn(move || {
            let Ok(discoverer) = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(3)) else { return };
            let Ok(uri) = glib::filename_to_uri(&path, None) else { return };
            let Ok(info) = discoverer.discover_uri(&uri) else { return };

            let ext = Path::new(&path).extension().unwrap_or_default().to_string_lossy().to_uppercase();
            let mut sample_rate = 0i32;
            let mut bitrate = 0i32;
            let mut bits = 0i32;
            let mut channels = 0i32;

            let streams = info.audio_streams();
            if let Some(a) = streams.first() {
                let a = a.clone().downcast::<gst_pbutils::DiscovererAudioInfo>().unwrap();
                let br = a.bitrate();
                bitrate = (br / 1000) as i32;
                sample_rate = a.sample_rate() as i32;
                bits = a.depth() as i32;
                channels = a.channels() as i32;
            }

            let dur = info.duration().map(|d| d.nseconds() as f64 / 1_000_000_000.0).unwrap_or(0.0);

            let mut parts = vec![ext.clone()];
            if (ext == "DSF" || ext == "DFF") && sample_rate > 0 {
                let dsd_level = (sample_rate as f64 * 8.0 / 44100.0).round() as i32;
                parts.push(format!("DSD{dsd_level}"));
            } else if LOSSLESS_FORMATS.contains(&ext.as_str()) && sample_rate > 0 {
                let b = if bits > 0 { bits } else { 16 };
                parts.push(format!("{b}-bit/{}kHz", sample_rate as f64 / 1000.0));
            } else if bitrate > 0 {
                parts.push(format!("{bitrate} kbps"));
            }
            let label = parts.join(" \u{2022} ");
            let is_hires = sample_rate >= 48000;

            let mut genre = String::new();
            let mut track_number = 0u32;
            if let Some(tags) = info.tags() {
                if let Some(g) = tags.get::<gst::tags::Genre>() { genre = g.get().to_string(); }
                if let Some(t) = tags.get::<gst::tags::TrackNumber>() { track_number = t.get(); }
            }

            glib::idle_add_once(move || {
                if let Some(obj) = obj_weak.upgrade() {
                    let imp = obj.imp();
                    imp.format_label.borrow().as_ref().unwrap().set_label(&label);
                    imp.format_label.borrow().as_ref().unwrap().set_visible(true);
                    imp.hires_icon_player.borrow().as_ref().unwrap().set_visible(is_hires);
                    let mut tags = imp.current_tags.borrow_mut();
                    tags.insert("format".to_string(), ext);
                    tags.insert("sample_rate".to_string(), sample_rate.to_string());
                    tags.insert("bits_per_sample".to_string(), bits.to_string());
                    tags.insert("bitrate".to_string(), bitrate.to_string());
                    tags.insert("duration".to_string(), dur.to_string());
                    tags.insert("channels".to_string(), channels.to_string());
                    tags.insert("genre".to_string(), genre);
                    tags.insert("track_number".to_string(), track_number.to_string());
                }
            });
        });
    }

    // ── Player callbacks ───────────────────────────────────────────
    fn on_player_state(&self, is_playing: bool) {
        let icon = if is_playing { "fp-media-playback-pause-symbolic" } else { "fp-media-playback-start-symbolic" };
        self.play_btn.borrow().as_ref().unwrap().set_icon_name(icon);
    }

    fn on_position(&self, position: f64, duration: f64) {
        let now = glib::monotonic_time();
        if (now - self.last_seek_time.get()) > 800000
            && duration > 0.0 {
                self.seek_scale.borrow().as_ref().unwrap().set_range(0.0, duration);
                self.seek_scale.borrow().as_ref().unwrap().set_value(position);
            }
        self.pos_label.borrow().as_ref().unwrap().set_label(&format_time(position));
        self.dur_label.borrow().as_ref().unwrap().set_label(&format_time(duration));
    }

    fn on_song_finished(&self) {
        let mode = self.repeat_mode.get();
        if mode == REPEAT_LOOP {
            // Repeat the current song indefinitely
            let pl = self.playlist.borrow();
            let idx = self.current_index.get();
            if idx >= 0 && (idx as usize) < pl.len() {
                let path = pl[idx as usize].clone();
                drop(pl);
                self.play_file(&path);
            }
            return;
        }
        if mode == REPEAT_ONCE {
            self.repeat_mode.set(REPEAT_CONSECUTIVE);
            self.repeat_btn.borrow().as_ref().unwrap().set_icon_name(REPEAT_ICONS[REPEAT_CONSECUTIVE as usize]);
            self.repeat_btn.borrow().as_ref().unwrap().set_tooltip_text(Some(&gettext("Consecutive")));
            self.update_repeat_icon_in_list();
            let pl = self.playlist.borrow();
            let idx = self.current_index.get();
            if idx >= 0 && (idx as usize) < pl.len() {
                let path = pl[idx as usize].clone();
                drop(pl);
                self.play_file(&path);
            }
            return;
        }
        // REPEAT_CONSECUTIVE: advance within the same folder, wrap at end
        let pl = self.playlist.borrow();
        if pl.is_empty() { return; }
        let idx = self.current_index.get();
        if idx < 0 || (idx as usize) >= pl.len() { return; }
        let current_folder = Path::new(&pl[idx as usize])
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Collect indices of songs in the same folder
        let folder_indices: Vec<usize> = pl.iter().enumerate()
            .filter(|(_, p)| {
                Path::new(p).parent().map(|pp| pp.to_string_lossy().into_owned()).unwrap_or_default() == current_folder
            })
            .map(|(i, _)| i)
            .collect();
        drop(pl);
        if folder_indices.is_empty() { return; }
        let cur_pos = folder_indices.iter().position(|&i| i == idx as usize).unwrap_or(0);
        let next_pos = if cur_pos + 1 < folder_indices.len() {
            cur_pos + 1
        } else {
            0 // Wrap to first song in the same folder
        };
        let new_idx = folder_indices[next_pos] as i32;
        let pl = self.playlist.borrow();
        let path = pl[new_idx as usize].clone();
        drop(pl);
        self.current_index.set(new_idx);
        self.play_file(&path);
    }

    fn on_cover_art(&self, data: &glib::Bytes) {
        if let Ok(tex) = gdk::Texture::from_bytes(data) {
            self.set_cover_art(&tex);
        }
    }

    fn on_tags(&self, title: &str, artist: &str, album: &str, year: &str) {
        if !title.is_empty() {
            self.title_label.borrow().as_ref().unwrap().set_label(title);
            self.current_tags.borrow_mut().insert("title".to_string(), title.to_string());
        }
        if !artist.is_empty() {
            self.current_tags.borrow_mut().insert("artist".to_string(), artist.to_string());
        }
        if !album.is_empty() {
            self.current_tags.borrow_mut().insert("album".to_string(), album.to_string());
        }
        if !year.is_empty() {
            self.current_tags.borrow_mut().insert("year".to_string(), year.to_string());
        }
        let parts: Vec<&str> = [artist, album].iter().copied().filter(|s| !s.is_empty()).collect();
        let mut subtitle = parts.join(" \u{2014} ");
        if !year.is_empty() {
            if !subtitle.is_empty() { subtitle.push_str(" \u{2014} "); }
            subtitle.push_str(year);
        }
        if !subtitle.is_empty() {
            self.subtitle_label.borrow().as_ref().unwrap().set_label(&subtitle);
        }
    }

    // ── Cover art & gradient ───────────────────────────────────────
    fn set_cover_art(&self, texture: &gdk::Texture) {
        *self.cover_texture.borrow_mut() = Some(texture.clone());
        self.cover_picture.borrow().as_ref().unwrap().set_paintable(Some(texture.upcast_ref()));
        self.cover_stack.borrow().as_ref().unwrap().set_visible_child_name("art");
        self.update_background_colors(Some(texture));
    }

    fn update_background_colors(&self, texture: Option<&gdk::Texture>) {
        let dynamic = self.dynamic_css.borrow();
        let dynamic = dynamic.as_ref().unwrap();
        if let Some(tex) = texture {
            if let Some(palette) = extract_palette(tex, 3) {
                if palette.len() >= 3 {
                    let c = &palette;
                    let css = format!(
                        ".album-bg {{\
                          background:\
                            linear-gradient(127deg,rgba({},{},{},0.55),rgba({},{},{},0.0) 70.71%),\
                            linear-gradient(217deg,rgba({},{},{},0.55),rgba({},{},{},0.0) 70.71%),\
                            linear-gradient(336deg,rgba({},{},{},0.55),rgba({},{},{},0.0) 70.71%);\
                        }}",
                        c[0].0,c[0].1,c[0].2, c[0].0,c[0].1,c[0].2,
                        c[1].0,c[1].1,c[1].2, c[1].0,c[1].1,c[1].2,
                        c[2].0,c[2].1,c[2].2, c[2].0,c[2].1,c[2].2,
                    );
                    dynamic.load_from_string(&css);
                    return;
                }
            }
        }
        dynamic.load_from_string("");
    }

    fn update_now_playing_widgets(&self, old_path: Option<&str>, new_path: Option<&str>) {
        let bound = self.bound_rows.borrow();
        let rmode = self.repeat_mode.get();
        if let Some(old) = old_path {
            if let Some(entries) = bound.get(old) {
                for (row, icon, rep_icon) in entries {
                    row.remove_css_class("now-playing-row");
                    icon.set_visible(false);
                    rep_icon.set_visible(false);
                }
            }
        }
        if let Some(new) = new_path {
            if let Some(entries) = bound.get(new) {
                for (row, icon, rep_icon) in entries {
                    row.add_css_class("now-playing-row");
                    icon.set_visible(true);
                    if rmode != REPEAT_CONSECUTIVE {
                        rep_icon.set_icon_name(Some(REPEAT_ICONS[rmode as usize]));
                        rep_icon.set_visible(true);
                    } else {
                        rep_icon.set_visible(false);
                    }
                }
            }
        }
    }

    // ── Tag info dialog ────────────────────────────────────────────
    fn show_tag_info(&self) {
        let tags = self.current_tags.borrow().clone();
        if tags.is_empty() { return; }

        let dlg = adw::Dialog::builder()
            .title(gettext("Song Info"))
            .content_width(380)
            .content_height(420)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();

        let clamp = adw::Clamp::builder()
            .maximum_size(360)
            .margin_top(12)
            .margin_bottom(12)
            .build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_start(16)
            .margin_end(16)
            .build();

        let tag_group = adw::PreferencesGroup::builder().title(gettext("Tags")).build();
        let fields = [
            (gettext("Title"), tags.get("title").cloned().unwrap_or_default()),
            (gettext("Artist"), tags.get("artist").cloned().unwrap_or_default()),
            (gettext("Album"), tags.get("album").cloned().unwrap_or_default()),
            (gettext("Year"), tags.get("year").cloned().unwrap_or_default()),
            (gettext("Genre"), tags.get("genre").cloned().unwrap_or_default()),
            (gettext("Track"), tags.get("track_number").cloned().unwrap_or_default()),
        ];
        for (label, value) in &fields {
            let subtitle = if value.is_empty() { "\u{2014}".to_string() } else { value.clone() };
            let row = adw::ActionRow::builder().title(label).subtitle(&subtitle).build();
            tag_group.add(&row);
        }
        content.append(&tag_group);

        let tech_group = adw::PreferencesGroup::builder().title(gettext("Audio")).build();
        let dur: f64 = tags.get("duration").and_then(|d| d.parse().ok()).unwrap_or(0.0);
        let sr: i32 = tags.get("sample_rate").and_then(|s| s.parse().ok()).unwrap_or(0);
        let bits: i32 = tags.get("bits_per_sample").and_then(|s| s.parse().ok()).unwrap_or(0);
        let ch: i32 = tags.get("channels").and_then(|s| s.parse().ok()).unwrap_or(0);
        let tech_fields = [
            (gettext("Format"), tags.get("format").cloned().unwrap_or_default()),
            (gettext("Duration"), if dur > 0.0 { format_time(dur) } else { "\u{2014}".to_string() }),
            (gettext("Sample Rate"), if sr > 0 { format!("{sr} Hz") } else { "\u{2014}".to_string() }),
            (gettext("Bit Depth"), if bits > 0 { format!("{bits}-bit") } else { "\u{2014}".to_string() }),
            (gettext("Channels"), if ch > 0 { ch.to_string() } else { "\u{2014}".to_string() }),
        ];
        for (label, value) in &tech_fields {
            let row = adw::ActionRow::builder().title(label).subtitle(value).build();
            tech_group.add(&row);
        }
        content.append(&tech_group);

        if let Some(path) = tags.get("path") {
            let file_group = adw::PreferencesGroup::builder().title(gettext("File")).build();
            let row = adw::ActionRow::builder()
                .title(gettext("Filename"))
                .subtitle(Path::new(path).file_name().unwrap_or_default().to_string_lossy().to_string())
                .build();
            file_group.add(&row);
            let row2 = adw::ActionRow::builder()
                .title(gettext("Location"))
                .subtitle(Path::new(path).parent().unwrap_or(Path::new("")).to_string_lossy().to_string())
                .subtitle_lines(2)
                .build();
            file_group.add(&row2);
            if let Ok(meta) = std::fs::metadata(path) {
                let size = meta.len();
                let size_str = if size >= 1024 * 1024 {
                    format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
                } else {
                    format!("{} KB", size / 1024)
                };
                let row3 = adw::ActionRow::builder().title(gettext("Size")).subtitle(&size_str).build();
                file_group.add(&row3);
            }
            content.append(&file_group);
        }

        clamp.set_child(Some(&content));
        scroll.set_child(Some(&clamp));
        toolbar_view.set_content(Some(&scroll));
        dlg.set_child(Some(&toolbar_view));
        dlg.present(Some(&*self.obj()));
    }

    // ── Control handlers ───────────────────────────────────────────
    fn on_toggle_browse(&self, auto: bool) {
        let visible = !self.browse_visible.get();
        self.browse_visible.set(visible);
        if visible {
            if !auto { self.browse_manual_closed.set(false); }
            self.player_panel.borrow().as_ref().unwrap().set_hexpand(false);
            self.player_panel.borrow().as_ref().unwrap().set_size_request(PLAYER_WIDTH, -1);
            self.browse_revealer.borrow().as_ref().unwrap().set_hexpand(true);
            self.browse_revealer.borrow().as_ref().unwrap().set_reveal_child(true);
            self.sep.borrow().as_ref().unwrap().set_visible(true);
        } else {
            if !auto { self.browse_manual_closed.set(true); }
            self.browse_revealer.borrow().as_ref().unwrap().set_reveal_child(false);
            self.sep.borrow().as_ref().unwrap().set_visible(false);
        }
    }

    fn on_locate_playing(&self) {
        let pl = self.playlist.borrow();
        let idx = self.current_index.get();
        if pl.is_empty() || idx < 0 || (idx as usize) >= pl.len() { return; }
        let playing_path = pl[idx as usize].clone();
        drop(pl);
        let folder_path = Path::new(&playing_path).parent().unwrap_or(Path::new("")).to_string_lossy().into_owned();

        if !self.browse_visible.get() {
            self.on_toggle_browse(false);
        }
        self.search_btn.borrow().as_ref().unwrap().set_active(false);
        self.search_entry.borrow().as_ref().unwrap().set_text("");
        self.nav_stack.borrow_mut().clear();

        let settings = gio::Settings::new(config::APP_ID);
        let folders: Vec<String> = settings.strv("music-folders").iter()
            .map(|s| s.to_string()).filter(|f| !f.is_empty()).collect();

        if folders.len() > 1 {
            self.nav_stack.borrow_mut().push((None, gettext("Folders")));
            *self.current_folder.borrow_mut() = Some(folder_path.clone());
            self.folder_label.borrow().as_ref().unwrap()
                .set_label(&Path::new(&folder_path).file_name().unwrap_or_default().to_string_lossy());
            self.back_btn.borrow().as_ref().unwrap().set_visible(true);
        } else if folders.len() == 1 {
            let root = &folders[0];
            if folder_path == *root {
                *self.current_folder.borrow_mut() = Some(root.clone());
                self.folder_label.borrow().as_ref().unwrap()
                    .set_label(&Path::new(root).file_name().unwrap_or_default().to_string_lossy());
                self.back_btn.borrow().as_ref().unwrap().set_visible(false);
            } else {
                self.nav_stack.borrow_mut().push((Some(root.clone()), gettext("Folder")));
                self.back_btn.borrow().as_ref().unwrap().set_visible(true);
                *self.current_folder.borrow_mut() = Some(folder_path.clone());
                if let Some(fname) = Path::new(&folder_path).file_name() {
                    self.folder_label.borrow().as_ref().unwrap().set_label(&fname.to_string_lossy());
                }
            }
        } else {
            return;
        }
        *self.pending_scroll_path.borrow_mut() = Some(playing_path);
        self.load_folder_contents(&folder_path);
    }

    fn check_auto_hide(&self) {
        if self.auto_hide_busy.get() { return; }
        let w = self.obj().width();
        if w <= 0 { return; }
        self.auto_hide_busy.set(true);
        self.dock_btn.borrow().as_ref().unwrap().set_visible(w >= 770);
        let pl = self.playlist.borrow();
        let idx = self.current_index.get();
        self.locate_btn.borrow().as_ref().unwrap().set_visible(
            w >= 770 && !pl.is_empty() && idx >= 0 && (idx as usize) < pl.len(),
        );
        drop(pl);
        if (w < 900 && self.browse_visible.get())
            || (w >= 900 && !self.browse_visible.get() && !self.browse_manual_closed.get())
        {
            self.on_toggle_browse(true);
        }
        self.auto_hide_busy.set(false);
    }

    fn on_cycle_repeat(&self) {
        let mode = (self.repeat_mode.get() + 1) % 3;
        self.repeat_mode.set(mode);
        self.repeat_btn.borrow().as_ref().unwrap().set_icon_name(REPEAT_ICONS[mode as usize]);
        let tooltips = [gettext("Consecutive"), gettext("Repeat Once"), gettext("Repeat Indefinitely")];
        self.repeat_btn.borrow().as_ref().unwrap().set_tooltip_text(Some(&tooltips[mode as usize]));
        self.update_repeat_icon_in_list();
    }

    fn update_repeat_icon_in_list(&self) {
        let playing = self.playing_path.borrow().clone();
        if let Some(path) = playing {
            let bound = self.bound_rows.borrow();
            let rmode = self.repeat_mode.get();
            if let Some(entries) = bound.get(&path) {
                for (_, _, rep_icon) in entries {
                    if rmode != REPEAT_CONSECUTIVE {
                        rep_icon.set_icon_name(Some(REPEAT_ICONS[rmode as usize]));
                        rep_icon.set_visible(true);
                    } else {
                        rep_icon.set_visible(false);
                    }
                }
            }
        }
    }

    fn on_volume_changed(&self, val: f64) {
        self.player.borrow().as_ref().unwrap().set_volume(val);
        let icon = if val == 0.0 { "fp-speaker-0-symbolic" }
        else if val < 0.33 { "fp-speaker-1-symbolic" }
        else if val < 0.66 { "fp-speaker-2-symbolic" }
        else { "fp-speaker-3-symbolic" };
        self.vol_btn.borrow().as_ref().unwrap().set_icon_name(icon);
    }

    fn on_seek(&self, value: f64) {
        self.last_seek_time.set(glib::monotonic_time());
        let upper = self.seek_scale.borrow().as_ref().unwrap().adjustment().upper();
        if upper > 0.0 {
            self.player.borrow().as_ref().unwrap().seek(value.clamp(0.0, upper));
        }
    }

    fn set_playlist(&self, playlist: Vec<String>) {
        let count = playlist.len();
        *self.playlist.borrow_mut() = playlist;
        if count > 0 {
            let label = ngettext("{n} song", "{n} songs", count as u32)
                .replace("{n}", &count.to_string());
            self.song_count_label.borrow().as_ref().unwrap().set_label(&label);
        } else {
            self.song_count_label.borrow().as_ref().unwrap().set_label("");
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Free functions (for background threads)
// ────────────────────────────────────────────────────────────────────

/// Build FileItem list from DB data for the given folder.
/// Returns (items, has_unscanned_songs) — no disk-scan, no GStreamer.
fn build_folder_items(db: &Arc<LibraryDB>, path: &str) -> (Vec<FileItem>, bool) {
    let hide_bb = gio::Settings::new(config::APP_ID).boolean("anti-bad-bunny");
    let db_folders = db.get_folder_children(path);
    let db_songs = db.get_folder_songs(path);

    let mut items = Vec::with_capacity(db_folders.len() + db_songs.len());
    // Show sub-folders that contain audio anywhere in their subtree (has_audio=true)
    for sf in &db_folders {
        if hide_bb && is_bad_bunny(&sf.name) { continue; }
        if !sf.has_audio { continue; }
        let fi = FileItem::new(&sf.path, &sf.name, true);
        fi.set_cover_thumb(get_folder_preview(db, &sf.path, 0));
        items.push(fi);
    }

    let folder_cover = get_folder_cover(db, path);
    let song_covers = db.get_song_covers_for_folder(path);

    for rec in &db_songs {
        if hide_bb && is_bad_bunny(&rec.artist) { continue; }
        let item = FileItem::new(&rec.path, &rec.name, false);
        let title_str = if rec.title.is_empty() {
            Path::new(&rec.name).file_stem().unwrap_or_default().to_string_lossy().to_string()
        } else {
            rec.title.clone()
        };
        item.set_title(&title_str);
        item.set_artist(&rec.artist);
        item.set_album(&rec.album);
        item.set_year(&rec.year);
        item.set_format_type(&rec.format_type);
        item.set_bitrate(rec.bitrate);
        item.set_sample_rate(rec.sample_rate);
        item.set_bits_per_sample(rec.bits_per_sample);
        item.set_duration(rec.duration);

        if let Some(data) = song_covers.get(&rec.path) {
            if let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(data)) {
                item.set_cover_thumb(Some(tex));
            } else {
                item.set_cover_thumb(folder_cover.clone());
            }
        } else {
            item.set_cover_thumb(folder_cover.clone());
        }
        items.push(item);
    }
    let has_unscanned = db_songs.iter().any(|s| !s.meta_scanned);
    (items, has_unscanned)
}

/// Scan, index, and enrich `folders`.
/// If `report_progress` is true the scan phase reports song counts to the UI,
/// discovers covers, then calls `load_all_folders` (so root shows with covers).
fn deep_scan_and_index(
    db: &Arc<LibraryDB>,
    folders: &[String],
    shutdown: &Arc<AtomicBool>,
    obj_weak: glib::SendWeakRef<super::FolderplayWindow>,
    report_progress: bool,
) {
    // ── Phase 1: scan ────────────────────────────────────────────
    for folder in folders {
        if shutdown.load(Ordering::Relaxed) { return; }
        if report_progress {
            let obj_weak2 = obj_weak.clone();
            db.scan_folder_deep_with_progress(folder, None, move |count| {
                let w = obj_weak2.clone();
                glib::idle_add_once(move || {
                    if let Some(obj) = w.upgrade() {
                        obj.imp().update_scan_progress(count);
                    }
                });
            });
        } else {
            db.scan_folder_deep(folder, None);
        }
    }

    if shutdown.load(Ordering::Relaxed) { return; }

    // ── Phase 2: cover discovery ─────────────────────────────────
    // Update label before starting covers (only for new-folder flow)
    if report_progress {
        let obj_weak2 = obj_weak.clone();
        glib::idle_add_once(move || {
            if let Some(obj) = obj_weak2.upgrade() {
                obj.imp().update_scan_covers_label();
            }
        });
    }

    for folder in folders {
        if shutdown.load(Ordering::Relaxed) { return; }
        discover_folder_cover(db, folder);
        for child in &db.get_folder_children(folder) {
            if shutdown.load(Ordering::Relaxed) { return; }
            discover_folder_cover(db, &child.path);
        }
    }

    // ── After covers: show root view ──────────────────────────────
    if report_progress {
        let obj_weak2 = obj_weak.clone();
        glib::idle_add_once(move || {
            if let Some(obj) = obj_weak2.upgrade() {
                obj.imp().finish_scan_and_reload();
            }
        });
    }

    if shutdown.load(Ordering::Relaxed) { return; }

    // ── Phase 3: playlist + metadata enrichment (always in bg) ───
    let excl = {
        let settings = gio::Settings::new(config::APP_ID);
        if settings.boolean("anti-bad-bunny") { Some("bad bunny") } else { None }
    };
    let playlist = db.get_playlist(folders, excl);

    let obj_weak2 = obj_weak.clone();
    glib::idle_add_once(move || {
        if let Some(obj) = obj_weak2.upgrade() {
            obj.imp().set_playlist(playlist);
        }
    });

    enrich_all_metadata(db, folders, shutdown);
}

fn enrich_all_metadata(db: &Arc<LibraryDB>, folders: &[String], shutdown: &Arc<AtomicBool>) {
    let Ok(discoverer) = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(3)) else { return };
    let folders = folders.to_vec();
    loop {
        if shutdown.load(Ordering::Relaxed) { return; }
        let batch = db.get_unscanned_songs(&folders, 50);
        if batch.is_empty() { break; }
        for song_path in &batch {
            enrich_one_song(db, &discoverer, song_path);
        }
    }
}

fn enrich_folder_metadata(db: &Arc<LibraryDB>, folder_path: &str) {
    // Discover cover art for this folder (from disk files or embedded)
    discover_folder_cover(db, folder_path);

    let unscanned = db.get_unscanned_songs(&[folder_path.to_string()], 100);
    if unscanned.is_empty() { return; }
    let Ok(discoverer) = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(3)) else { return };
    for song_path in &unscanned {
        enrich_one_song(db, &discoverer, song_path);
    }
}

fn enrich_one_song(db: &Arc<LibraryDB>, discoverer: &gst_pbutils::Discoverer, song_path: &str) {
    let Ok(uri) = glib::filename_to_uri(song_path, None) else { return };
    let Ok(info) = discoverer.discover_uri(&uri) else { return };

    let dur = info.duration().map(|d| d.nseconds() as f64 / 1_000_000_000.0).unwrap_or(0.0);
    let (mut title, mut artist, mut album, mut year) = (String::new(), String::new(), String::new(), String::new());
    let (mut bitrate, mut sample_rate, mut bits_per_sample) = (0i32, 0i32, 0i32);

    let streams = info.audio_streams();
    if let Some(a) = streams.first() {
        let a = a.clone().downcast::<gst_pbutils::DiscovererAudioInfo>().unwrap();
        bitrate = (a.bitrate() / 1000) as i32;
        sample_rate = a.sample_rate() as i32;
        bits_per_sample = a.depth() as i32;
    }

    if let Some(tags) = info.tags() {
        if let Some(v) = tags.get::<gst::tags::Title>() { title = v.get().to_string(); }
        if let Some(v) = tags.get::<gst::tags::Artist>() { artist = v.get().to_string(); }
        if let Some(v) = tags.get::<gst::tags::Album>() { album = v.get().to_string(); }
        if let Some(v) = tags.get::<gst::tags::DateTime>() { year = v.get().year().to_string(); }
        else if let Some(v) = tags.get::<gst::tags::Date>() { year = v.get().year().to_string(); }

        // Cover art
        let folder = Path::new(song_path).parent().unwrap_or(Path::new("")).to_string_lossy().into_owned();
        let sample = tags.index::<gst::tags::Image>(0)
            .or_else(|| tags.index::<gst::tags::PreviewImage>(0));
        if let Some(sample_val) = sample {
            let sample: gst::Sample = sample_val.get();
            if let Some(buf) = sample.buffer() {
                if let Ok(map) = buf.map_readable() {
                    let data: Vec<u8> = map.as_slice().to_vec();
                    db.set_song_cover(song_path, &data);
                    if !db.has_cover(&folder) {
                        db.set_cover(&folder, &data, "embedded");
                    }
                }
            }
        }
    }

    if title.is_empty() {
        title = Path::new(song_path).file_stem().unwrap_or_default().to_string_lossy().into_owned();
    }
    db.update_song_metadata(song_path, &title, &artist, &album, &year, bitrate, sample_rate, bits_per_sample, dur);
}

/// Fast, DB-only cover lookup.  Never touches disk or GStreamer.
/// Returns `Some(texture)` if the cover is cached, `None` otherwise.
fn get_folder_preview(db: &Arc<LibraryDB>, path: &str, _depth: u32) -> Option<gdk::Texture> {
    if let Some(data) = db.get_cover(path) {
        if let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(&data)) {
            return Some(tex);
        }
    }
    None
}

fn get_folder_cover(db: &Arc<LibraryDB>, path: &str) -> Option<gdk::Texture> {
    // DB-only: just return what we already cached
    if let Some(data) = db.get_cover(path) {
        if let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from(&data)) {
            return Some(tex);
        }
    }
    None
}

/// Expensive cover discovery: reads disk + GStreamer.  Called only from
/// background enrichment, never from the UI-path.
fn discover_folder_cover(db: &Arc<LibraryDB>, path: &str) {
    // Already have a cover (real data or NULL sentinel)?
    if db.has_cover(path) { return; }

    if let Some(data) = discover_cover_recursive(db, path, 0) {
        db.set_cover(path, &data, "discovered");
    } else {
        db.mark_no_cover(path);
    }
}

/// Recursively search for cover art: cover image files → embedded art in
/// first audio file → descend into subdirectories.  Returns raw bytes.
fn discover_cover_recursive(db: &Arc<LibraryDB>, path: &str, depth: u32) -> Option<Vec<u8>> {
    if depth > 5 { return None; }

    // 1. Cover image files on disk
    for name in COVER_NAMES {
        let p = Path::new(path).join(name);
        if p.is_file() {
            if let Ok(data) = std::fs::read(&p) {
                return Some(data);
            }
        }
    }

    // 2. Check DB for any song cover already discovered in this folder
    if let Some(first_cover) = db.get_first_song_cover_in_folder(path) {
        return Some(first_cover);
    }

    // 3. Embedded art from the first audio file (GStreamer)
    let mut subdirs = Vec::new();
    if let Ok(rd) = std::fs::read_dir(path) {
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|a| a.file_name());
        for e in &entries {
            let ft = match e.file_type() { Ok(t) => t, Err(_) => continue };
            if e.file_name().to_string_lossy().starts_with('.') { continue; }
            if ft.is_dir() {
                subdirs.push(e.path());
            } else if ft.is_file() {
                if let Some(ext) = e.path().extension() {
                    if !library_db::is_audio_ext(&ext.to_string_lossy()) { continue; }
                    if let Ok(discoverer) = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(3)) {
                        if let Ok(uri) = glib::filename_to_uri(&*e.path().to_string_lossy(), None) {
                            if let Ok(info) = discoverer.discover_uri(&uri) {
                                if let Some(tags) = info.tags() {
                                    let sample = tags.index::<gst::tags::Image>(0)
                                        .or_else(|| tags.index::<gst::tags::PreviewImage>(0));
                                    if let Some(sample_val) = sample {
                                        let sample: gst::Sample = sample_val.get();
                                        if let Some(buf) = sample.buffer() {
                                            if let Ok(map) = buf.map_readable() {
                                                return Some(map.as_slice().to_vec());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    // Only try the first audio file per folder
                    break;
                }
            }
        }
    }

    // 4. Recurse into subdirectories
    for sub in &subdirs {
        if let Some(data) = discover_cover_recursive(db, &sub.to_string_lossy(), depth + 1) {
            return Some(data);
        }
    }

    None
}

fn get_cover_file(folder: &str) -> Option<String> {
    for name in COVER_NAMES {
        let p = Path::new(folder).join(name);
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

// ── Palette extraction (median-cut) ────────────────────────────────
fn extract_palette(texture: &gdk::Texture, n_colors: usize) -> Option<Vec<(u8, u8, u8)>> {
    let w = texture.width() as usize;
    let h = texture.height() as usize;
    if w == 0 || h == 0 { return None; }

    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let data: Vec<u8> = bytes[..].to_vec();

    let step_x = (w / 64).max(1);
    let step_y = (h / 64).max(1);

    let mut pixels = Vec::new();
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let off = y * stride + x * 4;
            if off + 2 < data.len() {
                pixels.push((data[off], data[off + 1], data[off + 2]));
            }
            x += step_x;
        }
        y += step_y;
    }

    median_cut(pixels, n_colors)
}

fn median_cut(pixels: Vec<(u8, u8, u8)>, n_colors: usize) -> Option<Vec<(u8, u8, u8)>> {
    if pixels.is_empty() { return None; }
    let mut buckets = vec![pixels];
    while buckets.len() < n_colors {
        let mut best_range = 0u8;
        let mut best_idx = 0;
        let mut best_ch = 0usize;
        for (i, bkt) in buckets.iter().enumerate() {
            for ch in 0..3 {
                let lo = bkt.iter().map(|p| match ch { 0 => p.0, 1 => p.1, _ => p.2 }).min().unwrap_or(0);
                let hi = bkt.iter().map(|p| match ch { 0 => p.0, 1 => p.1, _ => p.2 }).max().unwrap_or(0);
                let span = hi - lo;
                if span > best_range {
                    best_range = span;
                    best_idx = i;
                    best_ch = ch;
                }
            }
        }
        let mut bkt = buckets.remove(best_idx);
        bkt.sort_by_key(|p| match best_ch { 0 => p.0, 1 => p.1, _ => p.2 });
        let mid = bkt.len() / 2;
        let right = bkt.split_off(mid);
        buckets.push(bkt);
        buckets.push(right);
    }

    let mut colors = Vec::new();
    for bkt in &buckets {
        if bkt.is_empty() { continue; }
        let r = bkt.iter().map(|p| p.0 as u32).sum::<u32>() / bkt.len() as u32;
        let g = bkt.iter().map(|p| p.1 as u32).sum::<u32>() / bkt.len() as u32;
        let b = bkt.iter().map(|p| p.2 as u32).sum::<u32>() / bkt.len() as u32;
        colors.push((r as u8, g as u8, b as u8));
    }
    Some(colors)
}
