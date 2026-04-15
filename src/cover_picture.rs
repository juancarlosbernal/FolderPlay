// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::cell::RefCell;

const COVER_SIZE: i32 = 300;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CoverPicture {
        pub paintable: RefCell<Option<gdk::Paintable>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverPicture {
        const NAME: &'static str = "CoverPicture";
        type Type = super::CoverPicture;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CoverPicture {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_overflow(gtk::Overflow::Hidden);
        }
    }

    impl WidgetImpl for CoverPicture {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let _ = orientation;
            (COVER_SIZE, COVER_SIZE, -1, -1)
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let paintable = self.paintable.borrow();
            let paintable = match paintable.as_ref() {
                Some(p) => p,
                None => return,
            };
            let w = self.obj().width() as f64;
            let h = self.obj().height() as f64;
            if w <= 0.0 || h <= 0.0 {
                return;
            }
            let iw = paintable.intrinsic_width() as f64;
            let ih = paintable.intrinsic_height() as f64;
            let iw = if iw <= 0.0 { w } else { iw };
            let ih = if ih <= 0.0 { h } else { ih };
            let scale = (w / iw).max(h / ih);
            let sw = iw * scale;
            let sh = ih * scale;
            let x = (w - sw) / 2.0;
            let y = (h - sh) / 2.0;

            let rect = graphene::Rect::new(0.0, 0.0, w as f32, h as f32);
            let point = graphene::Point::new(x as f32, y as f32);

            snapshot.save();
            snapshot.push_clip(&rect);
            snapshot.translate(&point);
            paintable.snapshot(snapshot.upcast_ref::<gdk::Snapshot>(), sw, sh);
            snapshot.pop();
            snapshot.restore();
        }
    }
}

glib::wrapper! {
    pub struct CoverPicture(ObjectSubclass<imp::CoverPicture>)
        @extends gtk::Widget;
}

impl CoverPicture {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_paintable(&self, paintable: Option<&gdk::Paintable>) {
        *self.imp().paintable.borrow_mut() = paintable.cloned();
        self.queue_draw();
    }
}
