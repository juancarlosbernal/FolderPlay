pkgdatadir = get_option('prefix') / get_option('datadir') / meson.project_name()

gnome = import('gnome')

# GResource bundle – still compiled by meson and installed for the Rust binary
gnome.compile_resources('folderplay',
  'folderplay.gresource.xml',
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
)

# config.rs (template substitution for Rust)
conf = configuration_data()
conf.set('VERSION', meson.project_version())
conf.set('localedir', get_option('prefix') / get_option('localedir'))

configure_file(
  input: 'config.rs.in',
  output: 'config.rs',
  configuration: conf,
)

# Copy generated config.rs to the source tree so cargo can find it
run_command(
  'cp',
  meson.project_build_root() / 'src' / 'config.rs',
  meson.project_source_root() / 'src' / 'config.rs',
  check: true,
)

cargo = find_program('cargo', required: true)

cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: get_option('bindir'),
  command: [
    find_program(meson.project_source_root() / 'build-aux' / 'cargo.sh'),
    meson.project_build_root(),
    meson.project_source_root(),
    '@OUTPUT@',
    get_option('buildtype'),
    meson.project_name(),
  ],
)
