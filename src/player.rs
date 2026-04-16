// Copyright (c) 2026 Juan Carlos Bernal
// SPDX-License-Identifier: GPL-3.0-or-later

use glib::prelude::*;
use glib::subclass::prelude::*;
use glib::subclass::Signal;
use gst::prelude::*;
use std::cell::{Cell, RefCell};
use std::process::Command;
use std::sync::OnceLock;

static HIFI_RATES: &[u32] = &[44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000];
static STD_RATES: &[u32] = &[44100, 48000];

fn rates_str(rates: &[u32]) -> String {
    let inner: Vec<String> = rates.iter().map(|r| r.to_string()).collect();
    format!("[ {} ]", inner.join(", "))
}

// ── imp ────────────────────────────────────────────────────────────
mod imp {
    use super::*;

    pub struct AudioPlayer {
        pub playbin: RefCell<Option<gst::Element>>,
        pub position_timer: RefCell<Option<glib::SourceId>>,
        pub current_uri: RefCell<Option<String>>,
        pub is_playing: Cell<bool>,
        pub volume: Cell<f64>,
    }

    impl Default for AudioPlayer {
        fn default() -> Self {
            Self {
                playbin: RefCell::new(None),
                position_timer: RefCell::new(None),
                current_uri: RefCell::new(None),
                is_playing: Cell::new(false),
                volume: Cell::new(0.7),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AudioPlayer {
        const NAME: &'static str = "AudioPlayer";
        type Type = super::AudioPlayer;
        type ParentType = glib::Object;
    }

    impl ObjectImpl for AudioPlayer {
        fn constructed(&self) {
            self.parent_constructed();
            let playbin = gst::ElementFactory::make("playbin3")
                .name("player")
                .build()
                .or_else(|_| gst::ElementFactory::make("playbin").name("player").build())
                .expect("Failed to create playbin");

            playbin.set_property("volume", 0.7f64);

            let bus = match playbin.bus() {
                Some(b) => b,
                None => {
                    eprintln!("Failed to get GStreamer bus");
                    *self.playbin.borrow_mut() = Some(playbin);
                    return;
                }
            };
            bus.add_signal_watch();

            let obj_weak = self.obj().downgrade();
            bus.connect_local("message::eos", false, move |_args| {
                if let Some(obj) = obj_weak.upgrade() {
                    glib::idle_add_local_once(glib::clone!(
                        #[weak] obj,
                        move || {
                            obj.stop();
                            obj.emit_by_name::<()>("song-finished", &[]);
                        }
                    ));
                }
                None
            });

            let obj_weak = self.obj().downgrade();
            bus.connect_local("message::error", false, move |args| {
                if let Ok(msg) = args[1].get::<gst::Message>() {
                    if let gst::MessageView::Error(err) = msg.view() {
                        eprintln!("GStreamer error: {}", err.error());
                    }
                }
                if let Some(obj) = obj_weak.upgrade() {
                    glib::idle_add_local_once(glib::clone!(
                        #[weak] obj,
                        move || obj.stop()
                    ));
                }
                None
            });

            let obj_weak = self.obj().downgrade();
            bus.connect_local("message::state-changed", false, move |args| {
                let Ok(msg) = args[1].get::<gst::Message>() else { return None };
                if let Some(obj) = obj_weak.upgrade() {
                    let pb = obj.imp().playbin.borrow();
                    let pb = pb.as_ref().unwrap();
                    if msg.src().map(|s| *s == *pb).unwrap_or(false) {
                        if let gst::MessageView::StateChanged(sc) = msg.view() {
                            let playing = sc.current() == gst::State::Playing;
                            let was = obj.imp().is_playing.get();
                            if playing != was {
                                obj.imp().is_playing.set(playing);
                                if playing {
                                    obj.start_position_poll();
                                } else {
                                    obj.stop_position_poll();
                                }
                                let obj2 = obj.clone();
                                glib::idle_add_local_once(move || {
                                    obj2.emit_by_name::<()>("state-changed", &[&playing]);
                                });
                            }
                        }
                    }
                }
                None
            });

            let obj_weak = self.obj().downgrade();
            bus.connect_local("message::tag", false, move |args| {
                let msg = args[1].get::<gst::Message>().unwrap();
                if let Some(obj) = obj_weak.upgrade() {
                    if let gst::MessageView::Tag(tag_msg) = msg.view() {
                        let tags = tag_msg.tags();
                        let title = tags.get::<gst::tags::Title>().map(|v| v.get().to_string()).unwrap_or_default();
                        let artist = tags.get::<gst::tags::Artist>().map(|v| v.get().to_string()).unwrap_or_default();
                        let album = tags.get::<gst::tags::Album>().map(|v| v.get().to_string()).unwrap_or_default();
                        let year = tags.get::<gst::tags::DateTime>()
                            .map(|v| v.get().year().to_string())
                            .or_else(|| tags.get::<gst::tags::Date>().map(|v| v.get().year().to_string()))
                            .unwrap_or_default();

                        if !title.is_empty() || !artist.is_empty() || !album.is_empty() {
                            let obj2 = obj.clone();
                            glib::idle_add_local_once(move || {
                                obj2.emit_by_name::<()>(
                                    "tags-updated",
                                    &[&title, &artist, &album, &year],
                                );
                            });
                        }

                        // Cover art from tags
                        let sample = tags.index::<gst::tags::Image>(0)
                            .or_else(|| tags.index::<gst::tags::PreviewImage>(0));
                        if let Some(sample_val) = sample {
                            let sample: gst::Sample = sample_val.get();
                            if let Some(buf) = sample.buffer() {
                                if let Ok(map) = buf.map_readable() {
                                    let data: Vec<u8> = map.as_slice().to_vec();
                                    let obj2 = obj.clone();
                                    glib::idle_add_local_once(move || {
                                        obj2.emit_by_name::<()>("cover-art-changed", &[&glib::Bytes::from(&data)]);
                                    });
                                }
                            }
                        }
                    }
                }
                None
            });

            *self.playbin.borrow_mut() = Some(playbin);
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("state-changed")
                        .param_types([bool::static_type()])
                        .build(),
                    Signal::builder("position-updated")
                        .param_types([f64::static_type(), f64::static_type()])
                        .build(),
                    Signal::builder("song-finished").build(),
                    Signal::builder("cover-art-changed")
                        .param_types([glib::Bytes::static_type()])
                        .build(),
                    Signal::builder("tags-updated")
                        .param_types([
                            String::static_type(),
                            String::static_type(),
                            String::static_type(),
                            String::static_type(),
                        ])
                        .build(),
                ]
            })
        }
    }
}

