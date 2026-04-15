// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;

use crate::config;
use crate::preferences::FolderplayPreferences;
use crate::window::FolderplayWindow;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FolderplayApplication;

    #[glib::object_subclass]
    impl ObjectSubclass for FolderplayApplication {
        const NAME: &'static str = "FolderplayApplication";
        type Type = super::FolderplayApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for FolderplayApplication {
        fn constructed(&self) {
            self.parent_constructed();
            let app = self.obj();
            app.set_accels_for_action("win.play-pause", &["space"]);
            app.set_accels_for_action("win.next-track", &["Right"]);
            app.set_accels_for_action("win.prev-track", &["Left"]);
            app.set_accels_for_action("win.open-folder", &["<Control>o"]);
            app.set_accels_for_action("app.quit", &["<Control>q"]);
        }
    }

    impl ApplicationImpl for FolderplayApplication {
        fn startup(&self) {
            self.parent_startup();
            let display = gdk::Display::default().unwrap();
            let icon_theme = gtk::IconTheme::for_display(&display);
            icon_theme.add_resource_path("/org/gnome/folderplay/icons");
        }

        fn activate(&self) {
            let app = self.obj();
            let win = app.active_window().unwrap_or_else(|| {
                let w = FolderplayWindow::new(&app);
                w.upcast()
            });

            let settings = gio::Settings::new(config::APP_ID);
            let scheme = settings.int("color-scheme");
            app.style_manager().set_color_scheme(
                match scheme {
                    1 => adw::ColorScheme::ForceLight,
                    4 => adw::ColorScheme::ForceDark,
                    _ => adw::ColorScheme::Default,
                },
            );

            win.present();
        }

        fn open(&self, files: &[gio::File], _hint: &str) {
            self.activate();
            if let Some(file) = files.first() {
                if let Some(path) = file.path() {
                    if let Some(win) = self.obj().active_window() {
                        if let Some(win) = win.downcast_ref::<FolderplayWindow>() {
                            win.open_file(&path.to_string_lossy());
                        }
                    }
                }
            }
        }
    }

    impl GtkApplicationImpl for FolderplayApplication {}
    impl AdwApplicationImpl for FolderplayApplication {}
}

glib::wrapper! {
    pub struct FolderplayApplication(ObjectSubclass<imp::FolderplayApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl FolderplayApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .property("resource-base-path", "/org/gnome/folderplay")
            .build()
    }

    pub fn setup_actions(&self) {
        let app = self.clone();
        let quit = gio::SimpleAction::new("quit", None);
        quit.connect_activate(move |_, _| app.quit());
        self.add_action(&quit);

        let about = gio::SimpleAction::new("about", None);
        let app = self.clone();
        about.connect_activate(move |_, _| app.on_about());
        self.add_action(&about);

        let shortcuts = gio::SimpleAction::new("shortcuts", None);
        let app = self.clone();
        shortcuts.connect_activate(move |_, _| app.on_shortcuts());
        self.add_action(&shortcuts);

        let prefs = gio::SimpleAction::new("preferences", None);
        let app = self.clone();
        prefs.connect_activate(move |_, _| app.on_preferences());
        self.add_action(&prefs);
    }

    fn on_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("FolderPlay")
            .application_icon(config::APP_ID)
            .developer_name("Juan Carlos Bernal")
            .version(config::VERSION)
            .translator_credits(&gettext("translator-credits"))
            .developers(["Juan Carlos Bernal"])
            .copyright("© 2026 Juan Carlos Bernal")
            .comments(&{
                let p1 = gettext(
                    "FolderPlay is a minimalist music player developed in Rust and GTK4. Its philosophy is simple: letting you enjoy your local collection by faithfully respecting your disk's folder structure\u{2014}it doesn't group by artist, album, or genre. It simply respects your order.",
                );
                let p2 = gettext(
                    "Designed with a strong focus on Lossless Hi-Res audio playback.",
                );
                format!("{p1}\n\n{p2}")
            })
            .license_type(gtk::License::Gpl30)
            .build();

        if let Some(win) = self.active_window() {
            about.present(Some(&win));
        }
    }

    fn on_shortcuts(&self) {
        let builder =
            gtk::Builder::from_resource("/org/gnome/folderplay/shortcuts-dialog.ui");
        let dialog: gtk::ShortcutsWindow = builder.object("shortcuts_dialog").unwrap();
        if let Some(win) = self.active_window() {
            dialog.set_transient_for(Some(&win));
        }
        dialog.present();
    }

    fn on_preferences(&self) {
        let prefs = FolderplayPreferences::new();
        if let Some(win) = self.active_window() {
            prefs.present(Some(&win));
        }
    }
}
