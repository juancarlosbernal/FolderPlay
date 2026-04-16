desktop_file = i18n.merge_file(
        input: 'io.github.juancarlosbernal.FolderPlay.desktop.in',
       output: 'io.github.juancarlosbernal.FolderPlay.desktop',
         type: 'desktop',
       po_dir: '../po',
      install: true,
  install_dir: get_option('datadir') / 'applications'
)

desktop_utils = find_program('desktop-file-validate', required: false)
if desktop_utils.found()
  test('Validate desktop file', desktop_utils, args: [desktop_file])
endif

appstream_file = i18n.merge_file(
        input: 'io.github.juancarlosbernal.FolderPlay.metainfo.xml.in',
       output: 'io.github.juancarlosbernal.FolderPlay.metainfo.xml',
       po_dir: '../po',
      install: true,
  install_dir: get_option('datadir') / 'metainfo'
)

appstreamcli = find_program('appstreamcli', required: false, disabler: true)
test('Validate appstream file', appstreamcli,
     args: ['validate', '--no-net', '--explain', appstream_file])

install_data('io.github.juancarlosbernal.FolderPlay.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas'
)

# Install screenshots so they are available in the Flatpak bundle.
install_data(
  ['../screenshots/1.png', '../screenshots/2.png',
   '../screenshots/3.png', '../screenshots/4.png'],
  install_dir: get_option('datadir') / 'app-info' / 'screenshots' / 'io.github.juancarlosbernal.FolderPlay'
)

compile_schemas = find_program('glib-compile-schemas', required: false, disabler: true)
test('Validate schema file',
     compile_schemas,
     args: ['--strict', '--dry-run', meson.current_source_dir()])


service_conf = configuration_data()
service_conf.set('bindir', get_option('prefix') / get_option('bindir'))
configure_file(
  input: 'io.github.juancarlosbernal.FolderPlay.service.in',
  output: 'io.github.juancarlosbernal.FolderPlay.service',
  configuration: service_conf,
  install_dir: get_option('datadir') / 'dbus-1' / 'services'
)

subdir('icons')

gtk_update_icon_cache = find_program('gtk4-update-icon-cache', required: false)
if gtk_update_icon_cache.found()
  meson.add_install_script('sh', '-c',
    'gtk4-update-icon-cache -qtf @0@'.format(
      get_option('prefix') / get_option('datadir') / 'icons' / 'hicolor'
    )
  )
endif
