# Copyright (c) 2026 Juan Carlos Bernal
#
# SPDX-License-Identifier: GPL-3.0-or-later

from gettext import gettext as _

from gi.repository import Adw, Gtk, Gio

_OUTPUT_VALUES = ['auto', 'hifi', 'standard']

_OUTPUT_DESCRIPTIONS = {
    0: _('Selects the best available output. Prefers HiFi when '
       'PipeWire is present, otherwise falls back to Standard.'),
    1: _('Passthrough output for bit-perfect playback via PipeWire '
       'or ALSA. Ideal for Hi-Res lossless audio (96 – 192 kHz).'),
    2: _('Legacy output via PulseAudio. Audio is resampled to a '
       'common rate (44.1 – 48 kHz). Compatible with all systems.'),
}


class FolderplayPreferences(Adw.PreferencesDialog):
    __gtype_name__ = 'FolderplayPreferences'

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.set_title(_('Preferences'))

        settings = Gio.Settings.new('org.gnome.folderplay')

        # ── Audio ───────────────────────────────────────────────────
        audio = Adw.PreferencesGroup(
            title=_('Audio Output'),
            description=_('HiFi sends audio directly to your DAC without '
            'resampling. Standard uses PulseAudio for maximum compatibility.'),
        )

        self._output_row = Adw.ComboRow(
            title=_('Output'),
        )
        out_model = Gtk.StringList.new([
            _('Automatic'),
            _('HiFi (Bit-perfect)'),
            _('Standard (PulseAudio)'),
        ])
        self._output_row.set_model(out_model)

        cur = settings.get_string('audio-output')
        try:
            cur_idx = _OUTPUT_VALUES.index(cur)
        except ValueError:
            cur_idx = 0
        self._output_row.set_selected(cur_idx)
        self._output_row.set_subtitle(_OUTPUT_DESCRIPTIONS[cur_idx])

        self._output_row.connect(
            'notify::selected', self._on_output_changed,
        )
        audio.add(self._output_row)

        # ── Appearance ──────────────────────────────────────────────
        appearance = Adw.PreferencesGroup(title=_('Appearance'))

        self._scheme_row = Adw.ComboRow(
            title=_('Style'),
            subtitle=_('Choose between light and dark appearance'),
        )
        model = Gtk.StringList.new([_('System'), _('Light'), _('Dark')])
        self._scheme_row.set_model(model)
        idx_map = {0: 0, 1: 1, 4: 2}
        self._scheme_row.set_selected(
            idx_map.get(settings.get_int('color-scheme'), 0)
        )
        self._scheme_row.connect(
            'notify::selected', self._on_scheme_changed,
        )
        appearance.add(self._scheme_row)

        page = Adw.PreferencesPage()
        page.add(audio)
        page.add(appearance)
        self.add(page)

    def _on_scheme_changed(self, row, pspec):
        val_map = {0: 0, 1: 1, 2: 4}
        value = val_map.get(row.get_selected(), 0)
        Gio.Settings.new('org.gnome.folderplay').set_int(
            'color-scheme', value,
        )
        adw_map = {
            0: Adw.ColorScheme.DEFAULT,
            1: Adw.ColorScheme.FORCE_LIGHT,
            2: Adw.ColorScheme.FORCE_DARK,
        }
        app = Adw.Application.get_default()
        if app:
            app.get_style_manager().set_color_scheme(
                adw_map.get(row.get_selected(), Adw.ColorScheme.DEFAULT)
            )

    def _on_output_changed(self, row, pspec):
        idx = row.get_selected()
        value = _OUTPUT_VALUES[idx]
        row.set_subtitle(_OUTPUT_DESCRIPTIONS[idx])

        Gio.Settings.new('org.gnome.folderplay').set_string(
            'audio-output', value,
        )
        app = Adw.Application.get_default()
        if app:
            win = app.get_active_window()
            if win and hasattr(win, '_player'):
                win._player.set_audio_output(value)
