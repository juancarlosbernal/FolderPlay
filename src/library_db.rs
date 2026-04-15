// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static DB_VERSION: i32 = 3;

static SCHEMA: &str = "
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
";

pub static AUDIO_EXTENSIONS: &[&str] = &[
    ".mp3", ".flac", ".ogg", ".opus", ".wav", ".aac", ".m4a",
    ".wma", ".aiff", ".aif", ".ape", ".wv", ".mka", ".oga",
    ".dsf", ".dff",
];

pub static COVER_NAMES: &[&str] = &[
    "cover.jpg", "cover.png", "Cover.jpg", "Cover.png",
    "folder.jpg", "folder.png", "Folder.jpg", "Folder.png",
    "front.jpg", "front.png", "Front.jpg", "Front.png",
    "album.jpg", "album.png", "Album.jpg", "Album.png",
    "art.jpg", "art.png",
];

pub fn is_audio_ext(ext: &str) -> bool {
    let lower = ext.to_lowercase();
    let dotted = if lower.starts_with('.') { lower } else { format!(".{lower}") };
    AUDIO_EXTENSIONS.iter().any(|e| *e == dotted)
}

fn db_path() -> PathBuf {
    let data_dir = glib::user_data_dir().join("folderplay");
    std::fs::create_dir_all(&data_dir).ok();
    data_dir.join("library.db")
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug)]
pub struct SongRecord {
    pub path: String,
    pub folder: String,
    pub name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub format_type: String,
    pub bitrate: i32,
    pub sample_rate: i32,
    pub bits_per_sample: i32,
    pub duration: f64,
    pub meta_scanned: bool,
}

#[derive(Debug)]
pub struct FolderRecord {
    pub path: String,
    pub name: String,
    pub has_audio: bool,
}

pub struct LibraryDB {
    conn: Mutex<Connection>,
}

impl LibraryDB {
    pub fn new(path: Option<&Path>) -> Self {
        let db_file = path.map(|p| p.to_path_buf()).unwrap_or_else(db_path);
        let conn = Connection::open(&db_file).expect("Failed to open library DB");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;").ok();
        let db = LibraryDB { conn: Mutex::new(conn) };
        db.init_schema();
        db
    }

    fn init_schema(&self) {
        let conn = self.conn.lock().unwrap();
        let has_meta: bool = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='db_meta'",
                [],
                |r| r.get::<_, i32>(0),
            )
            .unwrap_or(0) > 0;

