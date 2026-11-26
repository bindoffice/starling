#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let arguments = std::env::args().skip(1);
    let cli = match starling::runtime::parse_cli(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("starling: [emerg] {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = starling::runtime::execute(cli) {
        eprintln!("starling: [emerg] {error}");
        std::process::exit(1);
    }
}
