use std::env;

fn main() {
    if let Err(err) = related::run(env::args().skip(1).collect()) {
        eprintln!("related: {err}");
        std::process::exit(related::exit_code_for_error(err.as_ref()));
    }
}