// ── wrapper ────────────────────────────────────────────────────────
glib::wrapper! {
    pub struct AudioPlayer(ObjectSubclass<imp::AudioPlayer>);
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioPlayer {
    pub fn new() -> Self {
        glib::Object::builder().build()
    }

    pub fn set_audio_output(&self, output_type: &str) {
        let was_playing = self.imp().is_playing.get();
        let uri = self.imp().current_uri.borrow().clone();
        if was_playing {
            self.stop();
        }

        let pb = self.imp().playbin.borrow();
        let playbin = pb.as_ref().unwrap();

        match output_type {
            "hifi" => {
                set_pipewire_rates(&rates_str(HIFI_RATES));
                write_hifi_config();
                if let Some(sink) = make_hifi_sink() {
                    playbin.set_property("audio-sink", &sink);
                }
            }
            "standard" => {
                set_pipewire_rates(&rates_str(STD_RATES));
                if let Ok(sink) = gst::ElementFactory::make("pulsesink").name("audio-sink").build() {
                    playbin.set_property("audio-sink", &sink);
                }
            }
            _ => {
                // auto
                if let Some(sink) = make_hifi_sink() {
                    set_pipewire_rates(&rates_str(HIFI_RATES));
                    write_hifi_config();
                    playbin.set_property("audio-sink", &sink);
                } else if let Ok(sink) = gst::ElementFactory::make("pulsesink").name("audio-sink").build() {
                    playbin.set_property("audio-sink", &sink);
                }
            }
        }

        if was_playing {
            if let Some(uri) = uri {
                *self.imp().current_uri.borrow_mut() = Some(uri.clone());
                playbin.set_property("uri", &uri);
                playbin.set_state(gst::State::Playing).ok();
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        self.imp().is_playing.get()
    }

    pub fn volume(&self) -> f64 {
        self.imp().volume.get()
    }

    pub fn set_volume(&self, value: f64) {
        let v = value.clamp(0.0, 1.0);
        self.imp().volume.set(v);
        if let Some(pb) = self.imp().playbin.borrow().as_ref() {
            pb.set_property("volume", v);
        }
    }

    pub fn play_uri(&self, uri: &str) {
        self.stop();
        *self.imp().current_uri.borrow_mut() = Some(uri.to_string());
        if let Some(pb) = self.imp().playbin.borrow().as_ref() {
            pb.set_property("uri", uri);
            pb.set_state(gst::State::Playing).ok();
        }
    }

    pub fn play(&self) {
        if self.imp().current_uri.borrow().is_some() {
            if let Some(pb) = self.imp().playbin.borrow().as_ref() {
                pb.set_state(gst::State::Playing).ok();
            }
        }
    }

    pub fn pause(&self) {
        if let Some(pb) = self.imp().playbin.borrow().as_ref() {
            pb.set_state(gst::State::Paused).ok();
        }
    }

    pub fn toggle_play(&self) {
        if self.imp().is_playing.get() {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn stop(&self) {
        if let Some(pb) = self.imp().playbin.borrow().as_ref() {
            pb.set_state(gst::State::Null).ok();
        }
        self.stop_position_poll();
        self.imp().is_playing.set(false);
    }

    pub fn seek(&self, position_secs: f64) {
        if let Some(pb) = self.imp().playbin.borrow().as_ref() {
            pb.seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                gst::ClockTime::from_nseconds((position_secs * 1_000_000_000.0) as u64),
            ).ok();
        }
    }

    fn start_position_poll(&self) {
        if self.imp().position_timer.borrow().is_none() {
            let obj_weak = self.downgrade();
            let id = glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                if let Some(obj) = obj_weak.upgrade() {
                    if !obj.imp().is_playing.get() {
                        *obj.imp().position_timer.borrow_mut() = None;
                        return glib::ControlFlow::Break;
                    }
                    if let Some(pb) = obj.imp().playbin.borrow().as_ref() {
                        if let (Some(pos), Some(dur)) = (
                            pb.query_position::<gst::ClockTime>(),
                            pb.query_duration::<gst::ClockTime>(),
                        ) {
                            let pos_secs = pos.nseconds() as f64 / 1_000_000_000.0;
                            let dur_secs = dur.nseconds() as f64 / 1_000_000_000.0;
                            if dur_secs > 0.0 {
                                obj.emit_by_name::<()>("position-updated", &[&pos_secs, &dur_secs]);
                            }
                        }
                    }
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
            *self.imp().position_timer.borrow_mut() = Some(id);
        }
    }

    fn stop_position_poll(&self) {
        if let Some(id) = self.imp().position_timer.borrow_mut().take() {
            id.remove();
        }
    }

    pub fn cleanup(&self) {
        self.stop();
        if let Some(pb) = self.imp().playbin.borrow().as_ref() {
            if let Some(bus) = pb.bus() {
                bus.remove_signal_watch();
            }
            pb.set_state(gst::State::Null).ok();
        }
    }
}

fn make_hifi_sink() -> Option<gst::Element> {
    gst::ElementFactory::make("pipewiresink")
        .name("audio-sink")
        .build()
        .ok()
        .or_else(|| {
            gst::ElementFactory::make("alsasink")
                .name("audio-sink")
                .build()
                .ok()
        })
}

fn set_pipewire_rates(rates: &str) {
    Command::new("pw-metadata")
        .args(["-n", "settings", "0", "clock.allowed-rates", rates])
        .output()
        .ok();
}

fn write_hifi_config() {
    // Use $HOME/.config/pipewire/pipewire.conf.d because:
    // - PipeWire reads from the real ~/.config, not from XDG_CONFIG_HOME
    // - Inside Flatpak, XDG_CONFIG_HOME = ~/.var/app/.../config (sandbox-only)
    // - The manifest grants --filesystem=xdg-config/pipewire/pipewire.conf.d:create
    //   which mounts the HOST's ~/.config/pipewire/pipewire.conf.d at that same path
    let conf_dir = dirs_home().join(".config").join("pipewire").join("pipewire.conf.d");
    let conf_path = conf_dir.join("folderplay-hifi.conf");
    if !conf_path.exists() {
        if let Err(e) = std::fs::create_dir_all(&conf_dir) {
            eprintln!("Failed to create PipeWire config dir: {e}");
            return;
        }
        if let Err(e) = std::fs::write(
            &conf_path,
            format!(
                "# Added by FolderPlay for Hi-Res playback\n\
                 context.properties = {{\n\
                     default.clock.allowed-rates = {}\n\
                 }}\n",
                rates_str(HIFI_RATES)
            ),
        ) {
            eprintln!("Failed to write PipeWire HiFi config: {e}");
        }
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"))
}
