pkgdatadir = get_option('prefix') / get_option('datadir') / meson.project_name()
moduledir = pkgdatadir / 'folderplay'
gnome = import('gnome')

gnome.compile_resources('folderplay',
  'folderplay.gresource.xml',
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
)

python = import('python')

conf = configuration_data()
conf.set('PYTHON', python.find_installation('python3').full_path())
conf.set('VERSION', meson.project_version())
conf.set('localedir', get_option('prefix') / get_option('localedir'))
conf.set('pkgdatadir', pkgdatadir)

configure_file(
  input: 'folderplay.in',
  output: 'folderplay',
  configuration: conf,
  install: true,
  install_dir: get_option('bindir'),
  install_mode: 'rwxr-xr-x'
)

folderplay_sources = [
  '__init__.py',
  'main.py',
  'window.py',
  'player.py',
  'preferences.py',
  'library_db.py',
]

install_data(folderplay_sources, install_dir: moduledir)
