// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;

use crate::config;

static OUTPUT_VALUES: &[&str] = &["auto", "hifi", "standard"];

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct FolderplayPreferences {}

    #[glib::object_subclass]
    impl ObjectSubclass for FolderplayPreferences {
        const NAME: &'static str = "FolderplayPreferences";
        type Type = super::FolderplayPreferences;
        type ParentType = adw::PreferencesDialog;
    }

    impl ObjectImpl for FolderplayPreferences {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_title(&gettext("Preferences"));

            let settings = gio::Settings::new(config::APP_ID);

            // ── Audio ──────────────────────────────────────────────
            let audio = adw::PreferencesGroup::builder()
                .title(gettext("Audio Output"))
                .description(gettext(
                    "HiFi sends audio directly to your DAC without \
                     resampling. Standard uses PulseAudio for maximum compatibility.",
                ))
                .build();

            let output_row = adw::ComboRow::builder()
                .title(gettext("Output"))
                .build();

            let out_model = gtk::StringList::new(&[
                &gettext("Automatic"),
                &gettext("HiFi (Bit-perfect)"),
                &gettext("Standard (PulseAudio)"),
            ]);
            output_row.set_model(Some(&out_model));

            let cur = settings.string("audio-output");
            let cur_idx = OUTPUT_VALUES.iter().position(|v| *v == cur.as_str()).unwrap_or(0) as u32;
            output_row.set_selected(cur_idx);
            output_row.set_subtitle(&output_description(cur_idx));

            output_row.connect_selected_notify(|row| {
                let idx = row.selected();
                let value = OUTPUT_VALUES[idx as usize];
                row.set_subtitle(&output_description(idx));
                let s = gio::Settings::new(config::APP_ID);
                s.set_string("audio-output", value).ok();
                if let Some(app) = gio::Application::default() {
                    if let Ok(app) = app.downcast::<gtk::Application>() {
                        if let Some(win) = app.active_window() {
                            let _ = win.activate_action("apply-audio-output", None);
                        }
                    }
                }
            });
            audio.add(&output_row);

            // ── Appearance ─────────────────────────────────────────
            let appearance = adw::PreferencesGroup::builder()
                .title(gettext("Appearance"))
                .build();

            let scheme_row = adw::ComboRow::builder()
                .title(gettext("Style"))
                .subtitle(gettext("Choose between light and dark appearance"))
                .build();
            let model = gtk::StringList::new(&[
                &gettext("System"),
                &gettext("Light"),
                &gettext("Dark"),
            ]);
            scheme_row.set_model(Some(&model));

            let scheme_val = settings.int("color-scheme");
            let idx = match scheme_val {
                1 => 1u32,
                4 => 2,
                _ => 0,
            };
            scheme_row.set_selected(idx);

            scheme_row.connect_selected_notify(|row| {
                let val_map = [0, 1, 4];
                let value = val_map[row.selected() as usize];
                gio::Settings::new(config::APP_ID)
                    .set_int("color-scheme", value)
                    .ok();
                let adw_scheme = match row.selected() {
                    1 => adw::ColorScheme::ForceLight,
                    2 => adw::ColorScheme::ForceDark,
                    _ => adw::ColorScheme::Default,
                };
                if let Some(app) = gio::Application::default() {
                    if let Ok(app) = app.downcast::<adw::Application>() {
                        app.style_manager().set_color_scheme(adw_scheme);
                    }
                }
            });
            appearance.add(&scheme_row);

            let page = adw::PreferencesPage::new();
            page.add(&audio);
            page.add(&appearance);
            obj.add(&page);
        }
    }

    impl WidgetImpl for FolderplayPreferences {}
    impl AdwDialogImpl for FolderplayPreferences {}
    impl PreferencesDialogImpl for FolderplayPreferences {}
}

glib::wrapper! {
    pub struct FolderplayPreferences(ObjectSubclass<imp::FolderplayPreferences>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget;
}

impl FolderplayPreferences {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }
}

fn output_description(idx: u32) -> String {
    match idx {
        0 => gettext(
            "Selects the best available output. Prefers HiFi when \
             PipeWire is present, otherwise falls back to Standard.",
        ),
        1 => gettext(
            "Passthrough output for bit-perfect playback via PipeWire \
             or ALSA. Ideal for Hi-Res lossless audio (96 – 192 kHz).",
        ),
        2 => gettext(
            "Legacy output via PulseAudio. Audio is resampled to a \
             common rate (44.1 – 48 kHz). Compatible with all systems.",
        ),
        _ => String::new(),
    }
}
