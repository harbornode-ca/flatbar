use flatbar_core::ipc::generate_niri_snippet;
use flatbar_core::{run_flatbar_with_path, Config, InstanceLock, SingletonError};
use std::env;
use std::path::{Path, PathBuf};
use std::process;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!(
        "flatbar v{VERSION} - Lightweight, two-color Wayland status bar\n\n\
        USAGE:\n    \
            flatbar [OPTIONS]\n\n\
        OPTIONS:\n    \
            -c, --config <PATH>      Path to configuration file\n    \
            --validate-config        Validate configuration file and exit (0 = valid, 1 = invalid)\n    \
            --print-window-rules     Print generated Niri window-rules configuration\n    \
            --print-schema           Print JSON schema for configuration\n    \
            -v, --version            Print version information\n    \
            -h, --help               Print this help message"
    );
}

fn resolve_config_path(explicit_path: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = explicit_path {
        return Some(path);
    }

    if let Ok(xdg_config) = env::var("XDG_CONFIG_HOME") {
        let p = Path::new(&xdg_config).join("flatbar").join("config.toml");
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(home) = env::var("HOME") {
        let p = Path::new(&home)
            .join(".config")
            .join("flatbar")
            .join("config.toml");
        if p.exists() {
            return Some(p);
        }
    }

    None
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut config_path: Option<PathBuf> = None;
    let mut validate_only = false;
    let mut print_window_rules = false;
    let mut print_schema = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            "-v" | "--version" => {
                println!("flatbar v{VERSION}");
                process::exit(0);
            }
            "--validate-config" => {
                validate_only = true;
            }
            "--print-window-rules" => {
                print_window_rules = true;
            }
            "--print-schema" => {
                print_schema = true;
            }
            "-c" | "--config" => {
                if i + 1 < args.len() {
                    config_path = Some(PathBuf::from(&args[i + 1]));
                    i += 1;
                } else {
                    eprintln!("Error: --config requires a file path argument");
                    process::exit(1);
                }
            }
            unknown => {
                eprintln!("Error: unrecognized argument '{unknown}'");
                print_help();
                process::exit(1);
            }
        }
        i += 1;
    }

    if print_schema {
        println!("{}", flatbar_core::config::print_schema_json());
        process::exit(0);
    }

    // Initialize tracing (env filter, default off/warn)
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    let target_path = match resolve_config_path(config_path.clone()) {
        Some(path) => path,
        None => {
            if let Some(p) = config_path {
                eprintln!("Error: configuration file not found at {}", p.display());
                process::exit(1);
            } else {
                eprintln!("No configuration file found in default locations (~/.config/flatbar/config.toml).");
                process::exit(1);
            }
        }
    };

    let config = match Config::load_from_file(&target_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Configuration error in {}:\n  {err}", target_path.display());
            process::exit(1);
        }
    };

    if validate_only {
        println!("Configuration at {} is valid.", target_path.display());
        process::exit(0);
    }

    if print_window_rules {
        let snippet = generate_niri_snippet(&config.window_rules.rules);
        print!("{snippet}");
        process::exit(0);
    }

    // Singleton check per config bar name
    let _lock = match InstanceLock::acquire(&config.bar.name) {
        Ok(lock) => lock,
        Err(SingletonError::AlreadyRunning(name)) => {
            println!(
                "Another instance of flatbar named '{name}' is already running. Exiting cleanly."
            );
            process::exit(0);
        }
        Err(err) => {
            eprintln!("Failed to acquire singleton lock: {err}");
            process::exit(1);
        }
    };

    if let Err(err) = run_flatbar_with_path(config, Some(target_path)) {
        eprintln!("Runtime error: {err}");
        process::exit(1);
    }
}
