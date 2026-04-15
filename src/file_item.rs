// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use glib::subclass::prelude::*;
use std::cell::RefCell;

// ── imp ────────────────────────────────────────────────────────────
mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FileItem {
        pub path: RefCell<String>,
        pub name: RefCell<String>,
        pub is_folder: RefCell<bool>,
        pub title: RefCell<String>,
        pub artist: RefCell<String>,
        pub album: RefCell<String>,
        pub year: RefCell<String>,
        pub format_type: RefCell<String>,
        pub bitrate: RefCell<i32>,
        pub sample_rate: RefCell<i32>,
        pub bits_per_sample: RefCell<i32>,
        pub duration: RefCell<f64>,
        pub cover_thumb: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileItem {
        const NAME: &'static str = "FileItem";
        type Type = super::FileItem;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for FileItem {}
}

// ── wrapper ────────────────────────────────────────────────────────
glib::wrapper! {
    pub struct FileItem(ObjectSubclass<imp::FileItem>);
}

impl FileItem {
    pub fn new(path: &str, name: &str, is_folder: bool) -> Self {
        let obj: Self = glib::Object::builder().build();
        let imp = obj.imp();
        *imp.path.borrow_mut() = path.to_string();
        *imp.name.borrow_mut() = name.to_string();
        *imp.is_folder.borrow_mut() = is_folder;

        if !is_folder {
            if let Some(stem) = std::path::Path::new(name).file_stem() {
                *imp.title.borrow_mut() = stem.to_string_lossy().into_owned();
            }
            if let Some(ext) = std::path::Path::new(name).extension() {
                *imp.format_type.borrow_mut() = ext.to_string_lossy().to_uppercase();
            }
        }
        obj
    }

    pub fn path(&self) -> String {
        self.imp().path.borrow().clone()
    }
    pub fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }
    pub fn is_folder(&self) -> bool {
        *self.imp().is_folder.borrow()
    }
    pub fn title(&self) -> String {
        self.imp().title.borrow().clone()
    }
    pub fn set_title(&self, v: &str) {
        *self.imp().title.borrow_mut() = v.to_string();
    }
    pub fn artist(&self) -> String {
        self.imp().artist.borrow().clone()
    }
    pub fn set_artist(&self, v: &str) {
        *self.imp().artist.borrow_mut() = v.to_string();
    }
    pub fn album(&self) -> String {
        self.imp().album.borrow().clone()
    }
    pub fn set_album(&self, v: &str) {
        *self.imp().album.borrow_mut() = v.to_string();
    }
    pub fn year(&self) -> String {
        self.imp().year.borrow().clone()
    }
    pub fn set_year(&self, v: &str) {
        *self.imp().year.borrow_mut() = v.to_string();
    }
    pub fn format_type(&self) -> String {
        self.imp().format_type.borrow().clone()
    }
    pub fn set_format_type(&self, v: &str) {
        *self.imp().format_type.borrow_mut() = v.to_string();
    }
    pub fn bitrate(&self) -> i32 {
        *self.imp().bitrate.borrow()
    }
    pub fn set_bitrate(&self, v: i32) {
        *self.imp().bitrate.borrow_mut() = v;
    }
    pub fn sample_rate(&self) -> i32 {
        *self.imp().sample_rate.borrow()
    }
    pub fn set_sample_rate(&self, v: i32) {
        *self.imp().sample_rate.borrow_mut() = v;
    }
    pub fn bits_per_sample(&self) -> i32 {
        *self.imp().bits_per_sample.borrow()
    }
    pub fn set_bits_per_sample(&self, v: i32) {
        *self.imp().bits_per_sample.borrow_mut() = v;
    }
    pub fn duration(&self) -> f64 {
        *self.imp().duration.borrow()
    }
    pub fn set_duration(&self, v: f64) {
        *self.imp().duration.borrow_mut() = v;
    }
    pub fn cover_thumb(&self) -> Option<gdk::Texture> {
        self.imp().cover_thumb.borrow().clone()
    }
    pub fn set_cover_thumb(&self, v: Option<gdk::Texture>) {
        *self.imp().cover_thumb.borrow_mut() = v;
    }
}
