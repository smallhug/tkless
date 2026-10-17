fn main() {
    let build_date = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            let days = d.as_secs() / 86400;
            epoch_days_to_date(days)
        })
        .unwrap_or_else(|_| "2026-10-15".to_string());

    println!("cargo:rustc-env=TKLESS_BUILD_DATE={build_date}");
    println!("cargo:rustc-env=PAGECLAW_BUILD_DATE={build_date}");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set(
            "FileDescription",
            "TkLess - Spend less tokens, perceive more web",
        );
        res.set("ProductName", "TkLess");
        res.set("CompanyName", "TkLess Contributors");
        res.set("OriginalFilename", "tkless.exe");
        res.set("LegalCopyright", "TkLess Contributors");
        res.set("FileVersion", "0.1.0.0");
        res.set("ProductVersion", "0.1.0.0");
        if let Err(e) = res.compile() {
            eprintln!("Warning: Failed to compile Windows resource: {e}");
        }
    }
}

fn epoch_days_to_date(days: u64) -> String {
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
