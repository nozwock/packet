plugins_conf = configuration_data()
plugins_conf.set('APP_ID', application_id)
plugins_conf.set('LOCALE_DOMAIN', gettext_package)
plugins_conf.set('LOCALE_DIR', localedir)

configure_file(
  input: 'packet_nautilus.py.in',
  output: 'packet_nautilus.py',
  configuration: plugins_conf,
  install_dir: pkgdatadir / 'plugins',
)