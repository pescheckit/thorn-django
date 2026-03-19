fn main() {
    thorn_cli::run(|| vec![Box::new(thorn_django::DjangoPlugin::new())]);
}
