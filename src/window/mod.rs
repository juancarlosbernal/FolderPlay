// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

mod imp;

use glib::subclass::prelude::ObjectSubclassIsExt;

glib::wrapper! {
    pub struct FolderplayWindow(ObjectSubclass<imp::FolderplayWindow>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl FolderplayWindow {
    pub fn new(app: &crate::application::FolderplayApplication) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    pub fn open_file(&self, path: &str) {
        self.imp().open_file(path);
    }
}
