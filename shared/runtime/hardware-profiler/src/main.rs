use hardware_profiler::detect_hardware_profile;

fn main() {
    let pretty = std::env::args().any(|a| a == "--pretty");
    let source_app =
        std::env::var("HARDWARE_PROFILE_SOURCE").unwrap_or_else(|_| String::from("unknown"));

    let profile = detect_hardware_profile(&source_app);
    let json = if pretty {
        serde_json::to_string_pretty(&profile)
    } else {
        serde_json::to_string(&profile)
    };

    match json {
        Ok(v) => println!("{}", v),
        Err(e) => {
            eprintln!("failed to serialize hardware profile: {}", e);
            std::process::exit(1);
        }
    }
}