        if has_meta {
            let ver: Option<i32> = conn
                .query_row("SELECT value FROM db_meta WHERE key='version'", [], |r| {
                    r.get::<_, String>(0)
                })
                .ok()
                .and_then(|v| v.parse().ok());
            if ver != Some(DB_VERSION) {
                conn.execute_batch(
                    "DROP TABLE IF EXISTS song_covers;
                     DROP TABLE IF EXISTS covers;
                     DROP TABLE IF EXISTS songs;
                     DROP TABLE IF EXISTS folders;
                     DROP TABLE IF EXISTS db_meta;",
                ).ok();
            }
        }
        conn.execute_batch(SCHEMA).expect("Failed to init schema");
        let ver: Option<String> = conn
            .query_row("SELECT value FROM db_meta WHERE key='version'", [], |r| r.get(0))
            .ok();
        if ver.is_none() {
            conn.execute(
                "INSERT INTO db_meta (key, value) VALUES ('version', ?1)",
                params![DB_VERSION.to_string()],
            ).ok();
        }
    }

    // ── Folder queries ─────────────────────────────────────────────

    pub fn folder_needs_rescan(&self, path: &str) -> bool {
        let current_mtime = match std::fs::metadata(path) {
            Ok(m) => m.modified().ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            Err(_) => return true,
        };
        let conn = self.conn.lock().unwrap();
        let stored: Option<f64> = conn
            .query_row("SELECT mtime FROM folders WHERE path=?1", params![path], |r| r.get(0))
            .ok();
        match stored {
            Some(mt) => (current_mtime - mt).abs() > 0.001,
            None => true,
        }
    }

    pub fn get_folder_children(&self, parent_path: &str) -> Vec<FolderRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT path, name, has_audio FROM folders WHERE parent=?1 ORDER BY name COLLATE NOCASE")
            .unwrap();
        stmt.query_map(params![parent_path], |r| {
            Ok(FolderRecord {
                path: r.get(0)?,
                name: r.get(1)?,
                has_audio: r.get::<_, i32>(2)? != 0,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn get_folder_songs(&self, folder_path: &str) -> Vec<SongRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT path,folder,name,title,artist,album,year,format_type,\
                 bitrate,sample_rate,bits_per_sample,duration,mtime,meta_scanned \
                 FROM songs WHERE folder=?1 ORDER BY name COLLATE NOCASE",
            )
            .unwrap();
        stmt.query_map(params![folder_path], |r| Self::row_to_song(r))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    fn row_to_song(r: &rusqlite::Row) -> rusqlite::Result<SongRecord> {
        Ok(SongRecord {
            path: r.get(0)?,
            folder: r.get(1)?,
            name: r.get(2)?,
            title: r.get(3)?,
            artist: r.get(4)?,
            album: r.get(5)?,
            year: r.get(6)?,
            format_type: r.get(7)?,
            bitrate: r.get(8)?,
            sample_rate: r.get(9)?,
            bits_per_sample: r.get(10)?,
            duration: r.get(11)?,
            meta_scanned: r.get::<_, i32>(13)? != 0,
        })
    }

    pub fn get_playlist(&self, root_paths: &[String], exclude_artist: Option<&str>) -> Vec<String> {
        if root_paths.is_empty() {
            return vec![];
        }
        let conn = self.conn.lock().unwrap();
        let (root_cond, mut root_params) = Self::root_conditions(root_paths);
        let mut sql = format!(
            "SELECT path FROM songs WHERE ({root_cond})"
        );
        if let Some(artist) = exclude_artist {
            sql.push_str(" AND LOWER(COALESCE(artist,'')) NOT LIKE ?");
            root_params.push(format!("%{}%", artist.to_lowercase()));
        }
        sql.push_str(" ORDER BY path COLLATE NOCASE");
        let mut stmt = conn.prepare(&sql).unwrap();
        let params: Vec<&dyn rusqlite::ToSql> = root_params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    fn root_conditions(root_paths: &[String]) -> (String, Vec<String>) {
        let mut parts = Vec::new();
        let mut params = Vec::new();
        for p in root_paths {
            parts.push("folder = ? OR folder LIKE ?".to_string());
            params.push(p.clone());
            params.push(format!("{}/%", p.trim_end_matches('/')));
        }
        (parts.join(" OR "), params)
    }

    // ── Scanning / writing ─────────────────────────────────────────

    pub fn scan_folder(&self, path: &str, parent: Option<&str>) -> (Vec<FolderRecord>, Vec<SongScanEntry>) {
        let current_mtime = match std::fs::metadata(path) {
            Ok(m) => m.modified().ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
            Err(_) => return (vec![], vec![]),
        };
        let name = Path::new(path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();

        let mut subfolders = Vec::new();
        let mut audio_files: Vec<SongScanEntry> = Vec::new();

        let mut entries: Vec<_> = match std::fs::read_dir(path) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
            Err(_) => return (vec![], vec![]),
        };
        entries.sort_by(|a, b| {
            let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
            match (a_dir, b_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.file_name().to_string_lossy().to_lowercase().cmp(
                    &b.file_name().to_string_lossy().to_lowercase(),
                ),
            }
        });

        for entry in &entries {
            let ename = entry.file_name().to_string_lossy().into_owned();
            if ename.starts_with('.') {
                continue;
            }
            let epath = entry.path();
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                subfolders.push(FolderRecord {
                    path: epath.to_string_lossy().into_owned(),
                    name: ename.clone(),
                    has_audio: false,
                });
            } else if ft.is_file() {
                if let Some(ext) = epath.extension() {
                    if is_audio_ext(&ext.to_string_lossy()) {
                        let file_mtime = entry.metadata().ok()
                            .and_then(|m| m.modified().ok())
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs_f64())
                            .unwrap_or(0.0);
                        let stem = epath.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                        let fmt = ext.to_string_lossy().to_uppercase();
                        audio_files.push(SongScanEntry {
                            path: epath.to_string_lossy().into_owned(),
                            name: ename.clone(),
                            format_type: fmt,
                            title: stem,
                            mtime: file_mtime,
                        });
                    }
                }
            }
        }

        // Upsert to DB
        {
            let conn = self.conn.lock().unwrap();
            let now = now_secs();

            conn.execute(
                "INSERT INTO folders (path,parent,name,mtime,has_audio,scanned_at) \
                 VALUES (?1,?2,?3,?4,?5,?6) \
                 ON CONFLICT(path) DO UPDATE SET \
                 parent=excluded.parent,name=excluded.name,\
                 mtime=excluded.mtime,has_audio=excluded.has_audio,\
                 scanned_at=excluded.scanned_at",
                params![path, parent, name, current_mtime, !audio_files.is_empty() as i32, now],
            ).ok();

            for sf in &subfolders {
                conn.execute(
                    "INSERT INTO folders (path,parent,name,mtime,has_audio,scanned_at) \
                     VALUES (?1,?2,?3,0,0,0) \
                     ON CONFLICT(path) DO UPDATE SET parent=excluded.parent,name=excluded.name",
                    params![sf.path, path, sf.name],
                ).ok();
            }

            // Remove songs no longer on disk
            let current_paths: HashSet<&str> = audio_files.iter().map(|s| s.path.as_str()).collect();
            let mut stmt = conn.prepare("SELECT path FROM songs WHERE folder=?1").unwrap();
            let existing: Vec<String> = stmt.query_map(params![path], |r| r.get(0))
                .unwrap().filter_map(|r| r.ok()).collect();
            for ep in &existing {
                if !current_paths.contains(ep.as_str()) {
                    conn.execute("DELETE FROM songs WHERE path=?1", params![ep]).ok();
                }
            }

            // Remove subfolders no longer on disk
            let current_sf_paths: HashSet<&str> = subfolders.iter().map(|s| s.path.as_str()).collect();
            let mut stmt = conn.prepare("SELECT path FROM folders WHERE parent=?1").unwrap();
            let existing_subs: Vec<String> = stmt.query_map(params![path], |r| r.get(0))
                .unwrap().filter_map(|r| r.ok()).collect();
            for ep in &existing_subs {
                if !current_sf_paths.contains(ep.as_str()) {
                    Self::delete_folder_recursive_inner(&conn, ep);
                }
            }

            // Upsert songs
            for sf in &audio_files {
                let existing_song: Option<(f64, i32)> = conn
                    .query_row(
                        "SELECT mtime, meta_scanned FROM songs WHERE path=?1",
                        params![sf.path],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .ok();
                if let Some((mt, _)) = existing_song {
                    if (mt - sf.mtime).abs() < 0.001 {
                        continue;
                    }
                }
                conn.execute(
                    "INSERT INTO songs (path,folder,name,title,format_type,mtime) \
                     VALUES (?1,?2,?3,?4,?5,?6) \
                     ON CONFLICT(path) DO UPDATE SET \
                     folder=excluded.folder,name=excluded.name,\
                     title=excluded.title,format_type=excluded.format_type,\
                     mtime=excluded.mtime,meta_scanned=0",
                    params![sf.path, path, sf.name, sf.title, sf.format_type, sf.mtime],
                ).ok();
            }
        }

        (subfolders, audio_files)
    }

    fn delete_folder_recursive_inner(conn: &Connection, path: &str) {
        let children: Vec<String> = conn
            .prepare("SELECT path FROM folders WHERE parent=?1")
            .unwrap()
            .query_map(params![path], |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        for c in children {
            Self::delete_folder_recursive_inner(conn, &c);
        }
        conn.execute("DELETE FROM songs WHERE folder=?1", params![path]).ok();
        conn.execute("DELETE FROM song_covers WHERE song_path IN (SELECT path FROM songs WHERE folder=?1)", params![path]).ok();
        conn.execute("DELETE FROM covers WHERE folder_path=?1", params![path]).ok();
        conn.execute("DELETE FROM folders WHERE path=?1", params![path]).ok();
    }

    pub fn scan_folder_deep(&self, path: &str, parent: Option<&str>) {
        self.scan_folder_deep_inner(path, parent, &mut 0u32, None);
        self.update_has_audio_recursive(path);
    }

    /// Update the `has_audio` flag of a single folder based on what is currently
    /// stored in the DB for its direct songs and its immediate child folders.
    /// Useful after `scan_folder` (non-deep) to get an accurate flag without
    /// a full recursive walk.
    pub fn update_has_audio_for_folder(&self, path: &str) {
        let conn = self.conn.lock().unwrap();
        let has_direct: bool = conn
            .query_row("SELECT COUNT(*) FROM songs WHERE folder=?1", params![path], |r| r.get::<_, i32>(0))
            .unwrap_or(0) > 0;
        let has_child: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE parent=?1 AND has_audio=1",
                params![path],
                |r| r.get::<_, i32>(0),
            )
            .unwrap_or(0) > 0;
        conn.execute(
            "UPDATE folders SET has_audio=?1 WHERE path=?2",
            params![(has_direct || has_child) as i32, path],
        ).ok();
    }

    pub fn scan_folder_deep_with_progress<F>(
        &self,
        path: &str,
        parent: Option<&str>,
        progress_cb: F,
    ) where
        F: Fn(u32) + Send + Sync,
    {
        let mut count = 0u32;
        let cb: &dyn Fn(u32) = &progress_cb;
        self.scan_folder_deep_inner(path, parent, &mut count, Some(cb));
        self.update_has_audio_recursive(path);
    }

    fn scan_folder_deep_inner(
        &self,
        path: &str,
        parent: Option<&str>,
        count: &mut u32,
        progress_cb: Option<&dyn Fn(u32)>,
    ) {
        if !Path::new(path).is_dir() {
            return;
        }
        let (subfolders, songs) = self.scan_folder(path, parent);
        *count += songs.len() as u32;
        if let Some(cb) = progress_cb {
            cb(*count);
        }
        for sf in &subfolders {
            self.scan_folder_deep_inner(&sf.path, Some(path), count, progress_cb);
        }
    }

    fn update_has_audio_recursive(&self, path: &str) {
        let children: Vec<String> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT path FROM folders WHERE parent=?1")
                .unwrap();
            stmt.query_map(params![path], |r| r.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for c in &children {
            self.update_has_audio_recursive(c);
        }
        let conn = self.conn.lock().unwrap();
        let has_direct: bool = conn
            .query_row("SELECT COUNT(*) FROM songs WHERE folder=?1", params![path], |r| r.get::<_, i32>(0))
            .unwrap_or(0) > 0;
        let has_child: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM folders WHERE parent=?1 AND has_audio=1",
                params![path],
                |r| r.get::<_, i32>(0),
            )
            .unwrap_or(0) > 0;
        conn.execute(
            "UPDATE folders SET has_audio=?1 WHERE path=?2",
            params![(has_direct || has_child) as i32, path],
        ).ok();
    }

    pub fn update_song_metadata(
        &self, path: &str, title: &str, artist: &str, album: &str,
        year: &str, bitrate: i32, sample_rate: i32, bits_per_sample: i32, duration: f64,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE songs SET title=?1,artist=?2,album=?3,year=?4,\
             bitrate=?5,sample_rate=?6,bits_per_sample=?7,duration=?8,\
             meta_scanned=1 WHERE path=?9",
            params![title, artist, album, year, bitrate, sample_rate, bits_per_sample, duration, path],
        ).ok();
    }

    pub fn get_unscanned_songs(&self, root_paths: &[String], limit: i32) -> Vec<String> {
        if root_paths.is_empty() {
            return vec![];
        }
        let conn = self.conn.lock().unwrap();
        let (root_cond, root_params) = Self::root_conditions(root_paths);
        let mut all_params: Vec<String> = root_params;
        all_params.push(limit.to_string());
        let sql = format!(
            "SELECT path FROM songs WHERE meta_scanned=0 AND ({root_cond}) LIMIT ?"
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let p: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        stmt.query_map(p.as_slice(), |r| r.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    pub fn count_songs(&self, root_paths: &[String]) -> i32 {
        if root_paths.is_empty() {
            return 0;
        }
        let conn = self.conn.lock().unwrap();
        let (root_cond, root_params) = Self::root_conditions(root_paths);
        let sql = format!("SELECT COUNT(*) FROM songs WHERE {root_cond}");
        let mut stmt = conn.prepare(&sql).unwrap();
        let p: Vec<&dyn rusqlite::ToSql> = root_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        stmt.query_row(p.as_slice(), |r| r.get(0)).unwrap_or(0)
    }

    // ── Cover art cache ────────────────────────────────────────────

    pub fn get_cover(&self, folder_path: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT data FROM covers WHERE folder_path=?1",
            params![folder_path],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .ok()
        .flatten()
    }

    pub fn set_cover(&self, folder_path: &str, data: &[u8], source: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO covers (folder_path,source,data) VALUES (?1,?2,?3) \
             ON CONFLICT(folder_path) DO UPDATE SET source=excluded.source,data=excluded.data",
            params![folder_path, source, data],
        ).ok();
    }

    pub fn has_cover(&self, folder_path: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT 1 FROM covers WHERE folder_path=?1", params![folder_path], |_| Ok(()))
            .is_ok()
    }

    /// Stores a NULL sentinel so repeated calls to `get_folder_preview` skip
    /// the expensive GStreamer discovery for folders confirmed to have no cover.
    pub fn mark_no_cover(&self, folder_path: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO covers (folder_path, source, data) VALUES (?1, 'none', NULL) \
             ON CONFLICT(folder_path) DO NOTHING",
            params![folder_path],
        ).ok();
    }

    pub fn get_song_cover(&self, song_path: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT data FROM song_covers WHERE song_path=?1",
            params![song_path],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .ok()
        .flatten()
    }

    /// Batch-load all song covers for songs in a given folder (single query).
    pub fn get_song_covers_for_folder(&self, folder_path: &str) -> HashMap<String, Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT sc.song_path, sc.data FROM song_covers sc \
             INNER JOIN songs s ON s.path = sc.song_path \
             WHERE s.folder = ?1"
        ).unwrap();
        let mut map = HashMap::new();
        let rows = stmt.query_map(params![folder_path], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        }).unwrap();
        for row in rows.flatten() {
            map.insert(row.0, row.1);
        }
        map
    }

    pub fn set_song_cover(&self, song_path: &str, data: &[u8]) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO song_covers (song_path,data) VALUES (?1,?2) \
             ON CONFLICT(song_path) DO UPDATE SET data=excluded.data",
            params![song_path, data],
        ).ok();
    }

    /// Return the cover data of the first song in a folder that has one.
    pub fn get_first_song_cover_in_folder(&self, folder_path: &str) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT sc.data FROM song_covers sc \
             INNER JOIN songs s ON s.path = sc.song_path \
             WHERE s.folder = ?1 LIMIT 1",
            params![folder_path],
            |r| r.get::<_, Vec<u8>>(0),
        ).ok()
    }

    // ── Search ─────────────────────────────────────────────────────

    pub fn search_songs(&self, words: &[String], root_paths: &[String]) -> Vec<SongRecord> {
        if root_paths.is_empty() || words.is_empty() {
            return vec![];
        }
        let conn = self.conn.lock().unwrap();
        let (root_cond, mut all_params) = Self::root_conditions(root_paths);

        let mut word_conds = Vec::new();
        for w in words {
            let pattern = format!("%{w}%");
            word_conds.push("(title LIKE ? OR artist LIKE ? OR album LIKE ? OR name LIKE ?)".to_string());
            for _ in 0..4 {
                all_params.push(pattern.clone());
            }
        }
        let sql = format!(
            "SELECT path,folder,name,title,artist,album,year,format_type,\
             bitrate,sample_rate,bits_per_sample,duration,mtime,meta_scanned \
             FROM songs WHERE ({root_cond}) AND {} ORDER BY title COLLATE NOCASE LIMIT 200",
            word_conds.join(" AND ")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        let p: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();
        stmt.query_map(p.as_slice(), |r| Self::row_to_song(r))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    // ── Cleanup ────────────────────────────────────────────────────

    pub fn remove_root(&self, root_path: &str) {
        let conn = self.conn.lock().unwrap();
        Self::delete_folder_recursive_inner(&conn, root_path);
    }

    pub fn clear_all(&self) {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "DELETE FROM song_covers; DELETE FROM covers; DELETE FROM songs; DELETE FROM folders;",
        ).ok();
    }
}

#[derive(Debug)]
pub struct SongScanEntry {
    pub path: String,
    pub name: String,
    pub format_type: String,
    pub title: String,
    pub mtime: f64,
}
