# Copyright (c) 2026 Juan Carlos Bernal
#
# SPDX-License-Identifier: GPL-3.0-or-later

import os
import subprocess

import gi

gi.require_version('Gst', '1.0')

from gi.repository import GObject, GLib, Gst

Gst.init(None)

_HIFI_RATES = [44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000]
_HIFI_RATES_STR = '[ ' + ', '.join(str(r) for r in _HIFI_RATES) + ' ]'
_STD_RATES = [44100, 48000]
_STD_RATES_STR = '[ ' + ', '.join(str(r) for r in _STD_RATES) + ' ]'


class AudioPlayer(GObject.Object):

    __gsignals__ = {
        'state-changed': (GObject.SignalFlags.RUN_LAST, None, (bool,)),
        'position-updated': (GObject.SignalFlags.RUN_LAST, None, (float, float)),
        'song-finished': (GObject.SignalFlags.RUN_LAST, None, ()),
        'cover-art-changed': (GObject.SignalFlags.RUN_LAST, None, (object,)),
        'tags-updated': (GObject.SignalFlags.RUN_LAST, None, (str, str, str, str)),
    }

    def __init__(self):
        super().__init__()

        self._playbin = Gst.ElementFactory.make('playbin3', 'player')
        if not self._playbin:
            self._playbin = Gst.ElementFactory.make('playbin', 'player')

        bus = self._playbin.get_bus()
        bus.add_signal_watch()
        bus.connect('message::eos', self._on_eos)
        bus.connect('message::error', self._on_error)
        bus.connect('message::tag', self._on_tag)
        bus.connect('message::state-changed', self._on_state_changed)

        self._position_timer_id = 0
        self._current_uri = None
        self._is_playing = False
        self._volume = 0.7
        self._playbin.set_property('volume', self._volume)

    def set_audio_output(self, output_type):
        was_playing = self._is_playing
        uri = self._current_uri
        if was_playing:
            self.stop()

        sink = None
        if output_type == 'hifi':
            self._set_pipewire_rates(_HIFI_RATES_STR)
            self._write_hifi_config()
            sink = self._make_hifi_sink()
        elif output_type == 'standard':
            self._set_pipewire_rates(_STD_RATES_STR)
            sink = Gst.ElementFactory.make('pulsesink', 'audio-sink')
        else:
            # 'auto': try HiFi first, fall back to standard, then default
            sink = self._make_hifi_sink()
            if sink is not None:
                self._set_pipewire_rates(_HIFI_RATES_STR)
                self._write_hifi_config()
            else:
                sink = Gst.ElementFactory.make('pulsesink', 'audio-sink')

        self._playbin.set_property('audio-sink', sink)

        if was_playing and uri:
            self._current_uri = uri
            self._playbin.set_property('uri', uri)
            self._playbin.set_state(Gst.State.PLAYING)

    @staticmethod
    def _make_hifi_sink():
        sink = Gst.ElementFactory.make('pipewiresink', 'audio-sink')
        if sink is None:
            sink = Gst.ElementFactory.make('alsasink', 'audio-sink')
        return sink

    @staticmethod
    def _set_pipewire_rates(rates_str):
        """Set PipeWire allowed sample rates for the current session."""
        try:
            subprocess.run(
                ['pw-metadata', '-n', 'settings', '0',
                 'clock.allowed-rates', rates_str],
                capture_output=True, timeout=3,
            )
        except (OSError, subprocess.TimeoutExpired):
            pass

    @staticmethod
    def _write_hifi_config():
        """Write persistent PipeWire drop-in for Hi-Res rates."""
        try:
            conf_dir = os.path.join(
                os.environ.get('XDG_CONFIG_HOME',
                               os.path.expanduser('~/.config')),
                'pipewire', 'pipewire.conf.d',
            )
            conf_path = os.path.join(conf_dir, 'folderplay-hifi.conf')
            if not os.path.exists(conf_path):
                os.makedirs(conf_dir, exist_ok=True)
                with open(conf_path, 'w') as f:
                    f.write(
                        '# Added by FolderPlay for Hi-Res playback\n'
                        'context.properties = {\n'
                        f'    default.clock.allowed-rates = '
                        f'{_HIFI_RATES_STR}\n'
                        '}\n'
                    )
        except OSError:
            pass

    @property
    def is_playing(self):
        return self._is_playing

    @property
    def volume(self):
        return self._volume

    @volume.setter
    def volume(self, value):
        self._volume = max(0.0, min(1.0, value))
        self._playbin.set_property('volume', self._volume)

    def play_uri(self, uri):
        self.stop()
        self._current_uri = uri
        self._playbin.set_property('uri', uri)
        self._playbin.set_state(Gst.State.PLAYING)

    def play(self):
        if self._current_uri:
            self._playbin.set_state(Gst.State.PLAYING)

    def pause(self):
        self._playbin.set_state(Gst.State.PAUSED)

    def toggle_play(self):
        if self._is_playing:
            self.pause()
        else:
            self.play()

    def stop(self):
        self._playbin.set_state(Gst.State.NULL)
        self._stop_position_poll()
        self._is_playing = False

    def seek(self, position_secs):
        self._playbin.seek_simple(
            Gst.Format.TIME,
            Gst.SeekFlags.FLUSH | Gst.SeekFlags.KEY_UNIT,
            int(position_secs * Gst.SECOND)
        )

    def _on_eos(self, bus, message):
        GLib.idle_add(self._handle_eos)

    def _handle_eos(self):
        self.stop()
        self.emit('song-finished')
        return False

    def _on_error(self, bus, message):
        err, debug = message.parse_error()
        print(f"GStreamer error: {err.message}")
        GLib.idle_add(self.stop)

    def _on_state_changed(self, bus, message):
        if message.src != self._playbin:
            return
        old, new, pending = message.parse_state_changed()
        is_playing = new == Gst.State.PLAYING
        if is_playing != self._is_playing:
            self._is_playing = is_playing
            if is_playing:
                self._start_position_poll()
            else:
                self._stop_position_poll()
            GLib.idle_add(self.emit, 'state-changed', is_playing)

    def _on_tag(self, bus, message):
        tags = message.parse_tag()

        title = artist = album = year = ''
        ok, val = tags.get_string('title')
        if ok:
            title = val
        ok, val = tags.get_string('artist')
        if ok:
            artist = val
        ok, val = tags.get_string('album')
        if ok:
            album = val

        # Try date-time first, then date
        ok, dt = tags.get_date_time('datetime')
        if ok and dt:
            year = str(dt.get_year())
        else:
            ok, d = tags.get_date('date')
            if ok and d:
                year = str(d.get_year())

        if title or artist or album:
            GLib.idle_add(self.emit, 'tags-updated', title, artist, album, year)

        sample = None
        ok, s = tags.get_sample_index('image', 0)
        if ok:
            sample = s
        else:
            ok, s = tags.get_sample_index('preview-image', 0)
            if ok:
                sample = s

        if sample:
            buf = sample.get_buffer()
            ok, map_info = buf.map(Gst.MapFlags.READ)
            if ok:
                data = bytes(map_info.data)
                buf.unmap(map_info)
                GLib.idle_add(self.emit, 'cover-art-changed', data)

    def _start_position_poll(self):
        if self._position_timer_id == 0:
            self._position_timer_id = GLib.timeout_add(
                500, self._poll_position
            )

    def _stop_position_poll(self):
        if self._position_timer_id:
            GLib.source_remove(self._position_timer_id)
            self._position_timer_id = 0

    def _poll_position(self):
        if not self._is_playing:
            self._position_timer_id = 0
            return False

        ok1, position = self._playbin.query_position(Gst.Format.TIME)
        ok2, duration = self._playbin.query_duration(Gst.Format.TIME)

        if ok1 and ok2 and duration > 0:
            pos_secs = position / Gst.SECOND
            dur_secs = duration / Gst.SECOND
            self.emit('position-updated', pos_secs, dur_secs)

        return True

    def cleanup(self):
        self.stop()
        bus = self._playbin.get_bus()
        bus.remove_signal_watch()
        self._playbin.set_state(Gst.State.NULL)
