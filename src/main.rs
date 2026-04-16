// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

mod application;
mod config;
mod cover_picture;
mod file_item;
mod grid_cover;
mod library_db;
mod player;
mod preferences;
mod window;

use gettextrs::{bindtextdomain, setlocale, textdomain, LocaleCategory};
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    // i18n
    setlocale(LocaleCategory::LcAll, "");
    bindtextdomain("folderplay", config::LOCALEDIR).ok();
    textdomain("folderplay").ok();

    // GStreamer
    if let Err(e) = gst::init() {
        eprintln!("Failed to initialise GStreamer: {e}");
        return glib::ExitCode::FAILURE;
    }

    // GResources
    gio::resources_register_include!("folderplay.gresource")
        .expect("Failed to register resources");

    let app = application::FolderplayApplication::new();
    app.setup_actions();
    app.run()
}
