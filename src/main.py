# Copyright (c) 2026 Juan Carlos Bernal
#
# SPDX-License-Identifier: GPL-3.0-or-later

import sys
import gi

from gettext import gettext as _

gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
gi.require_version('Gst', '1.0')
gi.require_version('GstPbutils', '1.0')

from gi.repository import Gtk, Gdk, Gio, Adw
from .window import FolderplayWindow
from .preferences import FolderplayPreferences


class FolderplayApplication(Adw.Application):

    def __init__(self):
        super().__init__(application_id='org.gnome.folderplay',
                         flags=Gio.ApplicationFlags.HANDLES_OPEN,
                         resource_base_path='/org/gnome/folderplay')
        self.create_action('quit', lambda *_: self.quit(), ['<control>q'])
        self.create_action('about', self.on_about_action)
        self.create_action('shortcuts', self.on_shortcuts_action)
        self.create_action('preferences', self.on_preferences_action)

        self.set_accels_for_action('win.play-pause', ['space'])
        self.set_accels_for_action('win.next-track', ['Right'])
        self.set_accels_for_action('win.prev-track', ['Left'])
        self.set_accels_for_action('win.open-folder', ['<control>o'])

    def do_startup(self):
        Adw.Application.do_startup(self)
        # Register app icon from gresource so it shows in taskbar/dashboard
        # even when running inside GNOME Builder / Flatpak sandbox
        icon_theme = Gtk.IconTheme.get_for_display(Gdk.Display.get_default())
        icon_theme.add_resource_path('/org/gnome/folderplay/icons')

    def do_activate(self):
        win = self.props.active_window
        if not win:
            win = FolderplayWindow(application=self)

        settings = Gio.Settings.new('org.gnome.folderplay')
        scheme = settings.get_int('color-scheme')
        self.get_style_manager().set_color_scheme(scheme)

        win.present()

    def do_open(self, files, n_files, hint):
        self.do_activate()
        win = self.props.active_window
        if win and files:
            win.open_file(files[0].get_path())

    def on_about_action(self, *args):
        about = Adw.AboutDialog(
            application_name='FolderPlay',
            application_icon='org.gnome.folderplay',
            developer_name='Juan Carlos Bernal',
            version='1.0.0',
            translator_credits=_('translator-credits'),
            developers=['Juan Carlos Bernal'],
            copyright='© 2026 Juan Carlos Bernal',
            comments=_(
                'FolderPlay is a minimalist music player whose only purpose '
                'is to let you enjoy your local music collection by showing '
                'you the folders exactly as you have organized them on your '
                'disk.\n'
                '\n'
                'FolderPlay does not group by artist, album, genre, or '
                'anything else… It respects the order you defined. It also '
                'has a strong focus on Lossless Hi-Res audio playback.'
            ),
            license_type=Gtk.License.GPL_3_0,
        )
        about.present(self.props.active_window)

    def on_shortcuts_action(self, *args):
        builder = Gtk.Builder.new_from_resource(
            '/org/gnome/folderplay/shortcuts-dialog.ui'
        )
        dialog = builder.get_object('shortcuts_dialog')
        dialog.present(self.props.active_window)

    def on_preferences_action(self, *args):
        prefs = FolderplayPreferences()
        prefs.present(self.props.active_window)

    def create_action(self, name, callback, shortcuts=None):
        action = Gio.SimpleAction.new(name, None)
        action.connect("activate", callback)
        self.add_action(action)
        if shortcuts:
            self.set_accels_for_action(f"app.{name}", shortcuts)


def main(version):
    app = FolderplayApplication()
    return app.run(sys.argv)
