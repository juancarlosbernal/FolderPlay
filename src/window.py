# Copyright (c) 2026 Juan Carlos Bernal
#
# SPDX-License-Identifier: GPL-3.0-or-later

import os
import re
import threading
from gettext import gettext as _, ngettext

import gi

gi.require_version('GstPbutils', '1.0')

from gi.repository import (
    Adw, Gtk, Gio, GLib, Gdk,
    GObject, Graphene, Gst, GstPbutils, Pango,
)

from .player import AudioPlayer
from .library_db import LibraryDB

PLAYER_WIDTH = 450
THUMB_SIZE = 44
GRID_TILE_SIZE = 150
AUDIO_EXTENSIONS = {
    '.mp3', '.flac', '.ogg', '.opus', '.wav', '.aac', '.m4a',
    '.wma', '.aiff', '.aif', '.ape', '.wv', '.mka', '.oga',
    '.dsf', '.dff',
}
LOSSLESS_FORMATS = {'FLAC', 'WAV', 'AIFF', 'AIF', 'APE', 'WV'}

# Repeat modes
REPEAT_CONSECUTIVE = 0
REPEAT_ONCE = 1
REPEAT_LOOP = 2
REPEAT_ICONS = [
    'playlist-consecutive-symbolic',
    'playlist-repeat-song-symbolic',
    'playlist-repeat-symbolic',
]
COVER_NAMES = [
    'cover.jpg', 'cover.png', 'Cover.jpg', 'Cover.png',
    'folder.jpg', 'folder.png', 'Folder.jpg', 'Folder.png',
    'front.jpg', 'front.png', 'Front.jpg', 'Front.png',
    'album.jpg', 'album.png', 'Album.jpg', 'Album.png',
    'art.jpg', 'art.png',
]

_display_names = {}
_PORTAL_RE = re.compile(r'/run/user/\d+/doc/([a-zA-Z0-9]+)(?:/(.*)|$)')
_portal_proxy = None


def _get_portal_proxy():
    """Lazily create a D-Bus proxy for the Documents portal."""
    global _portal_proxy
    if _portal_proxy is None:
        try:
            _portal_proxy = Gio.DBusProxy.new_for_bus_sync(
                Gio.BusType.SESSION, Gio.DBusProxyFlags.NONE, None,
                'org.freedesktop.portal.Documents',
                '/org/freedesktop/portal/documents',
                'org.freedesktop.portal.Documents',
                None,
            )
        except Exception:
            _portal_proxy = False  # mark as failed
    return _portal_proxy if _portal_proxy else None


