#![crate_type = "bin"]

use std::env;
use std::fs::File;
use std::io::{BufReader, Write};
use std::process::exit;

use xdrgen::generate;

fn help() {
    println!(
        r#"XDR code generator {}

USAGE:
    xdrgen [FILE]

ARGS:
    <FILE>  Set *.x file

OPTIONS:
    -h, --help      Print help information
    -V, --version   Print version information
"#,
        env!("CARGO_PKG_VERSION")
    )
}

fn main() {
    let output = std::io::stdout();
    let mut stderr = std::io::stderr();

    let args: Vec<String> = env::args().collect();
    let result = match args.len() {
        // no arguments passed, read from stdin
        1 => generate("stdin", BufReader::new(std::io::stdin()), output),

        // one argument passed
        2 => {
            let arg = args[1].trim();
            if arg == "-h" || arg == "--help" {
                help();
                return;
            }

            if arg == "-V" || arg == "--version" {
                println!("XDR code generator {}", env!("CARGO_PKG_VERSION"));
                return;
            }

            if arg.starts_with("-") {
                help();
                return;
            }

            let f = match File::open(arg) {
                Ok(f) => f,
                Err(e) => {
                    let _ = writeln!(&mut stderr, "Failed to open {}: {}", arg, e);
                    exit(1);
                }
            };

            generate(arg, BufReader::new(f), output)
        }
        _ => {
            help();
            exit(1);
        }
    };

    if let Err(err) = result {
        let _ = writeln!(&mut stderr, "Failed: {}", err);
        exit(1);
    }
}
