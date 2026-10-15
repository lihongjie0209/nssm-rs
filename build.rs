#[path = "src/build_date.rs"]
mod build_date;

use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let timestamp = std::env::var("SOURCE_DATE_EPOCH")
        .map(|value| {
            value
                .parse::<u64>()
                .expect("SOURCE_DATE_EPOCH must be an unsigned Unix timestamp")
        })
        .unwrap_or_else(|_| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time must be after the Unix epoch")
                .as_secs()
        });
    println!(
        "cargo:rustc-env=NSSM_BUILD_DATE={}",
        build_date::from_unix_seconds(timestamp)
    );
}
