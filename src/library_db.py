# Copyright (c) 2026 Juan Carlos Bernal
#
# SPDX-License-Identifier: GPL-3.0-or-later

"""Persistent SQLite library cache for FolderPlay.

Strategy inspired by Lollypop, MPD and Beets:
- One SQLite DB at XDG_DATA_HOME/folderplay/library.db
- Tables: folders, songs, covers
- Incremental updates via mtime comparison — only re-scan folders
  whose mtime changed since last scan
- Cover art stored as BLOB for instant retrieval
- Thread-safe: one connection per thread via check_same_thread=False
  with a threading lock for writes
"""

import os
import sqlite3
import threading

from gi.repository import GLib

_DB_VERSION = 2

_SCHEMA = """
CREATE TABLE IF NOT EXISTS db_meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS folders (
    path       TEXT PRIMARY KEY,
    parent     TEXT,
    name       TEXT NOT NULL,
    mtime      REAL NOT NULL DEFAULT 0,
    has_audio  INTEGER NOT NULL DEFAULT 0,
    scanned_at REAL NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS songs (
    path            TEXT PRIMARY KEY,
    folder          TEXT NOT NULL,
    name            TEXT NOT NULL,
    title           TEXT NOT NULL DEFAULT '',
    artist          TEXT NOT NULL DEFAULT '',
    album           TEXT NOT NULL DEFAULT '',
    year            TEXT NOT NULL DEFAULT '',
    format_type     TEXT NOT NULL DEFAULT '',
    bitrate         INTEGER NOT NULL DEFAULT 0,
    sample_rate     INTEGER NOT NULL DEFAULT 0,
    bits_per_sample INTEGER NOT NULL DEFAULT 0,
    duration        REAL NOT NULL DEFAULT 0.0,
    mtime           REAL NOT NULL DEFAULT 0,
    meta_scanned    INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (folder) REFERENCES folders(path) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS covers (
    folder_path TEXT PRIMARY KEY,
    source      TEXT NOT NULL DEFAULT '',
    data        BLOB,
    FOREIGN KEY (folder_path) REFERENCES folders(path) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS song_covers (
    song_path TEXT PRIMARY KEY,
    data      BLOB NOT NULL,
    FOREIGN KEY (song_path) REFERENCES songs(path) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_songs_folder ON songs(folder);
CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent);
CREATE INDEX IF NOT EXISTS idx_songs_title ON songs(title COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_songs_artist ON songs(artist COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_songs_album ON songs(album COLLATE NOCASE);
"""

AUDIO_EXTENSIONS = {
    '.mp3', '.flac', '.ogg', '.opus', '.wav', '.aac', '.m4a',
    '.wma', '.aiff', '.aif', '.ape', '.wv', '.mka', '.oga',
    '.dsf', '.dff',
}

COVER_NAMES = [
    'cover.jpg', 'cover.png', 'Cover.jpg', 'Cover.png',
    'folder.jpg', 'folder.png', 'Folder.jpg', 'Folder.png',
    'front.jpg', 'front.png', 'Front.jpg', 'Front.png',
    'album.jpg', 'album.png', 'Album.jpg', 'Album.png',
    'art.jpg', 'art.png',
]


def _db_path():
    """Return the path to the library database file."""
    data_dir = os.path.join(
        GLib.get_user_data_dir(), 'folderplay',
    )
    os.makedirs(data_dir, exist_ok=True)
    return os.path.join(data_dir, 'library.db')


