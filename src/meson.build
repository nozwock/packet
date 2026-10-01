global_conf = configuration_data()
global_conf.set_quoted('APP_ID', application_id)
global_conf.set_quoted('DATADIR', datadir)
global_conf.set_quoted('PKGDATADIR', pkgdatadir)
global_conf.set_quoted('PROFILE', profile)
global_conf.set_quoted('VERSION', version + version_suffix)
global_conf.set_quoted('GETTEXT_PACKAGE', gettext_package)
global_conf.set_quoted('LOCALEDIR', localedir)
configure_file(
  input: 'config.rs.in',
  output: 'config.rs',
  configuration: global_conf
)
# Copy the config.rs output to the source directory.
run_command(
  'cp',
  meson.project_build_root() / 'src' / 'config.rs',
  meson.project_source_root() / 'src' / 'config.rs',
  check: true
)

cargo_options = [ '--manifest-path', meson.project_source_root() / 'Cargo.toml' ]
cargo_options += [ '--target-dir', meson.project_build_root() / 'src' ]

if get_option('profile') == 'default'
  cargo_options += [ '--release' ]
  rust_target = 'release'
  message('Building in release mode')
else
  rust_target = 'debug'
  message('Building in debug mode')
endif

cargo_env = [ 'CARGO_HOME=' + meson.project_build_root() / 'cargo-home' ]

# Compile Blueprint resources
# Blueprints are tracked via ui-blueprint-resources.in instead of find command
# as run_command only runs on meson setup.
#
# https://gitlab.gnome.org/World/fractal/-/blob/209da3c268691b51e15b736a86bf5ae4c24d13ba/src/meson.build
fs = import('fs')

blp_files = []
ui_files = []
ui_files_xml = ''

# These files are already added manually in the resources file because they need custom handling.
blp_gresource_exceptions = [
  'shortcuts-dialog.blp',
]

foreach file : fs.read('ui-blueprint-resources.in').strip().splitlines()
  file = file.strip()
  if file.startswith('#') or file == ''
    continue
  endif

  blp_files += file

  ui_alias = file.replace('.blp', '.ui')
  ui_file = ui_alias.replace('/', '-')
  ui_files += ui_file

  if not blp_gresource_exceptions.contains(file)
    ui_files_xml += '\n    <file compressed="true" preprocess="xml-stripblanks" alias="' + ui_alias + '">' + ui_file + '</file>'
  endif
endforeach

blueprint_compiler = find_program('blueprint-compiler', required: true)

blueprints = custom_target(
  'blueprints',
  input: blp_files,
  output: ui_files,
  command: [
    meson.project_source_root() / 'build-aux' / 'compile-blueprints.sh',
    blueprint_compiler.full_path(),
    '@OUTDIR@',
    '@CURRENT_SOURCE_DIR@',
    '@INPUT@',
  ],
)

ui_resources_xml = configure_file(
  input: 'ui-resources.gresource.xml.in',
  output: 'ui-resources.gresource.xml',
  configuration: {'UI_FILES': ui_files_xml},
)

# Compile the UI files.
ui_resources = gnome.compile_resources(
  'ui-resources',
  ui_resources_xml,
  gresource_bundle: true,
  install: true,
  install_dir: pkgdatadir,
  dependencies: [blueprints, ui_resources_xml],
)

custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: meson.project_name(),
  console: true,
  install: true,
  install_dir: bindir,
  depends: [resources, ui_resources],
  command: [
    'env',
    cargo_env,
    cargo, 'build',
    cargo_options,
    '&&',
    'cp', 'src' / rust_target / meson.project_name(), '@OUTPUT@',
  ]
)
