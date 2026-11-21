fn main() {
    let arguments = std::env::args().skip(1);
    let cli = match starling::runtime::parse_cli(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("nginx: [emerg] {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = starling::runtime::execute(cli) {
        eprintln!("nginx: [emerg] {error}");
        std::process::exit(1);
    }
}