class LibraryDB:
    """Thread-safe SQLite library cache."""

    def __init__(self, path=None):
        self._path = path or _db_path()
        self._lock = threading.Lock()
        self._conn = sqlite3.connect(
            self._path,
            check_same_thread=False,
            timeout=10,
        )
        self._conn.row_factory = sqlite3.Row
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._conn.execute("PRAGMA synchronous=NORMAL")
        self._init_schema()

    def _init_schema(self):
        with self._lock:
            # Check if DB exists with a stale schema and nuke it
            cur = self._conn.execute(
                "SELECT name FROM sqlite_master "
                "WHERE type='table' AND name='db_meta'"
            )
            if cur.fetchone():
                ver_cur = self._conn.execute(
                    "SELECT value FROM db_meta WHERE key='version'"
                )
                row = ver_cur.fetchone()
                if row is None or int(row[0]) != _DB_VERSION:
                    # Schema mismatch – drop everything and recreate
                    self._conn.executescript(
                        "DROP TABLE IF EXISTS covers;"
                        "DROP TABLE IF EXISTS songs;"
                        "DROP TABLE IF EXISTS folders;"
                        "DROP TABLE IF EXISTS db_meta;"
                    )
            self._conn.executescript(_SCHEMA)
            cur = self._conn.execute(
                "SELECT value FROM db_meta WHERE key='version'"
            )
            row = cur.fetchone()
            if row is None:
                self._conn.execute(
                    "INSERT INTO db_meta (key, value) VALUES ('version', ?)",
                    (str(_DB_VERSION),),
                )
                self._conn.commit()

    def _migrate(self, from_version):
        # Future migrations go here
        self._conn.execute(
            "UPDATE db_meta SET value=? WHERE key='version'",
            (str(_DB_VERSION),),
        )
        self._conn.commit()

    def close(self):
        self._conn.close()

    # ── Folder queries ──────────────────────────────────────────────

    def folder_needs_rescan(self, path):
        """Return True if folder mtime changed since last scan."""
        try:
            current_mtime = os.stat(path).st_mtime
        except OSError:
            return True
        cur = self._conn.execute(
            "SELECT mtime FROM folders WHERE path=?", (path,),
        )
        row = cur.fetchone()
        if row is None:
            return True
        return current_mtime != row[0]

    def get_folder_children(self, parent_path):
        """Return cached subfolders of parent_path that have audio."""
        cur = self._conn.execute(
            "SELECT path, name, has_audio FROM folders "
            "WHERE parent=? ORDER BY name COLLATE NOCASE",
            (parent_path,),
        )
        return [dict(r) for r in cur.fetchall()]

    def get_folder_songs(self, folder_path):
        """Return cached songs in folder_path."""
        cur = self._conn.execute(
            "SELECT * FROM songs WHERE folder=? ORDER BY name COLLATE NOCASE",
            (folder_path,),
        )
        return [dict(r) for r in cur.fetchall()]

    def get_all_songs(self, root_paths):
        """Return all songs under given root paths."""
        if not root_paths:
            return []
        placeholders = ','.join('?' * len(root_paths))
        # Use LIKE to match songs in subfolders
        conditions = ' OR '.join(
            "folder = ? OR folder LIKE ?" for _ in root_paths
        )
        params = []
        for p in root_paths:
            params.append(p)
            params.append(p.rstrip('/') + '/%')
        cur = self._conn.execute(
            f"SELECT * FROM songs WHERE {conditions} "
            "ORDER BY path COLLATE NOCASE",
            params,
        )
        return [dict(r) for r in cur.fetchall()]

    def get_all_folders(self, root_paths):
        """Return all folders under given root paths."""
        if not root_paths:
            return []
        conditions = ' OR '.join(
            "path = ? OR path LIKE ?" for _ in root_paths
        )
        params = []
        for p in root_paths:
            params.append(p)
            params.append(p.rstrip('/') + '/%')
        cur = self._conn.execute(
            f"SELECT path, name FROM folders WHERE {conditions} "
            "ORDER BY path COLLATE NOCASE",
            params,
        )
        return [dict(r) for r in cur.fetchall()]

    def get_playlist(self, root_paths, exclude_artist=None):
        """Return ordered list of audio file paths under root_paths."""
        if not root_paths:
            return []
        conditions = ' OR '.join(
            "folder = ? OR folder LIKE ?" for _ in root_paths
        )
        params = []
        for p in root_paths:
            params.append(p)
            params.append(p.rstrip('/') + '/%')
        extra = ''
        if exclude_artist:
            extra = " AND LOWER(COALESCE(artist,'')) NOT LIKE ?"
            params.append(f'%{exclude_artist.lower()}%')
        cur = self._conn.execute(
            f"SELECT path FROM songs WHERE ({conditions}){extra} "
            "ORDER BY path COLLATE NOCASE",
            params,
        )
        return [r[0] for r in cur.fetchall()]

    # ── Scanning / writing ──────────────────────────────────────────

    def scan_folder(self, path, parent=None):
        """Scan one directory level and update DB.

        Returns (subfolders, audio_files) as lists of dicts.
        Only rescans if mtime changed.
        """
        try:
            stat = os.stat(path)
            current_mtime = stat.st_mtime
        except OSError:
            return [], []

        name = os.path.basename(path)

        subfolders = []
        audio_files = []

        try:
            entries = sorted(
                os.scandir(path),
                key=lambda e: (not e.is_dir(), e.name.lower()),
            )
        except (PermissionError, OSError):
            return [], []

        for entry in entries:
            if entry.name.startswith('.'):
                continue
            try:
                if entry.is_dir(follow_symlinks=False):
                    subfolders.append({
                        'path': entry.path,
                        'name': entry.name,
                    })
                elif entry.is_file():
                    ext = os.path.splitext(entry.name)[1].lower()
                    if ext in AUDIO_EXTENSIONS:
                        try:
                            file_mtime = entry.stat().st_mtime
                        except OSError:
                            file_mtime = 0
                        audio_files.append({
                            'path': entry.path,
                            'name': entry.name,
                            'format_type': ext[1:].upper(),
                            'title': os.path.splitext(entry.name)[0],
                            'mtime': file_mtime,
                        })
            except OSError:
                continue

        with self._lock:
            import time
            now = time.time()

            # Upsert the folder itself
            self._conn.execute(
                "INSERT INTO folders (path, parent, name, mtime, has_audio, scanned_at) "
                "VALUES (?, ?, ?, ?, ?, ?) "
                "ON CONFLICT(path) DO UPDATE SET "
                "parent=excluded.parent, name=excluded.name, "
                "mtime=excluded.mtime, has_audio=excluded.has_audio, "
                "scanned_at=excluded.scanned_at",
                (path, parent, name, current_mtime,
                 1 if audio_files else 0, now),
            )

            # Upsert subfolders
            for sf in subfolders:
                self._conn.execute(
                    "INSERT INTO folders (path, parent, name, mtime, has_audio, scanned_at) "
                    "VALUES (?, ?, ?, 0, 0, 0) "
                    "ON CONFLICT(path) DO UPDATE SET "
                    "parent=excluded.parent, name=excluded.name",
                    (sf['path'], path, sf['name']),
                )

            # Remove songs no longer on disk in this folder
            current_song_paths = {s['path'] for s in audio_files}
            existing = self._conn.execute(
                "SELECT path FROM songs WHERE folder=?", (path,),
            )
            for row in existing:
                if row[0] not in current_song_paths:
                    self._conn.execute(
                        "DELETE FROM songs WHERE path=?", (row[0],),
                    )

            # Remove subfolders no longer on disk
            current_subfolder_paths = {sf['path'] for sf in subfolders}
            existing_subs = self._conn.execute(
                "SELECT path FROM folders WHERE parent=?", (path,),
            )
            for row in existing_subs:
                if row[0] not in current_subfolder_paths:
                    self._delete_folder_recursive(row[0])

            # Upsert songs (preserve metadata if file not changed)
            for sf in audio_files:
                existing_song = self._conn.execute(
                    "SELECT mtime, meta_scanned FROM songs WHERE path=?",
                    (sf['path'],),
                ).fetchone()
                if existing_song and existing_song[0] == sf['mtime']:
                    # File unchanged, skip
                    continue
                self._conn.execute(
                    "INSERT INTO songs (path, folder, name, title, format_type, mtime) "
                    "VALUES (?, ?, ?, ?, ?, ?) "
                    "ON CONFLICT(path) DO UPDATE SET "
                    "folder=excluded.folder, name=excluded.name, "
                    "title=excluded.title, format_type=excluded.format_type, "
                    "mtime=excluded.mtime, meta_scanned=0",
                    (sf['path'], path, sf['name'], sf['title'],
                     sf['format_type'], sf['mtime']),
                )

            self._conn.commit()

        return subfolders, audio_files

    def _delete_folder_recursive(self, path):
        """Delete a folder and all its children from DB (within lock)."""
        # Delete child folders recursively
        children = self._conn.execute(
            "SELECT path FROM folders WHERE parent=?", (path,),
        )
        for row in children:
            self._delete_folder_recursive(row[0])
        # Delete songs in this folder
        self._conn.execute("DELETE FROM songs WHERE folder=?", (path,))
        # Delete covers
        self._conn.execute("DELETE FROM covers WHERE folder_path=?", (path,))
        # Delete the folder itself
        self._conn.execute("DELETE FROM folders WHERE path=?", (path,))

    def scan_folder_deep(self, path, parent=None):
        """Recursively scan a folder tree. Updates DB incrementally."""
        if not os.path.isdir(path):
            return
        subfolders, audio_files = self.scan_folder(path, parent)
        for sf in subfolders:
            self.scan_folder_deep(sf['path'], path)

        # After deep scan, update has_audio for folders that have
        # audio in descendants
        self._update_has_audio_recursive(path)

    def _update_has_audio_recursive(self, path):
        """Update has_audio flag based on descendants."""
        with self._lock:
            cur = self._conn.execute(
                "SELECT path FROM folders WHERE parent=?", (path,),
            )
            children = [r[0] for r in cur.fetchall()]

        for child in children:
            self._update_has_audio_recursive(child)

        with self._lock:
            # Check if this folder or any child has audio
            has_direct = self._conn.execute(
                "SELECT COUNT(*) FROM songs WHERE folder=?", (path,),
            ).fetchone()[0] > 0

            has_child = self._conn.execute(
                "SELECT COUNT(*) FROM folders WHERE parent=? AND has_audio=1",
                (path,),
            ).fetchone()[0] > 0

            self._conn.execute(
                "UPDATE folders SET has_audio=? WHERE path=?",
                (1 if (has_direct or has_child) else 0, path),
            )
            self._conn.commit()

    def update_song_metadata(self, path, title='', artist='', album='',
                             year='', bitrate=0, sample_rate=0,
                             bits_per_sample=0, duration=0.0):
        """Update metadata for a song after Discoverer extraction."""
        with self._lock:
            self._conn.execute(
                "UPDATE songs SET title=?, artist=?, album=?, year=?, "
                "bitrate=?, sample_rate=?, bits_per_sample=?, duration=?, "
                "meta_scanned=1 WHERE path=?",
                (title, artist, album, year, bitrate, sample_rate,
                 bits_per_sample, duration, path),
            )
            self._conn.commit()

    def get_unscanned_songs(self, root_paths, limit=100):
        """Return songs that haven't been metadata-scanned yet."""
        if not root_paths:
            return []
        conditions = ' OR '.join(
            "folder = ? OR folder LIKE ?" for _ in root_paths
        )
        params = []
        for p in root_paths:
            params.append(p)
            params.append(p.rstrip('/') + '/%')
        cur = self._conn.execute(
            f"SELECT path FROM songs WHERE meta_scanned=0 "
            f"AND ({conditions}) LIMIT ?",
            params + [limit],
        )
        return [r[0] for r in cur.fetchall()]

    def count_songs(self, root_paths):
        """Count total songs under root paths."""
        if not root_paths:
            return 0
        conditions = ' OR '.join(
            "folder = ? OR folder LIKE ?" for _ in root_paths
        )
        params = []
        for p in root_paths:
            params.append(p)
            params.append(p.rstrip('/') + '/%')
        cur = self._conn.execute(
            f"SELECT COUNT(*) FROM songs WHERE {conditions}", params,
        )
        return cur.fetchone()[0]

    # ── Cover art cache ─────────────────────────────────────────────

    def get_cover(self, folder_path):
        """Return cover art bytes for a folder, or None."""
        cur = self._conn.execute(
            "SELECT data FROM covers WHERE folder_path=?", (folder_path,),
        )
        row = cur.fetchone()
        if row and row[0]:
            return bytes(row[0])
        return None

    def set_cover(self, folder_path, data, source='file'):
        """Store cover art bytes for a folder."""
        with self._lock:
            self._conn.execute(
                "INSERT INTO covers (folder_path, source, data) "
                "VALUES (?, ?, ?) "
                "ON CONFLICT(folder_path) DO UPDATE SET "
                "source=excluded.source, data=excluded.data",
                (folder_path, source, data),
            )
            self._conn.commit()

    def has_cover(self, folder_path):
        """Return True if a cover entry exists (even if NULL data)."""
        cur = self._conn.execute(
            "SELECT 1 FROM covers WHERE folder_path=?", (folder_path,),
        )
        return cur.fetchone() is not None

    def set_no_cover(self, folder_path):
        """Mark that this folder has no cover (avoid re-scanning)."""
        with self._lock:
            self._conn.execute(
                "INSERT INTO covers (folder_path, source, data) "
                "VALUES (?, 'none', NULL) "
                "ON CONFLICT(folder_path) DO UPDATE SET "
                "source='none', data=NULL",
                (folder_path,),
            )
            self._conn.commit()

    # ── Per-song cover art ──────────────────────────────────────────

    def get_song_cover(self, song_path):
        """Return cover art bytes for a song, or None."""
        cur = self._conn.execute(
            "SELECT data FROM song_covers WHERE song_path=?", (song_path,),
        )
        row = cur.fetchone()
        if row and row[0]:
            return bytes(row[0])
        return None

    def set_song_cover(self, song_path, data):
        """Store cover art bytes for a song."""
        with self._lock:
            self._conn.execute(
                "INSERT INTO song_covers (song_path, data) VALUES (?, ?) "
                "ON CONFLICT(song_path) DO UPDATE SET data=excluded.data",
                (song_path, data),
            )
            self._conn.commit()

    # ── Folder mtime check ──────────────────────────────────────────

    def folder_has_audio_cached(self, path):
        """Return cached has_audio flag, or None if not in DB."""
        cur = self._conn.execute(
            "SELECT has_audio FROM folders WHERE path=?", (path,),
        )
        row = cur.fetchone()
        if row is not None:
            return bool(row[0])
        return None

    # ── Search ──────────────────────────────────────────────────────

    def search_songs(self, words, root_paths):
        """Search songs matching all words in title/artist/album/name."""
        if not root_paths or not words:
            return []
        root_conditions = ' OR '.join(
            "folder = ? OR folder LIKE ?" for _ in root_paths
        )
        root_params = []
        for p in root_paths:
            root_params.append(p)
            root_params.append(p.rstrip('/') + '/%')

        # Build word conditions: each word must appear in at least one field
        word_conditions = []
        word_params = []
        for w in words:
            pattern = f'%{w}%'
            word_conditions.append(
                "(title LIKE ? OR artist LIKE ? OR album LIKE ? OR name LIKE ?)"
            )
            word_params.extend([pattern] * 4)

        sql = (
            f"SELECT * FROM songs WHERE ({root_conditions}) "
            f"AND {' AND '.join(word_conditions)} "
            "ORDER BY title COLLATE NOCASE LIMIT 200"
        )
        cur = self._conn.execute(sql, root_params + word_params)
        return [dict(r) for r in cur.fetchall()]

    def search_folders(self, words, root_paths):
        """Search folders matching all words in name."""
        if not root_paths or not words:
            return []
        root_conditions = ' OR '.join(
            "path = ? OR path LIKE ?" for _ in root_paths
        )
        root_params = []
        for p in root_paths:
            root_params.append(p)
            root_params.append(p.rstrip('/') + '/%')

        word_conditions = []
        word_params = []
        for w in words:
            word_conditions.append("name LIKE ?")
            word_params.append(f'%{w}%')

        sql = (
            f"SELECT path, name FROM folders WHERE ({root_conditions}) "
            f"AND {' AND '.join(word_conditions)} "
            "ORDER BY name COLLATE NOCASE LIMIT 100"
        )
        cur = self._conn.execute(sql, root_params + word_params)
        return [dict(r) for r in cur.fetchall()]

    # ── Cleanup ─────────────────────────────────────────────────────

    def remove_root(self, root_path):
        """Remove a root folder and all its descendants from DB."""
        with self._lock:
            self._delete_folder_recursive(root_path)
            self._conn.commit()

    def clear_all(self):
        """Clear the entire library cache."""
        with self._lock:
            self._conn.execute("DELETE FROM covers")
            self._conn.execute("DELETE FROM songs")
            self._conn.execute("DELETE FROM folders")
            self._conn.commit()