def _resolve_portal_path(path):
    """Resolve a /run/user/.../doc/ID/... path to the host path."""
    m = _PORTAL_RE.match(path)
    if not m:
        return None
    doc_id = m.group(1)

    # Try D-Bus directly first (works outside sandbox)
    proxy = _get_portal_proxy()
    if proxy:
        try:
            result = proxy.call_sync(
                'Info',
                GLib.Variant('(s)', (doc_id,)),
                Gio.DBusCallFlags.NONE, 1000, None,
            )
            data = result.unpack()
            return bytes(data[0]).rstrip(b'\0').decode('utf-8')
        except Exception:
            pass

    # Fallback: flatpak-spawn --host to query from outside the sandbox
    try:
        import subprocess
        dbus_addr = os.environ.get(
            'DBUS_SESSION_BUS_ADDRESS',
            f'unix:path=/run/user/{os.getuid()}/bus',
        )
        script = (
            'from gi.repository import Gio,GLib;'
            'p=Gio.DBusProxy.new_for_bus_sync(Gio.BusType.SESSION,0,None,'
            '"org.freedesktop.portal.Documents",'
            '"/org/freedesktop/portal/documents",'
            '"org.freedesktop.portal.Documents",None);'
            'r=p.call_sync("Info",GLib.Variant("(s)",("' + doc_id + '",)),0,3000,None);'
            'print(bytes(r.unpack()[0]).rstrip(b"\\x00").decode())'
        )
        result = subprocess.run(
            ['flatpak-spawn', '--host',
             f'--env=DBUS_SESSION_BUS_ADDRESS={dbus_addr}',
             'python3', '-c', script],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    except Exception:
        pass

    return None


def _display_path(path):
    """Return human-readable path, resolving portal paths via D-Bus."""
    cached = _display_names.get(path)
    # Only use cache if it's a real resolved path, not a portal path
    if cached and not cached.startswith('/run/user/'):
        return cached

    # Try D-Bus portal resolution
    host = _resolve_portal_path(path)
    if host:
        home = os.path.expanduser('~')
        if host.startswith(home):
            host = '~' + host[len(home):]
        _display_names[path] = host
        return host

    # Shorten home prefix for non-portal paths
    home = os.path.expanduser('~')
    if path.startswith(home):
        display = '~' + path[len(home):]
        _display_names[path] = display
        return display
    return path


def _load_display_names():
    """Load stored internal→display path mappings from GSettings."""
    settings = Gio.Settings.new('org.gnome.folderplay')
    pairs = settings.get_strv('folder-display-names')
    dirty = False
    for i in range(0, len(pairs) - 1, 2):
        internal, display = pairs[i], pairs[i + 1]
        # Discard stale entries where display is still a portal path
        if display.startswith('/run/user/'):
            dirty = True
            continue
        _display_names[internal] = display
    if dirty:
        # Persist cleaned-up pairs
        clean = []
        for k, v in _display_names.items():
            clean.extend([k, v])
        settings.set_strv('folder-display-names', clean)


def _save_display_name(internal_path, display_name):
    """Save an internal→display mapping to GSettings."""
    _display_names[internal_path] = display_name
    settings = Gio.Settings.new('org.gnome.folderplay')
    pairs = []
    for k, v in _display_names.items():
        pairs.extend([k, v])
    settings.set_strv('folder-display-names', pairs)


class FileItem(GObject.Object):
    __gtype_name__ = 'FileItem'

    def __init__(self, path, name, is_folder=False):
        super().__init__()
        self.path = path
        self.name = name
        self.is_folder = is_folder
        self.title = ''
        self.artist = ''
        self.album = ''
        self.year = ''
        self.format_type = ''
        self.bitrate = 0
        self.sample_rate = 0
        self.bits_per_sample = 0
        self.duration = 0.0
        self.cover_thumb = None

        if not is_folder:
            ext = os.path.splitext(name)[1].lower()
            self.format_type = ext[1:].upper()
            self.title = os.path.splitext(name)[0]


class CoverPicture(Gtk.Widget):
    """Fixed-size cover art widget (à la Amberol CoverPicture)."""
    __gtype_name__ = 'CoverPicture'

    COVER_SIZE = 300

    def __init__(self):
        super().__init__()
        self._paintable = None
        self.set_overflow(Gtk.Overflow.HIDDEN)

    def set_paintable(self, paintable):
        self._paintable = paintable
        self.queue_draw()

    def do_measure(self, orientation, for_size):
        return (self.COVER_SIZE, self.COVER_SIZE, -1, -1)

    def do_get_request_mode(self):
        return Gtk.SizeRequestMode.CONSTANT_SIZE

    def do_snapshot(self, snapshot):
        if not self._paintable:
            return
        w = self.get_width()
        h = self.get_height()
        if w <= 0 or h <= 0:
            return
        iw = self._paintable.get_intrinsic_width() or w
        ih = self._paintable.get_intrinsic_height() or h
        scale = max(w / iw, h / ih)
        sw = iw * scale
        sh = ih * scale
        x = (w - sw) / 2
        y = (h - sh) / 2
        rect = Graphene.Rect.alloc()
        rect.init(0, 0, w, h)
        point = Graphene.Point.alloc()
        point.init(x, y)
        snapshot.save()
        snapshot.push_clip(rect)
        snapshot.translate(point)
        self._paintable.snapshot(snapshot, sw, sh)
        snapshot.pop()
        snapshot.restore()


class GridCover(Gtk.Widget):
    """Fixed 150×150 cover widget for grid tiles, clips non-square art."""
    __gtype_name__ = 'GridCover'

    def __init__(self):
        super().__init__()
        self._paintable = None
        self.set_overflow(Gtk.Overflow.HIDDEN)

    def set_paintable(self, paintable):
        self._paintable = paintable
        self.queue_draw()

    def do_measure(self, orientation, for_size):
        return (GRID_TILE_SIZE, GRID_TILE_SIZE, -1, -1)

    def do_get_request_mode(self):
        return Gtk.SizeRequestMode.CONSTANT_SIZE

    def do_snapshot(self, snapshot):
        if not self._paintable:
            return
        w = self.get_width()
        h = self.get_height()
        if w <= 0 or h <= 0:
            return
        iw = self._paintable.get_intrinsic_width() or w
        ih = self._paintable.get_intrinsic_height() or h
        scale = max(w / iw, h / ih)
        sw = iw * scale
        sh = ih * scale
        x = (w - sw) / 2
        y = (h - sh) / 2
        rect = Graphene.Rect.alloc()
        rect.init(0, 0, w, h)
        point = Graphene.Point.alloc()
        point.init(x, y)
        snapshot.save()
        snapshot.push_clip(rect)
        snapshot.translate(point)
        self._paintable.snapshot(snapshot, sw, sh)
        snapshot.pop()
        snapshot.restore()


@Gtk.Template(resource_path='/org/gnome/folderplay/window.ui')
class FolderplayWindow(Adw.ApplicationWindow):
    __gtype_name__ = 'FolderplayWindow'

    main_box = Gtk.Template.Child()

    def __init__(self, **kwargs):
        super().__init__(**kwargs)

        self._player = AudioPlayer()
        self._db = LibraryDB()
        self._shutdown = threading.Event()
        self._playlist = []
        self._current_index = -1
        self._cover_texture = None
        self._last_seek_time = 0

        self._nav_stack = []
        self._current_folder = None
        self._root_folder = None
        self._folder_cover_cache = {}
        self._folder_preview_cache = {}
        self._bound_rows = {}  # path → [(row, playing_icon), ...]
        self._list_store = Gio.ListStore.new(FileItem)
        self._current_items = []
        self._repeat_mode = REPEAT_CONSECUTIVE
        self._browse_visible = True
        self._browse_manual_closed = False
        self._auto_hide_busy = False
        self._current_tags = {}
        self._pending_scroll_path = None
        self._playing_path = None
        self._external_file = False

        self._apply_audio_output()
        self._setup_dynamic_css()
        self._build_ui()
        self._setup_actions()
        self._connect_signals()

        _load_display_names()

        settings = Gio.Settings.new('org.gnome.folderplay')
        folders = list(settings.get_strv('music-folders'))
        # Migrate legacy single-folder key
        legacy = settings.get_string('music-folder')
        if legacy and os.path.isdir(legacy):
            if legacy not in folders:
                folders.insert(0, legacy)
                settings.set_strv('music-folders', folders)
            settings.set_string('music-folder', '')
        # Load folders
        folders = [f for f in folders if os.path.isdir(f)]
        if folders:
            settings.set_strv('music-folders', folders)
            self._load_all_folders()

    # ── CSS providers ───────────────────────────────────────────────

    def _setup_dynamic_css(self):
        css = Gtk.CssProvider()
        css.load_from_resource('/org/gnome/folderplay/style.css')
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), css,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )
        self._dynamic_css = Gtk.CssProvider()
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), self._dynamic_css,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        )

    def _apply_audio_output(self):
        settings = Gio.Settings.new('org.gnome.folderplay')
        self._player.set_audio_output(settings.get_string('audio-output'))

    # ── Build the entire UI ─────────────────────────────────────────

    def _build_ui(self):
        # Apply gradient background to the main container (Amberol-style)
        self.main_box.add_css_class('album-bg')
        content = self.main_box

        # ── Browse panel (left — expands, inside a Revealer for animation) ──
        self._browse_revealer = Gtk.Revealer()
        self._browse_revealer.set_reveal_child(True)
        self._browse_revealer.set_transition_type(
            Gtk.RevealerTransitionType.SLIDE_RIGHT,
        )
        self._browse_revealer.set_transition_duration(250)
        self._browse_revealer.set_hexpand(True)
        content.append(self._browse_revealer)

        browse = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, hexpand=True)
        browse.add_css_class('browse-panel')
        self._browse_revealer.set_child(browse)

        # Browse toolbar row (search … ) — draggable
        browse_handle = Gtk.WindowHandle()
        browse_bar = Gtk.Box(spacing=4, margin_start=8, margin_end=8,
                             margin_top=6, margin_bottom=2)

        self._search_btn = Gtk.ToggleButton(icon_name='edit-find-symbolic')
        self._search_btn.set_tooltip_text(_('Search'))
        self._search_btn.add_css_class('flat')
        self._search_btn.add_css_class('circular')
        browse_bar.append(self._search_btn)

        self._home_btn = Gtk.Button(
            icon_name='folder-open-symbolic',
        )
        self._home_btn.set_tooltip_text(_('Go to root folder'))
        self._home_btn.add_css_class('flat')
        self._home_btn.add_css_class('circular')
        browse_bar.append(self._home_btn)

        spacer = Gtk.Box(hexpand=True)
        browse_bar.append(spacer)

        self._song_count_label = Gtk.Label(label='')
        self._song_count_label.add_css_class('dim-label')
        self._song_count_label.add_css_class('caption')
        self._song_count_label.set_margin_end(6)
        browse_bar.append(self._song_count_label)

        self._grid_toggle = Gtk.ToggleButton(
            icon_name='grid-filled-symbolic',
        )
        self._grid_toggle.set_tooltip_text(_('Grid view'))
        self._grid_toggle.add_css_class('flat')
        self._grid_toggle.add_css_class('circular')
        self._grid_toggle.set_active(True)
        self._grid_toggle.connect('toggled', self._on_view_toggled)
        browse_bar.append(self._grid_toggle)

        browse_handle.set_child(browse_bar)
        browse.append(browse_handle)

        # Search revealer
        self._search_revealer = Gtk.Revealer()
        self._search_revealer.set_transition_type(
            Gtk.RevealerTransitionType.SLIDE_DOWN,
        )
        self._search_entry = Gtk.SearchEntry()
        self._search_entry.set_placeholder_text(_('Search songs, artist or album…'))
        self._search_entry.set_margin_start(12)
        self._search_entry.set_margin_end(12)
        self._search_entry.set_margin_top(6)
        self._search_entry.set_margin_bottom(6)
        self._search_revealer.set_child(self._search_entry)
        browse.append(self._search_revealer)

        self._search_btn.bind_property(
            'active', self._search_revealer, 'reveal-child',
            GObject.BindingFlags.SYNC_CREATE,
        )

        # Navigation bar
        nav = Gtk.Box(spacing=6, margin_start=12, margin_end=12,
                      margin_top=8, margin_bottom=4)
        self._back_btn = Gtk.Button(icon_name='arrow3-left-symbolic')
        self._back_btn.add_css_class('flat')
        self._back_btn.add_css_class('circular')
        self._back_btn.set_visible(False)
        nav.append(self._back_btn)

        self._folder_label = Gtk.Label(label=_('Folders'))
        self._folder_label.add_css_class('title-4')
        self._folder_label.set_ellipsize(Pango.EllipsizeMode.END)
        self._folder_label.set_xalign(0)
        self._folder_label.set_hexpand(True)
        nav.append(self._folder_label)
        browse.append(nav)

        # Browse stack: empty / loading / list
        self._browse_stack = Gtk.Stack()
        self._browse_stack.set_transition_type(
            Gtk.StackTransitionType.CROSSFADE,
        )
        self._browse_stack.set_vexpand(True)

        empty = Adw.StatusPage()
        empty.set_icon_name('folder-music-symbolic')
        empty.set_title(_('No Music Folder'))
        empty.set_description(_('Select a folder to start playing'))
        add_btn = Gtk.Button(label=_('Add Music Folder'))
        add_btn.set_halign(Gtk.Align.CENTER)
        add_btn.add_css_class('pill')
        add_btn.add_css_class('suggested-action')
        add_btn.connect('clicked', lambda b: self._on_open_folder())
        empty.set_child(add_btn)
        self._browse_stack.add_named(empty, 'empty')

        search_empty = Adw.StatusPage()
        search_empty.set_icon_name('edit-find-symbolic')
        search_empty.set_title(_('No results found'))
        search_empty.set_description(_('Try another title, artist, or album'))
        self._browse_stack.add_named(search_empty, 'search-empty')

        spinner_box = Gtk.Box(halign=Gtk.Align.CENTER,
                              valign=Gtk.Align.CENTER)
        spinner = Gtk.Spinner(spinning=True)
        spinner.set_size_request(32, 32)
        spinner_box.append(spinner)
        self._browse_stack.add_named(spinner_box, 'loading')

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_vexpand(True)
        self._list_view = Gtk.ListView()
        self._list_view.set_single_click_activate(True)
        self._list_view.add_css_class('browse-list')
        scroll.set_child(self._list_view)
        self._browse_stack.add_named(scroll, 'list')

        # Grid view (GridView) for folder covers
        grid_scroll = Gtk.ScrolledWindow()
        grid_scroll.set_policy(Gtk.PolicyType.NEVER,
                               Gtk.PolicyType.AUTOMATIC)
        grid_scroll.set_vexpand(True)
        self._grid_store = Gio.ListStore.new(FileItem)
        grid_factory = Gtk.SignalListItemFactory()
        grid_factory.connect('setup', self._on_grid_setup)
        grid_factory.connect('bind', self._on_grid_bind)
        grid_sel = Gtk.SingleSelection.new(self._grid_store)
        grid_sel.set_autoselect(False)
        self._grid_view = Gtk.GridView()
        self._grid_view.set_model(grid_sel)
        self._grid_view.set_factory(grid_factory)
        self._grid_view.set_single_click_activate(True)
        self._grid_view.set_max_columns(50)
        self._grid_view.set_min_columns(1)
        self._grid_view.add_css_class('browse-grid')
        self._grid_view.connect('activate', self._on_grid_activated)
        grid_scroll.set_child(self._grid_view)
        self._browse_stack.add_named(grid_scroll, 'grid')

        browse.append(self._browse_stack)

        # ── Thin vertical separator (inside revealer too) ──
        self._sep = Gtk.Separator(orientation=Gtk.Orientation.VERTICAL)
        self._sep.add_css_class('thin-separator')
        content.append(self._sep)

        # ── Player panel (right — fixed width, expands when browse hidden) ──
        self._player_panel = Gtk.Box(
            orientation=Gtk.Orientation.VERTICAL, spacing=12,
        )
        self._player_panel.set_size_request(PLAYER_WIDTH, -1)
        self._player_panel.set_hexpand(False)
        self._player_panel.add_css_class('player-panel')
        self._build_player(self._player_panel)
        content.append(self._player_panel)

        self._setup_list_view()

    # ── Player panel widgets ────────────────────────────────────────

    def _build_player(self, panel):
        # Player top bar: dock-left (left) + window controls (right) — draggable
        player_handle = Gtk.WindowHandle()
        player_top = Gtk.Box(spacing=4, margin_start=8, margin_end=8,
                             margin_top=6)
        self._dock_btn = Gtk.Button(icon_name='sidebar-show-symbolic')
        self._dock_btn.add_css_class('flat')
        self._dock_btn.add_css_class('circular')
        self._dock_btn.set_valign(Gtk.Align.CENTER)
        self._dock_btn.set_tooltip_text(_('Toggle Browse Panel'))
        player_top.append(self._dock_btn)

        self._locate_btn = Gtk.Button(icon_name='playlist-symbolic')
        self._locate_btn.add_css_class('flat')
        self._locate_btn.add_css_class('circular')
        self._locate_btn.set_valign(Gtk.Align.CENTER)
        self._locate_btn.set_tooltip_text(_('Go to Playing Folder'))
        self._locate_btn.set_visible(False)
        player_top.append(self._locate_btn)

        # Spacer
        player_top.append(Gtk.Box(hexpand=True))

        # HiRes indicator (visible when playing ≥48kHz)
        self._hires_icon_player = Gtk.Picture.new_for_resource(
            '/org/gnome/folderplay/icons/scalable/actions/hires-22.png'
        )
        self._hires_icon_player.set_size_request(22, 22)
        self._hires_icon_player.set_can_shrink(False)
        self._hires_icon_player.set_halign(Gtk.Align.CENTER)
        self._hires_icon_player.set_valign(Gtk.Align.CENTER)
        self._hires_icon_player.set_visible(False)
        player_top.append(self._hires_icon_player)

        # Tag info button
        self._tag_btn = Gtk.Button(icon_name='tag-outline-symbolic')
        self._tag_btn.set_tooltip_text(_('Song Info'))
        self._tag_btn.add_css_class('flat')
        self._tag_btn.add_css_class('circular')
        self._tag_btn.set_valign(Gtk.Align.CENTER)
        self._tag_btn.set_visible(False)
        player_top.append(self._tag_btn)

        # Menu button (between hires and minimize)
        self._menu_btn = Gtk.MenuButton()
        self._menu_btn.set_icon_name('open-menu-symbolic')
        self._menu_btn.set_tooltip_text(_('Menu'))
        self._menu_btn.add_css_class('flat')
        self._menu_btn.add_css_class('circular')
        self._menu_btn.set_valign(Gtk.Align.CENTER)
        self._build_app_menu()
        player_top.append(self._menu_btn)

        # Window control buttons
        self._win_minimize = Gtk.Button(icon_name='window-minimize-symbolic')
        self._win_minimize.add_css_class('circular')
        self._win_minimize.add_css_class('windowcontrol-btn')
        self._win_minimize.set_valign(Gtk.Align.CENTER)
        self._win_minimize.set_tooltip_text(_('Minimize'))
        player_top.append(self._win_minimize)

        self._win_close = Gtk.Button(icon_name='window-close-symbolic')
        self._win_close.add_css_class('circular')
        self._win_close.add_css_class('windowcontrol-btn')
        self._win_close.set_valign(Gtk.Align.CENTER)
        self._win_close.set_tooltip_text(_('Close'))
        player_top.append(self._win_close)

        player_handle.set_child(player_top)
        panel.append(player_handle)

        # Cover art (fixed 300×300, à la Amberol CoverPicture)
        self._cover_stack = Gtk.Stack()
        self._cover_stack.set_transition_type(
            Gtk.StackTransitionType.CROSSFADE,
        )
        self._cover_stack.set_transition_duration(300)

        ph = Gtk.Box(halign=Gtk.Align.FILL, valign=Gtk.Align.FILL)
        ph.set_size_request(300, 300)
        ph.add_css_class('cover-placeholder')
        ph_icon = Gtk.Image.new_from_icon_name(
            'folder-music-symbolic'
        )
        ph_icon.set_pixel_size(64)
        ph_icon.set_hexpand(True)
        ph_icon.set_vexpand(True)
        ph_icon.set_halign(Gtk.Align.CENTER)
        ph_icon.set_valign(Gtk.Align.CENTER)
        ph.append(ph_icon)
        self._cover_stack.add_named(ph, 'placeholder')

        self._cover_picture = CoverPicture()
        self._cover_stack.add_named(self._cover_picture, 'art')

        wrap = Gtk.Box()
        wrap.set_overflow(Gtk.Overflow.HIDDEN)
        wrap.add_css_class('cover-container')
        wrap.set_halign(Gtk.Align.CENTER)
        wrap.set_valign(Gtk.Align.CENTER)
        wrap.append(self._cover_stack)

        cover_box = Gtk.Box(halign=Gtk.Align.CENTER,
                            valign=Gtk.Align.CENTER, vexpand=True)
        cover_box.set_margin_top(12)
        cover_box.append(wrap)
        panel.append(cover_box)

        # Song info
        info = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2,
                       halign=Gtk.Align.CENTER)
        self._title_label = Gtk.Label(label='FolderPlay')
        self._title_label.add_css_class('title-3')
        self._title_label.set_ellipsize(Pango.EllipsizeMode.END)
        self._title_label.set_max_width_chars(30)
        self._title_label.set_justify(Gtk.Justification.CENTER)
        info.append(self._title_label)

        self._subtitle_label = Gtk.Label(label=_('Select a song'))
        self._subtitle_label.add_css_class('dim-label')
        self._subtitle_label.set_ellipsize(Pango.EllipsizeMode.END)
        self._subtitle_label.set_max_width_chars(35)
        self._subtitle_label.set_justify(Gtk.Justification.CENTER)
        info.append(self._subtitle_label)

        # Format / bitrate line
        self._format_label = Gtk.Label(label='')
        self._format_label.add_css_class('caption')
        self._format_label.add_css_class('dim-label')
        self._format_label.set_justify(Gtk.Justification.CENTER)
        self._format_label.set_visible(False)
        info.append(self._format_label)

        panel.append(info)

        # Seek bar
        seek = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0,
                       margin_start=24, margin_end=24)
        self._seek_scale = Gtk.Scale.new_with_range(
            Gtk.Orientation.HORIZONTAL, 0, 1, 0.01,
        )
        self._seek_scale.set_draw_value(False)
        self._seek_scale.add_css_class('seek-slider')
        seek.append(self._seek_scale)

        times = Gtk.Box()
        self._pos_label = Gtk.Label(label='0:00')
        self._pos_label.add_css_class('caption')
        self._pos_label.add_css_class('dim-label')
        self._dur_label = Gtk.Label(label='0:00')
        self._dur_label.add_css_class('caption')
        self._dur_label.add_css_class('dim-label')
        times.append(self._pos_label)
        times.append(Gtk.Box(hexpand=True))
        times.append(self._dur_label)
        seek.append(times)
        panel.append(seek)

        # Transport
        transport = Gtk.Box(spacing=16, halign=Gtk.Align.CENTER,
                            margin_top=4, margin_bottom=24)

        # Repeat mode button
        self._repeat_btn = Gtk.Button(
            icon_name=REPEAT_ICONS[REPEAT_CONSECUTIVE],
        )
        self._repeat_btn.add_css_class('circular')
        self._repeat_btn.add_css_class('flat')
        self._repeat_btn.add_css_class('transport-btn')
        self._repeat_btn.set_valign(Gtk.Align.CENTER)
        self._repeat_btn.set_tooltip_text(_('Consecutive'))
        transport.append(self._repeat_btn)

        self._prev_btn = Gtk.Button(
            icon_name='media-skip-backward-symbolic',
        )
        self._prev_btn.add_css_class('circular')
        self._prev_btn.add_css_class('flat')
        self._prev_btn.add_css_class('transport-btn')
        self._prev_btn.set_valign(Gtk.Align.CENTER)

        self._play_btn = Gtk.Button(
            icon_name='media-playback-start-symbolic',
        )
        self._play_btn.add_css_class('circular')
        self._play_btn.add_css_class('play-button')
        self._play_btn.set_valign(Gtk.Align.CENTER)

        self._next_btn = Gtk.Button(
            icon_name='media-skip-forward-symbolic',
        )
        self._next_btn.add_css_class('circular')
        self._next_btn.add_css_class('flat')
        self._next_btn.add_css_class('transport-btn')
        self._next_btn.set_valign(Gtk.Align.CENTER)

        # Volume popover button
        self._vol_btn = Gtk.MenuButton()
        self._vol_btn.set_icon_name('speaker-2-symbolic')
        self._vol_btn.set_tooltip_text(_('Volume'))
        self._vol_btn.add_css_class('circular')
        self._vol_btn.add_css_class('flat')
        self._vol_btn.add_css_class('transport-btn')
        self._vol_btn.set_valign(Gtk.Align.CENTER)
        vol_popover = Gtk.Popover()
        vol_popover.add_css_class('volume-popover')
        self._vol_scale = Gtk.Scale.new_with_range(
            Gtk.Orientation.VERTICAL, 0, 1, 0.05,
        )
        self._vol_scale.set_inverted(True)
        self._vol_scale.set_value(0.7)
        self._vol_scale.set_draw_value(False)
        self._vol_scale.set_size_request(-1, 150)
        vol_popover.set_child(self._vol_scale)
        self._vol_btn.set_popover(vol_popover)

        transport.append(self._prev_btn)
        transport.append(self._play_btn)
        transport.append(self._next_btn)
        transport.append(self._vol_btn)
        panel.append(transport)

    def _build_app_menu(self):
        menu = Gio.Menu()

        folder_section = Gio.Menu()
        folder_section.append(_('Manage Folders…'), 'win.manage-folders')
        menu.append_section(None, folder_section)

        bunny_section = Gio.Menu()
        bunny_section.append(_('Anti Bad Bunny'), 'win.anti-bad-bunny')
        menu.append_section(None, bunny_section)

        app_section = Gio.Menu()
        app_section.append(_('Preferences'), 'app.preferences')
        app_section.append(_('Keyboard Shortcuts'), 'app.shortcuts')
        app_section.append(_('About FolderPlay'), 'app.about')
        menu.append_section(None, app_section)

        self._menu_btn.set_menu_model(menu)

    # ── View toggle ─────────────────────────────────────────────────

    def _on_view_toggled(self, btn):
        icon = 'view-list-symbolic' if btn.get_active() else 'grid-filled-symbolic'
        btn.set_icon_name(icon)
        tooltip = _('List view') if btn.get_active() else _('Grid view')
        btn.set_tooltip_text(tooltip)
        # Re-display the current items with the new mode
        if self._current_items:
            self._populate_list(self._current_items)

    def _has_folders_only(self, items):
        """True if items contain at least one folder and no audio files."""
        has_folder = False
        for item in items:
            if item.is_folder:
                has_folder = True
            else:
                return False
        return has_folder

    def _on_grid_setup(self, factory, list_item):
        # Outer container — centers the card, provides external spacing
        outer = Gtk.Box(halign=Gtk.Align.CENTER,
                        valign=Gtk.Align.CENTER)
        outer.set_margin_start(10)
        outer.set_margin_end(10)
        outer.set_margin_top(10)
        outer.set_margin_bottom(10)

        # Card — fixed size, receives hover highlight
        card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        card.set_size_request(GRID_TILE_SIZE, -1)
        card.set_halign(Gtk.Align.CENTER)
        card.set_valign(Gtk.Align.START)
        card.add_css_class('grid-card')

        # Cover stack — switches between GridCover and folder icon
        cover_stack = Gtk.Stack()
        cover_stack.set_halign(Gtk.Align.CENTER)
        cover_stack.add_css_class('grid-cover-box')

        cover = GridCover()
        cover.add_css_class('grid-cover-box')
        cover_stack.add_named(cover, 'cover')

        icon = Gtk.Image.new_from_icon_name('folder-music-symbolic')
        icon.set_pixel_size(64)
        icon.set_size_request(GRID_TILE_SIZE, GRID_TILE_SIZE)
        icon.set_valign(Gtk.Align.CENTER)
        icon.set_halign(Gtk.Align.CENTER)
        icon.add_css_class('grid-folder-icon')
        cover_stack.add_named(icon, 'icon')

        card.append(cover_stack)

        lbl = Gtk.Label()
        lbl.set_ellipsize(Pango.EllipsizeMode.END)
        lbl.set_wrap(True)
        lbl.set_wrap_mode(Pango.WrapMode.WORD_CHAR)
        lbl.set_lines(2)
        lbl.set_max_width_chars(1)
        lbl.set_xalign(0.5)
        lbl.set_halign(Gtk.Align.CENTER)
        lbl.add_css_class('grid-label')
        card.append(lbl)

        outer.append(card)
        list_item.set_child(outer)

    def _on_grid_bind(self, factory, list_item):
        item = list_item.get_item()
        outer = list_item.get_child()
        card = outer.get_first_child()
        cover_stack = card.get_first_child()
        cover = cover_stack.get_child_by_name('cover')
        lbl = cover_stack.get_next_sibling()

        if item.cover_thumb:
            cover.set_paintable(item.cover_thumb)
            cover_stack.set_visible_child_name('cover')
        else:
            cover_stack.set_visible_child_name('icon')

        name = item.name
        if len(name) > 20:
            name = name[:20] + '…'
        lbl.set_label(name)

    def _on_grid_activated(self, grid_view, position):
        item = self._grid_store.get_item(position)
        if item and item.is_folder:
            self._navigate_into(item.path, item.name)

    # ── List view factory ───────────────────────────────────────────

    def _setup_list_view(self):
        factory = Gtk.SignalListItemFactory()
        factory.connect('setup', self._on_list_setup)
        factory.connect('bind', self._on_list_bind)
        factory.connect('unbind', self._on_list_unbind)
        self._list_view.set_factory(factory)

        self._filter_model = Gtk.FilterListModel.new(
            self._list_store, None,
        )
        selection = Gtk.SingleSelection.new(self._filter_model)
        selection.set_autoselect(False)
        self._list_view.set_model(selection)
        self._list_view.connect('activate', self._on_list_activated)

    def _on_list_setup(self, factory, list_item):
        row = Gtk.Box(spacing=10, margin_top=6, margin_bottom=6,
                      margin_start=12, margin_end=12)

        thumb_stack = Gtk.Stack()
        thumb_stack.set_size_request(THUMB_SIZE, THUMB_SIZE)
        thumb_stack.set_halign(Gtk.Align.CENTER)
        thumb_stack.set_valign(Gtk.Align.CENTER)

        icon = Gtk.Image()
        icon.set_pixel_size(32)
        thumb_stack.add_named(icon, 'icon')

        thumb = Gtk.Picture()
        thumb.set_content_fit(Gtk.ContentFit.COVER)
        thumb.set_can_shrink(True)
        thumb.set_size_request(THUMB_SIZE, THUMB_SIZE)
        thumb.add_css_class('song-thumb')
        thumb_stack.add_named(thumb, 'thumb')
        row.append(thumb_stack)

        info_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL,
                           valign=Gtk.Align.CENTER, hexpand=True,
                           spacing=2)
        title_lbl = Gtk.Label(xalign=0)
        title_lbl.set_ellipsize(Pango.EllipsizeMode.END)
        title_lbl.add_css_class('song-title')
        info_col.append(title_lbl)

        artist_lbl = Gtk.Label(xalign=0)
        artist_lbl.set_ellipsize(Pango.EllipsizeMode.END)
        artist_lbl.add_css_class('dim-label')
        artist_lbl.add_css_class('caption')
        info_col.append(artist_lbl)
        row.append(info_col)

        playing_icon = Gtk.Image.new_from_icon_name(
            'media-playback-start-symbolic'
        )
        playing_icon.set_pixel_size(18)
        playing_icon.set_halign(Gtk.Align.CENTER)
        playing_icon.set_valign(Gtk.Align.CENTER)
        playing_icon.add_css_class('now-playing-icon')
        playing_icon.set_visible(False)
        row.append(playing_icon)

        hires_box = Gtk.Picture.new_for_resource(
            '/org/gnome/folderplay/icons/scalable/actions/hires-22.png'
        )
        hires_box.set_size_request(22, 22)
        hires_box.set_can_shrink(False)
        hires_box.set_halign(Gtk.Align.CENTER)
        hires_box.set_valign(Gtk.Align.CENTER)
        hires_box.set_visible(False)
        row.append(hires_box)

        meta_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL,
                           valign=Gtk.Align.CENTER, spacing=2)
        fmt_lbl = Gtk.Label()
        fmt_lbl.add_css_class('caption')
        fmt_lbl.add_css_class('format-badge')
        meta_col.append(fmt_lbl)

        quality_lbl = Gtk.Label()
        quality_lbl.add_css_class('caption')
        quality_lbl.add_css_class('dim-label')
        meta_col.append(quality_lbl)
        row.append(meta_col)

        dur_lbl = Gtk.Label()
        dur_lbl.add_css_class('caption')
        dur_lbl.add_css_class('dim-label')
        dur_lbl.set_halign(Gtk.Align.END)
        dur_lbl.set_valign(Gtk.Align.CENTER)
        row.append(dur_lbl)

        arrow = Gtk.Image.new_from_icon_name('arrow3-right-symbolic')
        arrow.set_opacity(0.4)
        arrow.set_valign(Gtk.Align.CENTER)
        row.append(arrow)

        list_item.set_child(row)

    def _on_list_bind(self, factory, list_item):
        row = list_item.get_child()
        item = list_item.get_item()

        c = []
        child = row.get_first_child()
        while child:
            c.append(child)
            child = child.get_next_sibling()
        (thumb_stack, info_col, playing_icon,
         hires_box, meta_col, dur_lbl, arrow) = c

        title_lbl = info_col.get_first_child()
        artist_lbl = title_lbl.get_next_sibling()
        fmt_lbl = meta_col.get_first_child()
        quality_lbl = fmt_lbl.get_next_sibling()

        # Track bound widgets for direct now-playing updates
        entry = (row, playing_icon)
        self._bound_rows.setdefault(item.path, []).append(entry)
        list_item._bound_path = item.path

        if item.is_folder:
            if item.cover_thumb:
                thumb = thumb_stack.get_child_by_name('thumb')
                thumb.set_paintable(item.cover_thumb)
                thumb_stack.set_visible_child_name('thumb')
            else:
                icon = thumb_stack.get_child_by_name('icon')
                icon.set_from_icon_name('folder-open-symbolic')
                thumb_stack.set_visible_child_name('icon')
            title_lbl.set_label(item.name)
            artist_lbl.set_visible(False)
            playing_icon.set_visible(False)
            hires_box.set_visible(False)
            meta_col.set_visible(False)
            dur_lbl.set_visible(False)
            arrow.set_visible(True)
            row.remove_css_class('now-playing-row')
        else:
            arrow.set_visible(False)

            if item.cover_thumb:
                thumb = thumb_stack.get_child_by_name('thumb')
                thumb.set_paintable(item.cover_thumb)
                thumb_stack.set_visible_child_name('thumb')
            else:
                icon = thumb_stack.get_child_by_name('icon')
                icon.set_from_icon_name('folder-music-symbolic')
                thumb_stack.set_visible_child_name('icon')

            # Now-playing indicator
            is_playing = (
                self._playlist
                and 0 <= self._current_index < len(self._playlist)
                and item.path == self._playlist[self._current_index]
            )
            playing_icon.set_visible(is_playing)
            if is_playing:
                row.add_css_class('now-playing-row')
            else:
                row.remove_css_class('now-playing-row')

            title_lbl.set_label(
                item.title or os.path.splitext(item.name)[0]
            )
            if item.artist:
                artist_lbl.set_label(item.artist)
                artist_lbl.set_visible(True)
            else:
                artist_lbl.set_visible(False)

            meta_col.set_visible(True)
            fmt_lbl.set_label(item.format_type)

            # HiRes badge: ≥48000 Hz
            hires_box.set_visible(item.sample_rate >= 48000)

            if item.format_type in ('DSF', 'DFF') and item.sample_rate:
                dsd_level = round(item.sample_rate * 8 / 44100)
                quality_lbl.set_label(f'DSD{dsd_level}')
            elif item.format_type in LOSSLESS_FORMATS and item.sample_rate:
                bits = item.bits_per_sample or 16
                sr = item.sample_rate / 1000
                quality_lbl.set_label(f'{bits}-bit/{sr:g}kHz')
            elif item.bitrate:
                quality_lbl.set_label(f'{item.bitrate} kbps')
            else:
                quality_lbl.set_label('')

            dur_lbl.set_visible(True)
            if item.duration > 0:
                dur_lbl.set_label(self._format_time(item.duration))
            else:
                dur_lbl.set_label('\u2014')

    def _on_list_unbind(self, factory, list_item):
        path = getattr(list_item, '_bound_path', None)
        if path and path in self._bound_rows:
            row = list_item.get_child()
            self._bound_rows[path] = [
                e for e in self._bound_rows[path] if e[0] is not row
            ]
            if not self._bound_rows[path]:
                del self._bound_rows[path]

    def _update_now_playing_widgets(self, old_path, new_path):
        """Toggle now-playing on old/new path widgets directly."""
        if old_path and old_path in self._bound_rows:
            for row, icon in self._bound_rows[old_path]:
                row.remove_css_class('now-playing-row')
                icon.set_visible(False)
        if new_path and new_path in self._bound_rows:
            for row, icon in self._bound_rows[new_path]:
                row.add_css_class('now-playing-row')
                icon.set_visible(True)

    def _on_list_activated(self, list_view, position):
        item = self._filter_model.get_item(position)
        if item is None:
            return
        if item.is_folder:
            self._navigate_into(item.path, item.name)
        else:
            self._external_file = False
            self._play_file(item.path)

    # ── Navigation ──────────────────────────────────────────────────

    def _navigate_into(self, folder_path, folder_name):
        self._nav_stack.append(
            (self._current_folder, self._folder_label.get_label())
        )
        self._current_folder = folder_path
        self._folder_label.set_label(folder_name)
        self._back_btn.set_visible(True)
        self._load_folder_contents(folder_path)

    def _navigate_back(self):
        if not self._nav_stack:
            # If multi-folder, go back to virtual root
            settings = Gio.Settings.new('org.gnome.folderplay')
            folders = [f for f in settings.get_strv('music-folders')
                       if os.path.isdir(f)]
            if len(folders) > 1:
                self._current_folder = None
                self._folder_label.set_label(_('Folders'))
                self._back_btn.set_visible(False)
                self._load_virtual_root(folders)
            return
        prev_path, prev_name = self._nav_stack.pop()
        self._current_folder = prev_path
        self._folder_label.set_label(prev_name)
        if prev_path is None:
            # Back to virtual root
            self._back_btn.set_visible(False)
            settings = Gio.Settings.new('org.gnome.folderplay')
            folders = [f for f in settings.get_strv('music-folders')
                       if os.path.isdir(f)]
            self._load_virtual_root(folders)
        else:
            self._back_btn.set_visible(len(self._nav_stack) > 0)
            self._load_folder_contents(prev_path)

    def _navigate_home(self):
        """Go back to the library root."""
        self._search_btn.set_active(False)
        self._search_entry.set_text('')
        self._nav_stack.clear()
        self._return_to_root()

    def _show_tag_info(self):
        """Show a modal dialog with the current song's tag information."""
        tags = self._current_tags
        if not tags:
            return

        dlg = Adw.Dialog()
        dlg.set_title(_('Song Info'))
        dlg.set_content_width(380)
        dlg.set_content_height(420)

        toolbar_view = Adw.ToolbarView()
        header = Adw.HeaderBar()
        header.set_show_end_title_buttons(True)
        toolbar_view.add_top_bar(header)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)

        clamp = Adw.Clamp()
        clamp.set_maximum_size(360)
        clamp.set_margin_top(12)
        clamp.set_margin_bottom(12)

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12,
                          margin_start=16, margin_end=16)

        # Tag fields
        tag_group = Adw.PreferencesGroup(title=_('Tags'))
        track_no = tags.get('track_number', 0)
        fields = [
            (_('Title'), tags.get('title', '')),
            (_('Artist'), tags.get('artist', '')),
            (_('Album'), tags.get('album', '')),
            (_('Year'), tags.get('year', '')),
            (_('Genre'), tags.get('genre', '')),
            (_('Track'), str(track_no) if track_no else ''),
        ]
        for label, value in fields:
            row = Adw.ActionRow(title=label, subtitle=value or '\u2014')
            tag_group.add(row)
        content.append(tag_group)

        # Technical info
        tech_group = Adw.PreferencesGroup(title=_('Audio'))
        fmt = tags.get('format', '')
        sr = tags.get('sample_rate', 0)
        bits = tags.get('bits_per_sample', 0)
        ch = tags.get('channels', 0)
        dur = tags.get('duration', 0)

        tech_fields = [
            ('Format', fmt),
            (_('Duration'), self._format_time(dur) if dur else '\u2014'),
            (_('Sample Rate'), f'{sr:,} Hz' if sr else '\u2014'),
            (_('Bit Depth'), f'{bits}-bit' if bits else '\u2014'),
            (_('Channels'), str(ch) if ch else '\u2014'),
        ]
        for label, value in tech_fields:
            row = Adw.ActionRow(title=label, subtitle=value or '\u2014')
            tech_group.add(row)
        content.append(tech_group)

        # File info
        path = tags.get('path', '')
        if path:
            file_group = Adw.PreferencesGroup(title=_('File'))
            row = Adw.ActionRow(
                title=_('Filename'),
                subtitle=os.path.basename(path),
            )
            file_group.add(row)
            row2 = Adw.ActionRow(
                title=_('Location'),
                subtitle=os.path.dirname(path),
            )
            row2.set_subtitle_lines(2)
            file_group.add(row2)
            try:
                size = os.path.getsize(path)
                if size >= 1024 * 1024:
                    size_str = f'{size / (1024 * 1024):.1f} MB'
                else:
                    size_str = f'{size / 1024:.0f} KB'
                row3 = Adw.ActionRow(title=_('Size'), subtitle=size_str)
                file_group.add(row3)
            except OSError:
                pass
            content.append(file_group)

        clamp.set_child(content)
        scroll.set_child(clamp)
        toolbar_view.set_content(scroll)
        dlg.set_child(toolbar_view)
        dlg.present(self)

    # ── Actions ─────────────────────────────────────────────────────

    def _setup_actions(self):
        for name, cb in [
            ('play-pause', lambda *_: self._player.toggle_play()),
            ('next-track', lambda *_: self._play_next()),
            ('prev-track', lambda *_: self._play_prev()),
            ('open-folder', lambda *_: self._on_open_folder()),
            ('manage-folders', lambda *_: self._on_manage_folders()),
            ('anti-bad-bunny', lambda *_: self._on_anti_bad_bunny()),
        ]:
            action = Gio.SimpleAction.new(name, None)
            action.connect('activate', cb)
            self.add_action(action)

    # ── Signal wiring ───────────────────────────────────────────────

    def _connect_signals(self):
        self._player.connect('state-changed', self._on_player_state)
        self._player.connect('position-updated', self._on_position)
        self._player.connect('song-finished', self._on_song_finished)
        self._player.connect('cover-art-changed', self._on_cover_art)
        self._player.connect('tags-updated', self._on_tags)

        self._play_btn.connect(
            'clicked', lambda b: self._player.toggle_play(),
        )
        self._prev_btn.connect('clicked', lambda b: self._play_prev())
        self._next_btn.connect('clicked', lambda b: self._play_next())
        self._vol_scale.connect('value-changed', self._on_volume_changed)
        self._seek_scale.connect('change-value', self._on_seek)
        self._back_btn.connect('clicked', lambda b: self._navigate_back())
        self._home_btn.connect('clicked', lambda b: self._navigate_home())
        self._tag_btn.connect('clicked', lambda b: self._show_tag_info())
        self._search_entry.connect(
            'search-changed', self._on_search_changed,
        )
        focus_ctrl = Gtk.EventControllerFocus.new()
        focus_ctrl.connect('enter', lambda *_: self._on_search_focus(True))
        focus_ctrl.connect('leave', lambda *_: self._on_search_focus(False))
        self._search_entry.add_controller(focus_ctrl)
        self._dock_btn.connect('clicked', self._on_toggle_browse)
        self._locate_btn.connect('clicked', lambda b: self._on_locate_playing())
        self._browse_revealer.connect(
            'notify::child-revealed', self._on_browse_revealed,
        )
        self._repeat_btn.connect('clicked', self._on_cycle_repeat)
        self._win_minimize.connect('clicked', lambda b: self.minimize())
        self._win_close.connect('clicked', lambda b: self.close())

        # Auto-hide browse panel when window is narrower than 900px
        self.connect('notify::default-width', self._on_window_resized)
        self.connect('notify::maximized', lambda *_: GLib.idle_add(self._check_auto_hide))

        # Mouse back button in browse panel
        back_click = Gtk.GestureClick.new()
        back_click.set_button(8)  # X1 / back button
        back_click.connect('pressed', lambda *_: self._navigate_back())
        self._browse_revealer.add_controller(back_click)

    # ── Folder operations ───────────────────────────────────────────

    def _on_open_folder(self):
        """Quick-add a folder (used from the empty-state button)."""
        dialog = Gtk.FileDialog()
        dialog.set_title(_('Select Music Folder'))
        dialog.select_folder(self, None, self._on_quick_folder_selected)

    def _on_quick_folder_selected(self, dialog, result):
        try:
            folder = dialog.select_folder_finish(result)
            path = folder.get_path()
            # Resolve and cache the display name now
            display = _display_path(path)
            _save_display_name(path, display)
            settings = Gio.Settings.new('org.gnome.folderplay')
            folders = list(settings.get_strv('music-folders'))
            if path not in folders:
                folders.append(path)
                settings.set_strv('music-folders', folders)
            self._load_all_folders()
        except GLib.Error:
            pass

    @staticmethod
    def _is_bad_bunny(artist):
        if not artist:
            return False
        return 'bad bunny' in artist.lower()

    def _get_exclude_artist(self):
        settings = Gio.Settings.new('org.gnome.folderplay')
        if settings.get_boolean('anti-bad-bunny'):
            return 'bad bunny'
        return None

    def _on_anti_bad_bunny(self):
        settings = Gio.Settings.new('org.gnome.folderplay')

        dlg = Adw.Dialog()
        dlg.set_title(_('Anti Bad Bunny'))
        dlg.set_content_width(460)
        dlg.set_content_height(280)

        toolbar = Adw.ToolbarView()
        header = Adw.HeaderBar()
        toolbar.add_top_bar(header)

        page = Adw.PreferencesPage()
        group = Adw.PreferencesGroup(
            title=_('Anti Bad Bunny'),
            description=_(
                'This function will remove from your collection any song '
                'by the artist Bad Bunny (to protect your mental health). '
                "Let's promote real music! (It doesn't delete the files "
                'from disk, it just hides them, in case you have a '
                'relapse...)'
            ),
        )

        row = Adw.SwitchRow(title=_('Anti Bad Bunny'))
        row.set_active(settings.get_boolean('anti-bad-bunny'))

        def _on_toggled(r, _pspec):
            settings.set_boolean('anti-bad-bunny', r.get_active())
            self._reload_current_view()

        row.connect('notify::active', _on_toggled)
        group.add(row)
        page.add(group)
        toolbar.set_content(page)
        dlg.set_child(toolbar)
        dlg.present(self)

    def _reload_current_view(self):
        """Re-load the current folder/root to apply filter changes."""
        if self._current_folder:
            self._load_folder_contents(self._current_folder)
        else:
            settings = Gio.Settings.new('org.gnome.folderplay')
            folders = [f for f in settings.get_strv('music-folders')
                       if os.path.isdir(f)]
            if len(folders) > 1:
                self._load_virtual_root(folders)
            elif len(folders) == 1:
                self._load_folder_contents(folders[0])

    def _on_manage_folders(self):
        """Open the Manage Folders dialog."""
        dlg = Adw.Dialog()
        dlg.set_title(_('Manage Folders'))
        dlg.set_content_width(460)
        dlg.set_content_height(400)

        toolbar_view = Adw.ToolbarView()
        header = Adw.HeaderBar()
        toolbar_view.add_top_bar(header)

        scroll = Gtk.ScrolledWindow(vexpand=True)
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)

        clamp = Adw.Clamp(maximum_size=400, margin_top=12,
                          margin_bottom=12, margin_start=12, margin_end=12)

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)

        group = Adw.PreferencesGroup(title=_('Music Folders'),
                                     description=_('Add local folders to '
                                     'build your music library'))

        settings = Gio.Settings.new('org.gnome.folderplay')
        folders = list(settings.get_strv('music-folders'))

        rows_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        rows_box._folder_rows = {}  # track path → row for removal

        def _add_folder_row(path):
            row = Adw.ActionRow(title=os.path.basename(path))
            row.set_icon_name('folder-music-symbolic')
            remove_btn = Gtk.Button(icon_name='minus-circle-outline-symbolic')
            remove_btn.add_css_class('flat')
            remove_btn.add_css_class('circular')
            remove_btn.set_valign(Gtk.Align.CENTER)
            remove_btn.set_tooltip_text(_('Remove this folder'))
            remove_btn.connect('clicked',
                               lambda b, p=path: _remove_folder(p))
            row.add_suffix(remove_btn)
            group.add(row)
            rows_box._folder_rows[path] = row

        def _remove_folder(path):
            row = rows_box._folder_rows.pop(path, None)
            if row:
                group.remove(row)
            s = Gio.Settings.new('org.gnome.folderplay')
            cur = list(s.get_strv('music-folders'))
            if path in cur:
                cur.remove(path)
                s.set_strv('music-folders', cur)

        def _on_add_clicked(btn):
            fd = Gtk.FileDialog()
            fd.set_title(_('Add Music Folder'))
            fd.select_folder(self, None,
                             lambda d, r: _on_add_result(d, r))

        def _on_add_result(d, r):
            try:
                folder = d.select_folder_finish(r)
                path = folder.get_path()
                display = _display_path(path)
                _save_display_name(path, display)
                s = Gio.Settings.new('org.gnome.folderplay')
                cur = list(s.get_strv('music-folders'))
                if path not in cur:
                    cur.append(path)
                    s.set_strv('music-folders', cur)
                    _add_folder_row(path)
            except GLib.Error:
                pass

        for f in folders:
            _add_folder_row(f)

        content.append(group)

        add_btn = Gtk.Button()
        add_box = Gtk.Box(spacing=8, halign=Gtk.Align.CENTER)
        add_box.append(Gtk.Image.new_from_icon_name('plus-circle-outline-symbolic'))
        add_box.append(Gtk.Label(label=_('Add Folder')))
        add_btn.set_child(add_box)
        add_btn.add_css_class('pill')
        add_btn.add_css_class('suggested-action')
        add_btn.set_halign(Gtk.Align.CENTER)
        add_btn.connect('clicked', _on_add_clicked)
        content.append(add_btn)

        clamp.set_child(content)
        scroll.set_child(clamp)
        toolbar_view.set_content(scroll)
        dlg.set_child(toolbar_view)
        dlg.connect('closed', lambda d: self._load_all_folders())
        dlg.present(self)

    def _reset_library(self):
        """Clear the library view."""
        self._root_folder = None
        self._current_folder = None
        self._nav_stack.clear()
        self._playlist = []
        self._current_index = -1
        self._list_store.remove_all()
        self._current_items = []
        self._folder_cover_cache.clear()
        self._folder_preview_cache.clear()
        self._db.clear_all()
        self._back_btn.set_visible(False)
        self._folder_label.set_label(_('Folders'))
        self._song_count_label.set_label('')
        self._browse_stack.set_visible_child_name('empty')

    def _return_to_root(self):
        """Return to root view."""
        settings = Gio.Settings.new('org.gnome.folderplay')
        folders = [f for f in settings.get_strv('music-folders')
                   if os.path.isdir(f)]
        if not folders:
            self._reset_library()
            return
        if len(folders) == 1:
            path = folders[0]
            self._root_folder = path
            self._current_folder = path
            self._nav_stack.clear()
            self._back_btn.set_visible(False)
            self._folder_label.set_label(_('Folder'))
            self._load_folder_contents(path)
        else:
            self._root_folder = None
            self._current_folder = None
            self._nav_stack.clear()
            self._back_btn.set_visible(False)
            self._folder_label.set_label(_('Folders'))
            self._load_virtual_root(folders)

    def _load_all_folders(self):
        """Load all saved music folders into the library view."""
        settings = Gio.Settings.new('org.gnome.folderplay')
        folders = [f for f in settings.get_strv('music-folders')
                   if os.path.isdir(f)]
        if not folders:
            self._reset_library()
            return

        if len(folders) == 1:
            self._load_root_folder(folders[0])
        else:
            self._root_folder = None
            self._current_folder = None
            self._nav_stack.clear()
            self._back_btn.set_visible(False)
            self._folder_label.set_label(_('Folders'))
            self._playlist = []
            self._current_index = -1
            self._load_virtual_root(folders)
            threading.Thread(
                target=self._deep_scan_and_index, args=(folders,),
                daemon=True,
            ).start()

    def open_file(self, path):
        """Open and play a file passed from the system (e.g. file manager).

        Plays the file immediately without altering the browse panel,
        the playlist, or saving anything to GSettings / DB.
        """
        if not path or not os.path.isfile(path):
            return
        ext = os.path.splitext(path)[1].lower()
        if ext not in AUDIO_EXTENSIONS:
            return

        self._external_file = True
        self._play_file(path)
        self._locate_btn.set_visible(False)

    def _load_virtual_root(self, folders):
        """Show each folder as a navigable entry."""
        self._browse_stack.set_visible_child_name('loading')
        self._filter_model.set_filter(None)
        self._search_entry.set_text('')
        threading.Thread(
            target=self._scan_virtual_root, args=(folders,), daemon=True,
        ).start()

    def _scan_virtual_root(self, folders):
        items = []
        hide_bb = Gio.Settings.new('org.gnome.folderplay').get_boolean('anti-bad-bunny')
        for f in folders:
            if hide_bb and self._is_bad_bunny(os.path.basename(f)):
                continue
            fi = FileItem(f, os.path.basename(_display_path(f)), True)
            fi.cover_thumb = self._get_folder_preview(f)
            items.append(fi)
        GLib.idle_add(self._populate_list, items)

    def _build_multi_playlist_worker(self, folders):
        excl = self._get_exclude_artist()
        playlist = self._db.get_playlist(folders, exclude_artist=excl)
        if not playlist:
            # Fallback: filesystem scan if DB empty
            for folder in folders:
                self._build_playlist_recursive(folder, playlist)
        GLib.idle_add(self._set_playlist, playlist)

    def _load_root_folder(self, path):
        self._root_folder = path
        self._current_folder = path
        self._nav_stack.clear()
        self._back_btn.set_visible(False)
        self._folder_label.set_label(_('Folder'))
        self._playlist = []
        self._current_index = -1
        self._load_folder_contents(path)
        threading.Thread(
            target=self._deep_scan_and_index, args=([path],),
            daemon=True,
        ).start()

    def _deep_scan_and_index(self, folders):
        """Deep-scan folder trees into DB, then build playlist/index from DB."""
        for folder in folders:
            if self._shutdown.is_set():
                return
            self._db.scan_folder_deep(folder)
        if self._shutdown.is_set():
            return
        # Build playlist from DB
        excl = self._get_exclude_artist()
        playlist = self._db.get_playlist(folders, exclude_artist=excl)
        GLib.idle_add(self._set_playlist, playlist)
        # Background metadata enrichment (search reads DB directly)
        self._enrich_all_metadata(folders)

    def _enrich_all_metadata(self, folders):
        """Enrich unscanned songs with Discoverer metadata."""
        try:
            discoverer = GstPbutils.Discoverer.new(3 * Gst.SECOND)
        except GLib.Error:
            return
        while not self._shutdown.is_set():
            batch = self._db.get_unscanned_songs(folders, limit=50)
            if not batch:
                break
            for song_path in batch:
                try:
                    uri = GLib.filename_to_uri(song_path, None)
                    info = discoverer.discover_uri(uri)
                    dur = info.get_duration()
                    duration = dur / Gst.SECOND if dur > 0 else 0.0
                    title = artist = album = year = ''
                    bitrate = sample_rate = bits_per_sample = 0
                    streams = info.get_audio_streams()
                    if streams:
                        a = streams[0]
                        br = a.get_bitrate()
                        bitrate = br // 1000 if br else 0
                        sample_rate = a.get_sample_rate()
                        bits_per_sample = a.get_depth()
                    tags = info.get_tags()
                    if tags:
                        ok, v = tags.get_string('title')
                        if ok and v:
                            title = v
                        ok, v = tags.get_string('artist')
                        if ok and v:
                            artist = v
                        ok, v = tags.get_string('album')
                        if ok and v:
                            album = v
                        ok, dt = tags.get_date_time('datetime')
                        if ok and dt:
                            year = str(dt.get_year())
                        else:
                            ok, d = tags.get_date('date')
                            if ok and d:
                                year = str(d.get_year())
                        # Extract per-song cover art
                        folder = os.path.dirname(song_path)
                        sample = None
                        ok, s = tags.get_sample_index('image', 0)
                        if ok:
                            sample = s
                        else:
                            ok, s = tags.get_sample_index(
                                'preview-image', 0,
                            )
                            if ok:
                                sample = s
                        if sample:
                            buf = sample.get_buffer()
                            ok, map_info = buf.map(Gst.MapFlags.READ)
                            if ok:
                                data = bytes(map_info.data)
                                buf.unmap(map_info)
                                self._db.set_song_cover(song_path, data)
                                if not self._db.has_cover(folder):
                                    self._db.set_cover(
                                        folder, data, 'embedded',
                                    )
                    if not title:
                        title = os.path.splitext(
                            os.path.basename(song_path),
                        )[0]
                    self._db.update_song_metadata(
                        song_path, title, artist, album, year,
                        bitrate, sample_rate, bits_per_sample, duration,
                    )
                except (GLib.Error, Exception):
                    pass


    def _set_playlist(self, playlist):
        self._playlist = playlist
        count = len(playlist)
        if count > 0:
            self._song_count_label.set_label(
                ngettext('%d song', '%d songs', count) % count
            )
        else:
            self._song_count_label.set_label('')
        return False

    def _build_playlist_recursive(self, path, playlist):
        try:
            entries = sorted(
                os.scandir(path),
                key=lambda e: (not e.is_dir(), e.name.lower()),
            )
        except (PermissionError, OSError):
            return
        for entry in entries:
            if entry.name.startswith('.'):
                continue
            try:
                if entry.is_dir(follow_symlinks=False):
                    self._build_playlist_recursive(entry.path, playlist)
                elif entry.is_file():
                    ext = os.path.splitext(entry.name)[1].lower()
                    if ext in AUDIO_EXTENSIONS:
                        playlist.append(entry.path)
            except OSError:
                continue



    def _load_folder_contents(self, path):
        self._browse_stack.set_visible_child_name('loading')
        self._filter_model.set_filter(None)
        self._search_entry.set_text('')
        threading.Thread(
            target=self._scan_worker, args=(path,), daemon=True,
        ).start()

    def _scan_worker(self, path):
        items = []
        hide_bb = Gio.Settings.new('org.gnome.folderplay').get_boolean('anti-bad-bunny')

        # Check if folder changed since last scan
        needs_rescan = self._db.folder_needs_rescan(path)

        if needs_rescan:
            # Scan filesystem and update DB
            self._db.scan_folder(path, parent=self._root_folder if path != self._root_folder else None)

        # Load from DB (always fast)
        db_folders = self._db.get_folder_children(path)
        db_songs = self._db.get_folder_songs(path)

        # Build folder items
        for sf in db_folders:
            if hide_bb and self._is_bad_bunny(sf['name']):
                continue
            has_audio = self._db.folder_has_audio_cached(sf['path'])
            if has_audio is None:
                # Not scanned yet — check filesystem
                has_audio = self._has_audio(sf['path'])
            if has_audio:
                fi = FileItem(sf['path'], sf['name'], True)
                fi.cover_thumb = self._get_folder_preview(sf['path'])
                items.append(fi)

        # Get folder cover as fallback
        folder_cover = self._get_folder_cover(path)

        # Build song items from DB
        for rec in db_songs:
            if hide_bb and self._is_bad_bunny(rec.get('artist')):
                continue
            item = FileItem(rec['path'], rec['name'])
            item.title = rec['title'] or os.path.splitext(rec['name'])[0]
            item.artist = rec['artist']
            item.album = rec['album']
            item.year = rec['year']
            item.format_type = rec['format_type']
            item.bitrate = rec['bitrate']
            item.sample_rate = rec['sample_rate']
            item.bits_per_sample = rec['bits_per_sample']
            item.duration = rec['duration']
            # Per-song cover → folder cover → fallback
            song_cover = self._db.get_song_cover(rec['path'])
            if song_cover:
                try:
                    item.cover_thumb = Gdk.Texture.new_from_bytes(
                        GLib.Bytes(song_cover),
                    )
                except GLib.Error:
                    item.cover_thumb = folder_cover
            else:
                item.cover_thumb = folder_cover
            items.append(item)

        GLib.idle_add(self._populate_list, items)

        # Enrich metadata in background for songs not yet scanned
        if needs_rescan or any(
            not r.get('meta_scanned') for r in db_songs
        ):
            self._enrich_folder_metadata(path)

    def _enrich_folder_metadata(self, folder_path):
        """Run Discoverer on songs missing metadata and update DB."""
        if self._shutdown.is_set():
            return
        unscanned = self._db.get_unscanned_songs([folder_path])
        if not unscanned:
            return
        try:
            discoverer = GstPbutils.Discoverer.new(3 * Gst.SECOND)
        except GLib.Error:
            return

        for song_path in unscanned:
            try:
                uri = GLib.filename_to_uri(song_path, None)
                info = discoverer.discover_uri(uri)

                dur = info.get_duration()
                duration = dur / Gst.SECOND if dur > 0 else 0.0
                title = artist = album = year = ''
                bitrate = sample_rate = bits_per_sample = 0

                streams = info.get_audio_streams()
                if streams:
                    a = streams[0]
                    br = a.get_bitrate()
                    bitrate = br // 1000 if br else 0
                    sample_rate = a.get_sample_rate()
                    bits_per_sample = a.get_depth()

                tags = info.get_tags()
                if tags:
                    ok, v = tags.get_string('title')
                    if ok and v:
                        title = v
                    ok, v = tags.get_string('artist')
                    if ok and v:
                        artist = v
                    ok, v = tags.get_string('album')
                    if ok and v:
                        album = v
                    ok, dt = tags.get_date_time('datetime')
                    if ok and dt:
                        year = str(dt.get_year())
                    else:
                        ok, d = tags.get_date('date')
                        if ok and d:
                            year = str(d.get_year())

                    # Extract per-song cover art
                    sample = None
                    ok, s = tags.get_sample_index('image', 0)
                    if ok:
                        sample = s
                    else:
                        ok, s = tags.get_sample_index(
                            'preview-image', 0,
                        )
                        if ok:
                            sample = s
                    if sample:
                        buf = sample.get_buffer()
                        ok, map_info = buf.map(Gst.MapFlags.READ)
                        if ok:
                            data = bytes(map_info.data)
                            buf.unmap(map_info)
                            self._db.set_song_cover(song_path, data)
                            # Also set as folder cover if none yet
                            if not self._db.has_cover(folder_path):
                                self._db.set_cover(
                                    folder_path, data, 'embedded',
                                )

                if not title:
                    title = os.path.splitext(
                        os.path.basename(song_path),
                    )[0]
                self._db.update_song_metadata(
                    song_path, title, artist, album, year,
                    bitrate, sample_rate, bits_per_sample, duration,
                )
            except (GLib.Error, Exception):
                pass

        # Refresh visible list with enriched metadata + per-song covers
        GLib.idle_add(self._refresh_enriched_items, folder_path)

    def _refresh_enriched_items(self, folder_path):
        """Update visible song items with enriched metadata and covers."""
        if self._current_folder != folder_path:
            return False
        n = self._list_store.get_n_items()
        if n == 0:
            return False
        songs = self._db.get_folder_songs(folder_path)
        song_map = {s['path']: s for s in songs}
        for i in range(n):
            item = self._list_store.get_item(i)
            if not item or item.is_folder:
                continue
            rec = song_map.get(item.path)
            if rec:
                item.title = rec['title'] or os.path.splitext(rec['name'])[0]
                item.artist = rec['artist']
                item.album = rec['album']
                item.format_type = rec['format_type']
                item.bitrate = rec['bitrate']
                item.sample_rate = rec['sample_rate']
                item.bits_per_sample = rec['bits_per_sample']
                item.duration = rec['duration']
            song_cover = self._db.get_song_cover(item.path)
            if song_cover:
                try:
                    item.cover_thumb = Gdk.Texture.new_from_bytes(
                        GLib.Bytes(song_cover),
                    )
                except GLib.Error:
                    pass
        self._populate_list(self._current_items)
        return False

    def _extract_meta(self, item, info):
        dur = info.get_duration()
        if dur > 0:
            item.duration = dur / Gst.SECOND

        streams = info.get_audio_streams()
        if streams:
            a = streams[0]
            br = a.get_bitrate()
            item.bitrate = br // 1000 if br else 0
            item.sample_rate = a.get_sample_rate()
            item.bits_per_sample = a.get_depth()

        tags = info.get_tags()
        if tags:
            ok, v = tags.get_string('title')
            if ok and v:
                item.title = v
            ok, v = tags.get_string('artist')
            if ok and v:
                item.artist = v
            ok, v = tags.get_string('album')
            if ok and v:
                item.album = v
            ok, dt = tags.get_date_time('datetime')
            if ok and dt:
                item.year = str(dt.get_year())
            else:
                ok, d = tags.get_date('date')
                if ok and d:
                    item.year = str(d.get_year())

    def _extract_embedded_cover(self, item, info):
        tags = info.get_tags()
        if not tags:
            return
        sample = None
        ok, s = tags.get_sample_index('image', 0)
        if ok:
            sample = s
        else:
            ok, s = tags.get_sample_index('preview-image', 0)
            if ok:
                sample = s
        if not sample:
            return
        buf = sample.get_buffer()
        ok, map_info = buf.map(Gst.MapFlags.READ)
        if ok:
            data = bytes(map_info.data)
            buf.unmap(map_info)
            try:
                item.cover_thumb = Gdk.Texture.new_from_bytes(
                    GLib.Bytes(data)
                )
            except GLib.Error:
                pass

    def _get_folder_preview(self, path, depth=0):
        """Return a cover texture for *path* (recursive up to depth 5).

        Priority: DB cache → cover file → embedded art → recurse dirs.
        """
        if depth == 0:
            cached = self._folder_preview_cache.get(path)
            if cached is not None:
                return cached or None
            # Try DB cover cache (instant)
            cover_data = self._db.get_cover(path)
            if cover_data:
                try:
                    texture = Gdk.Texture.new_from_bytes(
                        GLib.Bytes(cover_data),
                    )
                    self._folder_preview_cache[path] = texture
                    return texture
                except GLib.Error:
                    pass
        if depth > 5:
            return None
        # 1. Cover image file in this folder
        cover = self._get_folder_cover(path)
        if cover:
            if depth == 0:
                self._folder_preview_cache[path] = cover
            return cover
        # 2. Embedded art from first audio file in this folder
        discoverer = None
        try:
            discoverer = GstPbutils.Discoverer.new(3 * Gst.SECOND)
        except GLib.Error:
            pass
        subdirs = []
        try:
            entries = sorted(
                os.scandir(path), key=lambda x: x.name.lower(),
            )
        except (PermissionError, OSError):
            return None
        for e in entries:
            if e.name.startswith('.'):
                continue
            try:
                if e.is_dir(follow_symlinks=False):
                    subdirs.append(e.path)
                elif e.is_file() and discoverer:
                    ext = os.path.splitext(e.name)[1].lower()
                    if ext not in AUDIO_EXTENSIONS:
                        continue
                    try:
                        uri = GLib.filename_to_uri(e.path, None)
                        info = discoverer.discover_uri(uri)
                        tags = info.get_tags()
                        if not tags:
                            continue
                        sample = None
                        ok, s = tags.get_sample_index('image', 0)
                        if ok:
                            sample = s
                        else:
                            ok, s = tags.get_sample_index(
                                'preview-image', 0,
                            )
                            if ok:
                                sample = s
                        if not sample:
                            continue
                        buf = sample.get_buffer()
                        ok, map_info = buf.map(Gst.MapFlags.READ)
                        if ok:
                            data = bytes(map_info.data)
                            buf.unmap(map_info)
                            try:
                                texture = Gdk.Texture.new_from_bytes(
                                    GLib.Bytes(data),
                                )
                                # Cache in DB
                                self._db.set_cover(
                                    path, data, 'embedded',
                                )
                                if depth == 0:
                                    self._folder_preview_cache[path] = texture
                                return texture
                            except GLib.Error:
                                pass
                    except GLib.Error:
                        continue
            except OSError:
                continue
        # 3. Recurse into subdirectories
        for sub in subdirs:
            result = self._get_folder_preview(sub, depth + 1)
            if result:
                if depth == 0:
                    self._folder_preview_cache[path] = result
                return result
        if depth == 0:
            self._folder_preview_cache[path] = False
        return None

    def _has_audio(self, path, depth=0):
        # Try DB cache first
        cached = self._db.folder_has_audio_cached(path)
        if cached is not None:
            return cached
        if depth > 5:
            return False
        try:
            for e in os.scandir(path):
                if e.name.startswith('.'):
                    continue
                if e.is_file():
                    ext = os.path.splitext(e.name)[1].lower()
                    if ext in AUDIO_EXTENSIONS:
                        return True
                elif e.is_dir(follow_symlinks=False):
                    if self._has_audio(e.path, depth + 1):
                        return True
        except (PermissionError, OSError):
            pass
        return False

    def _get_folder_cover(self, path):
        if path in self._folder_cover_cache:
            return self._folder_cover_cache[path]
        # Try DB cache first
        cover_data = self._db.get_cover(path)
        if cover_data:
            try:
                texture = Gdk.Texture.new_from_bytes(
                    GLib.Bytes(cover_data),
                )
                self._folder_cover_cache[path] = texture
                return texture
            except GLib.Error:
                pass
        # Scan filesystem for cover file
        for name in COVER_NAMES:
            p = os.path.join(path, name)
            if os.path.isfile(p):
                try:
                    texture = Gdk.Texture.new_from_filename(p)
                    self._folder_cover_cache[path] = texture
                    # Store in DB for next time
                    try:
                        with open(p, 'rb') as f:
                            self._db.set_cover(path, f.read(), 'file')
                    except OSError:
                        pass
                    return texture
                except GLib.Error:
                    continue
        self._folder_cover_cache[path] = None
        return None

    def _get_cover_file(self, folder):
        for name in COVER_NAMES:
            p = os.path.join(folder, name)
            if os.path.isfile(p):
                return p
        return None

    def _populate_list(self, items):
        self._current_items = items
        self._list_store.remove_all()

        if not items:
            self._browse_stack.set_visible_child_name('empty')
            return

        use_grid = (
            self._grid_toggle.get_active()
            and self._has_folders_only(items)
        )

        self._grid_toggle.set_visible(self._has_folders_only(items))

        if use_grid:
            # Populate GridView
            self._grid_store.remove_all()
            for item in items:
                self._grid_store.append(item)
            self._browse_stack.set_visible_child_name('grid')
        else:
            # Populate ListView
            for item in items:
                self._list_store.append(item)
            self._browse_stack.set_visible_child_name('list')

            # Scroll to pending song or top
            scroll_pos = 0
            if self._pending_scroll_path:
                for i, item in enumerate(items):
                    if item.path == self._pending_scroll_path:
                        scroll_pos = i
                        break
                self._pending_scroll_path = None
            self._list_view.scroll_to(
                scroll_pos, Gtk.ListScrollFlags.FOCUS, None,
            )

    # ── Search ──────────────────────────────────────────────────────

    def _on_search_changed(self, entry):
        query = entry.get_text().strip().lower()
        if not query:
            # Restore current folder view and label
            if self._current_folder:
                self._folder_label.set_label(
                    os.path.basename(self._current_folder)
                )
            elif self._root_folder:
                self._folder_label.set_label(_('Folder'))
            else:
                self._folder_label.set_label(_('Folders'))
            if self._current_items:
                self._populate_list(self._current_items)
            else:
                self._filter_model.set_filter(None)
            return
        self._folder_label.set_label(_('Search: %s') % query)
        words = query.split()
        results = []
        settings = Gio.Settings.new('org.gnome.folderplay')
        roots = [f for f in settings.get_strv('music-folders')
                 if os.path.isdir(f)]
        hide_bb = settings.get_boolean('anti-bad-bunny')
        for rec in self._db.search_songs(words, roots):
            if hide_bb and self._is_bad_bunny(rec.get('artist')):
                continue
            item = FileItem(rec['path'], rec['name'])
            item.title = rec['title'] or os.path.splitext(rec['name'])[0]
            item.artist = rec['artist']
            item.album = rec['album']
            item.format_type = rec['format_type']
            item.sample_rate = rec['sample_rate']
            item.bits_per_sample = rec['bits_per_sample']
            item.bitrate = rec['bitrate']
            item.duration = rec['duration']
            # Load per-song cover, fall back to folder cover
            song_cover = self._db.get_song_cover(rec['path'])
            if not song_cover:
                song_cover = self._db.get_cover(rec['folder'])
            if song_cover:
                try:
                    item.cover_thumb = Gdk.Texture.new_from_bytes(
                        GLib.Bytes(song_cover),
                    )
                except GLib.Error:
                    pass
            results.append(item)
        self._list_store.remove_all()
        for item in results:
            self._list_store.append(item)
        self._filter_model.set_filter(None)
        self._grid_toggle.set_visible(False)
        if results:
            self._browse_stack.set_visible_child_name('list')
        else:
            self._browse_stack.set_visible_child_name('search-empty')

    def _on_search_focus(self, focused):
        app = self.get_application()
        if not app:
            return
        if focused:
            app.set_accels_for_action('win.play-pause', [])
            app.set_accels_for_action('win.next-track', [])
            app.set_accels_for_action('win.prev-track', [])
        else:
            app.set_accels_for_action('win.play-pause', ['space'])
            app.set_accels_for_action('win.next-track', ['Right'])
            app.set_accels_for_action('win.prev-track', ['Left'])

    # ── Playback ────────────────────────────────────────────────────

    def _play_file(self, path):
        uri = GLib.filename_to_uri(path, None)
        self._player.play_uri(uri)

        # Track old playing path for now-playing widget update
        old_path = self._playing_path
        self._playing_path = path
        if path in self._playlist:
            self._current_index = self._playlist.index(path)

        # Update now-playing widgets directly (no list store changes)
        self._update_now_playing_widgets(old_path, path)
        if not self._external_file:
            self._locate_btn.set_visible(self.get_width() >= 770)

        self._cover_texture = None
        self._cover_stack.set_visible_child_name('placeholder')
        self._update_background_colors(None)

        name = os.path.splitext(os.path.basename(path))[0]
        self._title_label.set_label(name)
        self._subtitle_label.set_label('')
        self._format_label.set_visible(False)
        self._hires_icon_player.set_visible(False)
        self._current_tags = {'path': path}
        self._tag_btn.set_visible(True)

        # Discover audio info for format/bitrate line + hires
        self._discover_audio_info(path)

        cover_file = self._get_cover_file(os.path.dirname(path))
        if cover_file:
            try:
                texture = Gdk.Texture.new_from_filename(cover_file)
                self._set_cover_art(texture)
            except GLib.Error:
                pass

    def _play_next(self):
        if not self._playlist:
            return
        self._current_index = (
            (self._current_index + 1) % len(self._playlist)
        )
        self._play_file(self._playlist[self._current_index])

    def _play_prev(self):
        if not self._playlist:
            return
        self._current_index = (
            (self._current_index - 1) % len(self._playlist)
        )
        self._play_file(self._playlist[self._current_index])

    def _discover_audio_info(self, path):
        def _worker():
            try:
                discoverer = GstPbutils.Discoverer.new(3 * Gst.SECOND)
                uri = GLib.filename_to_uri(path, None)
                info = discoverer.discover_uri(uri)
            except GLib.Error:
                return

            ext = os.path.splitext(path)[1].lower()
            fmt = ext[1:].upper()
            sample_rate = 0
            bitrate = 0
            bits = 0

            streams = info.get_audio_streams()
            if streams:
                a = streams[0]
                br = a.get_bitrate()
                bitrate = br // 1000 if br else 0
                sample_rate = a.get_sample_rate()
                bits = a.get_depth()

            dur = info.get_duration()
            duration = dur / Gst.SECOND if dur > 0 else 0

            parts = [fmt]
            if fmt in ('DSF', 'DFF') and sample_rate:
                dsd_level = round(sample_rate * 8 / 44100)
                parts.append(f'DSD{dsd_level}')
            elif fmt in LOSSLESS_FORMATS and sample_rate:
                parts.append(f'{bits or 16}-bit/{sample_rate / 1000:g}kHz')
            elif bitrate:
                parts.append(f'{bitrate} kbps')

            label = ' \u2022 '.join(parts)
            is_hires = sample_rate >= 48000

            genre = ''
            track_number = 0
            album_artist = ''
            tags_obj = info.get_tags()
            if tags_obj:
                ok, v = tags_obj.get_string('genre')
                if ok and v:
                    genre = v
                ok, v = tags_obj.get_uint('track-number')
                if ok and v:
                    track_number = v
                ok, v = tags_obj.get_string('album-artist')
                if ok and v:
                    album_artist = v

            tech = {
                'format': fmt,
                'sample_rate': sample_rate,
                'bits_per_sample': bits,
                'bitrate': bitrate,
                'duration': duration,
                'channels': a.get_channels() if streams else 0,
                'genre': genre,
                'track_number': track_number,
                'album_artist': album_artist,
            }
            GLib.idle_add(self._show_audio_info, label, is_hires, tech)

        threading.Thread(target=_worker, daemon=True).start()

    def _show_audio_info(self, label, is_hires, tech=None):
        self._format_label.set_label(label)
        self._format_label.set_visible(True)
        self._hires_icon_player.set_visible(is_hires)
        if tech:
            self._current_tags.update(tech)

    # ── Player callbacks ────────────────────────────────────────────

    def _on_player_state(self, player, is_playing):
        icon = (
            'media-playback-pause-symbolic'
            if is_playing
            else 'media-playback-start-symbolic'
        )
        self._play_btn.set_icon_name(icon)

    def _on_position(self, player, position, duration):
        now = GLib.get_monotonic_time()
        if (now - self._last_seek_time) > 800000:
            if duration > 0:
                self._seek_scale.set_range(0, duration)
                self._seek_scale.set_value(position)
        self._pos_label.set_label(self._format_time(position))
        self._dur_label.set_label(self._format_time(duration))

    def _on_song_finished(self, player):
        if self._repeat_mode == REPEAT_LOOP:
            # Loop entire playlist: advance and wrap around
            self._play_next()
            return
        if self._repeat_mode == REPEAT_ONCE:
            # Repeat current song once, then switch to consecutive
            self._repeat_mode = REPEAT_CONSECUTIVE
            self._repeat_btn.set_icon_name(REPEAT_ICONS[REPEAT_CONSECUTIVE])
            self._repeat_btn.set_tooltip_text(_('Consecutive'))
            if self._playlist and 0 <= self._current_index < len(self._playlist):
                self._play_file(self._playlist[self._current_index])
            return
        # REPEAT_CONSECUTIVE: advance, but stop at end of playlist
        if not self._playlist:
            return
        if self._current_index < len(self._playlist) - 1:
            self._current_index += 1
            self._play_file(self._playlist[self._current_index])
        else:
            self._player.stop()
            self._play_btn.set_icon_name('media-playback-start-symbolic')

    def _on_cover_art(self, player, data):
        if data:
            try:
                texture = Gdk.Texture.new_from_bytes(GLib.Bytes(data))
                self._set_cover_art(texture)
            except GLib.Error:
                pass

    def _on_tags(self, player, title, artist, album, year):
        if title:
            self._title_label.set_label(title)
            self._current_tags['title'] = title
        if artist:
            self._current_tags['artist'] = artist
        if album:
            self._current_tags['album'] = album
        if year:
            self._current_tags['year'] = year
        parts = [p for p in (artist, album) if p]
        if year:
            parts.append(year)
        if parts:
            self._subtitle_label.set_label(' \u2014 '.join(parts))

    # ── Cover art & Amberol-style gradient background ──────────────

    def _set_cover_art(self, texture):
        self._cover_texture = texture
        self._cover_picture.set_paintable(texture)
        self._cover_stack.set_visible_child_name('art')
        self._update_background_colors(texture)

    def _update_background_colors(self, texture):
        if texture:
            palette = self._extract_palette(texture, 3)
            if palette and len(palette) >= 3:
                c = palette
                css = (
                    '.album-bg {'
                    '  background:'
                    f'    linear-gradient(127deg,'
                    f'      rgba({c[0][0]},{c[0][1]},{c[0][2]},0.55),'
                    f'      rgba({c[0][0]},{c[0][1]},{c[0][2]},0.0) 70.71%),'
                    f'    linear-gradient(217deg,'
                    f'      rgba({c[1][0]},{c[1][1]},{c[1][2]},0.55),'
                    f'      rgba({c[1][0]},{c[1][1]},{c[1][2]},0.0) 70.71%),'
                    f'    linear-gradient(336deg,'
                    f'      rgba({c[2][0]},{c[2][1]},{c[2][2]},0.55),'
                    f'      rgba({c[2][0]},{c[2][1]},{c[2][2]},0.0) 70.71%);'
                    '}'
                )
                self._dynamic_css.load_from_string(css)
                return
        self._dynamic_css.load_from_string('')

    # ── Median-cut palette extraction (à la color-thief) ────────────

    def _extract_palette(self, texture, n_colors=3):
        w = texture.get_width()
        h = texture.get_height()

        try:
            downloader = Gdk.TextureDownloader.new(texture)
            downloader.set_format(Gdk.MemoryFormat.R8G8B8A8)
            glib_bytes, stride = downloader.download_bytes()
            data = glib_bytes.get_data()
        except Exception:
            return None

        step_x = max(1, w // 64)
        step_y = max(1, h // 64)

        pixels = []
        for y in range(0, h, step_y):
            for x in range(0, w, step_x):
                off = y * stride + x * 4
                pixels.append((data[off], data[off + 1], data[off + 2]))

        return self._median_cut(pixels, n_colors)

    def _median_cut(self, pixels, n_colors):
        if not pixels:
            return None

        buckets = [pixels]
        while len(buckets) < n_colors:
            # Find bucket with the widest color channel range
            best_range = -1
            best_idx = 0
            best_ch = 0
            for i, bkt in enumerate(buckets):
                for ch in range(3):
                    lo = min(p[ch] for p in bkt)
                    hi = max(p[ch] for p in bkt)
                    span = hi - lo
                    if span > best_range:
                        best_range = span
                        best_idx = i
                        best_ch = ch

            bkt = buckets.pop(best_idx)
            bkt.sort(key=lambda p: p[best_ch])
            mid = len(bkt) // 2
            buckets.append(bkt[:mid])
            buckets.append(bkt[mid:])

        colors = []
        for bkt in buckets:
            if not bkt:
                continue
            r = sum(p[0] for p in bkt) // len(bkt)
            g = sum(p[1] for p in bkt) // len(bkt)
            b = sum(p[2] for p in bkt) // len(bkt)
            colors.append((r, g, b))
        return colors

    # ── Control handlers ────────────────────────────────────────────

    def _on_toggle_browse(self, button, auto=False):
        self._browse_visible = not self._browse_visible
        if self._browse_visible:
            if not auto:
                self._browse_manual_closed = False
                if self.get_width() <= 750:
                    self.set_default_size(950, self.get_height())
            self._player_panel.set_hexpand(False)
            self._player_panel.set_size_request(PLAYER_WIDTH, -1)
            self._browse_revealer.set_hexpand(True)
            self._browse_revealer.set_reveal_child(True)
            self._sep.set_visible(True)
        else:
            if not auto:
                self._browse_manual_closed = True
            self._browse_revealer.set_reveal_child(False)
            self._sep.set_visible(False)

    def _on_browse_revealed(self, revealer, pspec):
        if not revealer.get_child_revealed():
            revealer.set_hexpand(False)
            self._player_panel.set_hexpand(True)
            self._player_panel.set_size_request(-1, -1)

    def _on_locate_playing(self):
        """Open the folder of the currently playing song and scroll to it."""
        if (not self._playlist
                or not (0 <= self._current_index < len(self._playlist))):
            return
        playing_path = self._playlist[self._current_index]
        folder_path = os.path.dirname(playing_path)

        # Ensure browse panel is visible
        if not self._browse_visible:
            self._on_toggle_browse(self._dock_btn)

        # Clear search
        self._search_btn.set_active(False)
        self._search_entry.set_text('')

        # Navigate to folder: reset nav stack
        self._nav_stack.clear()

        # Determine the root to set up proper back navigation
        settings = Gio.Settings.new('org.gnome.folderplay')
        folders = [f for f in settings.get_strv('music-folders')
                       if os.path.isdir(f)]

        # Build nav stack from root to the target folder
        if len(folders) > 1:
            # Multi-folder: virtual root → target folder
            self._nav_stack.append((None, _('Folders')))
            self._current_folder = folder_path
            self._folder_label.set_label(os.path.basename(folder_path))
            self._back_btn.set_visible(True)
        elif len(folders) == 1:
            root = folders[0]
            if folder_path == root:
                # Playing song is in the root folder itself
                self._current_folder = root
                self._folder_label.set_label(os.path.basename(root))
                self._back_btn.set_visible(False)
            else:
                # Build intermediate nav stack entries
                rel = os.path.relpath(folder_path, root)
                parts = rel.split(os.sep)
                # Root as first entry — use root path so back loads its contents
                self._nav_stack.append((root, _('Folder')))
                self._back_btn.set_visible(True)
                # Intermediate folders
                current = root
                for part in parts[:-1]:
                    current = os.path.join(current, part)
                    self._nav_stack.append((current, part))
                self._current_folder = folder_path
                self._folder_label.set_label(parts[-1])
        else:
            return

        # Load the folder and scroll to the playing song once loaded
        self._pending_scroll_path = playing_path
        self._load_folder_contents(folder_path)

    def _on_window_resized(self, *args):
        if not self._auto_hide_busy:
            GLib.idle_add(self._check_auto_hide)

    def _check_auto_hide(self):
        if self._auto_hide_busy:
            return False
        w = self.get_width()
        if w <= 0:
            return False
        self._auto_hide_busy = True
        # Hide/show dock and locate buttons based on 770px threshold
        self._dock_btn.set_visible(w >= 770)
        self._locate_btn.set_visible(
            w >= 770 and self._playlist
            and 0 <= self._current_index < len(self._playlist)
        )
        if w < 900 and self._browse_visible:
            self._on_toggle_browse(self._dock_btn, auto=True)
        elif w >= 900 and not self._browse_visible and not self._browse_manual_closed:
            self._on_toggle_browse(self._dock_btn, auto=True)
        self._auto_hide_busy = False
        return False

    def _on_cycle_repeat(self, button):
        self._repeat_mode = (self._repeat_mode + 1) % 3
        self._repeat_btn.set_icon_name(REPEAT_ICONS[self._repeat_mode])
        tooltips = [_('Consecutive'), _('Repeat Once'), _('Repeat Indefinitely')]
        self._repeat_btn.set_tooltip_text(tooltips[self._repeat_mode])

    def _on_volume_changed(self, scale):
        val = scale.get_value()
        self._player.volume = val
        if val == 0:
            icon = 'speaker-0-symbolic'
        elif val < 0.33:
            icon = 'speaker-1-symbolic'
        elif val < 0.66:
            icon = 'speaker-2-symbolic'
        else:
            icon = 'speaker-3-symbolic'
        self._vol_btn.set_icon_name(icon)

    def _on_seek(self, scale, scroll_type, value):
        self._last_seek_time = GLib.get_monotonic_time()
        upper = self._seek_scale.get_adjustment().get_upper()
        if upper > 0:
            self._player.seek(max(0, min(value, upper)))
        return False

    @staticmethod
    def _format_time(secs):
        m = int(secs) // 60
        s = int(secs) % 60
        return f'{m}:{s:02d}'

    def do_close_request(self):
        self._shutdown.set()
        self._player.cleanup()
        self._db.close()
        return False

