use std::io::Write;
use std::process::Command;

/// Generate a realistic CloudFront-format log, import it, and run the full pipeline.
#[test]
#[ignore] // requires release build
fn cloudfront_import_full_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("cf.log");
    let trace_path = dir.path().join("trace.csv");
    let policy_path = dir.path().join("policy.json");

    // Generate a synthetic CloudFront log (1000 lines)
    generate_cf_log(&log_path, 1000);

    let qc = env!("CARGO_BIN_EXE_qc");

    // Import
    let out = Command::new(qc)
        .args(["import", "-p", "cloudfront", "-i"])
        .arg(&log_path)
        .args(["-o"])
        .arg(&trace_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let import_msg = String::from_utf8_lossy(&out.stdout);
    assert!(
        import_msg.contains("Imported"),
        "unexpected output: {import_msg}"
    );

    // Optimize
    let out = Command::new(qc)
        .args(["optimize", "-i"])
        .arg(&trace_path)
        .args(["-o"])
        .arg(&policy_path)
        .args(["--capacity", "500000", "--preset", "ecommerce"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "optimize failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Simulate
    let out = Command::new(qc)
        .args(["simulate", "-i"])
        .arg(&trace_path)
        .args(["-p"])
        .arg(&policy_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "simulate failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let sim_msg = String::from_utf8_lossy(&out.stdout);
    assert!(sim_msg.contains("Hit ratio"), "unexpected simulate output");

    // Compare
    let out = Command::new(qc)
        .args(["compare", "-i"])
        .arg(&trace_path)
        .args(["--capacity", "500000", "--preset", "ecommerce"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "compare failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cmp_msg = String::from_utf8_lossy(&out.stdout);
    assert!(cmp_msg.contains("LRU"));
    assert!(cmp_msg.contains("GDSF"));
    assert!(cmp_msg.contains("EconomicGreedy"));
}

fn generate_cf_log(path: &std::path::Path, num_lines: usize) {
    use std::io::BufWriter;
    let mut f = BufWriter::new(std::fs::File::create(path).unwrap());

    writeln!(f, "#Version: 1.0").unwrap();
    writeln!(f, "#Fields: date time x-edge-location sc-bytes c-ip cs-method cs(Host) cs-uri-stem sc-status cs(Referer) cs(User-Agent) cs-uri-query cs(Cookie) x-edge-result-type x-edge-request-id x-host-header cs-protocol cs-bytes time-taken x-forwarded-for ssl-protocol ssl-cipher x-edge-response-result-type cs-protocol-version fle-status fle-encrypted-fields c-port time-to-first-byte x-edge-detailed-result-type sc-content-type sc-content-len sc-range-start sc-range-end").unwrap();

    let paths = [
        ("/images/hero.jpg", "image/jpeg", 524288u64),
        ("/images/thumb.webp", "image/webp", 32768),
        ("/api/products", "application/json", 4096),
        ("/api/users", "application/json", 2048),
        ("/css/main.css", "text/css", 65536),
        ("/js/app.js", "application/javascript", 131072),
        ("/video/intro.mp4", "video/mp4", 5242880),
        ("/index.html", "text/html", 16384),
        ("/favicon.ico", "image/x-icon", 1024),
        ("/api/config", "application/json", 512),
    ];
    let results = ["Hit", "Miss", "Hit", "Hit", "Miss", "RefreshHit", "Hit"];

    for i in 0..num_lines {
        let (path, ct, size) = paths[i % paths.len()];
        let result = results[i % results.len()];
        let sec = i % 86400;
        let h = sec / 3600;
        let m = (sec % 3600) / 60;
        let s = sec % 60;
        let latency = if result == "Miss" { 0.045 } else { 0.002 };
        let query = if path.starts_with("/api/") {
            "id=1"
        } else {
            "-"
        };

        writeln!(
            f,
            "2026-03-31\t{h:02}:{m:02}:{s:02}\tNRT52\t{size}\t203.0.113.{ip}\tGET\texample.cloudfront.net\t{path}\t200\t-\tMozilla/5.0\t{query}\t-\t{result}\treq{i:06}\texample.cloudfront.net\thttps\t256\t{latency}\t-\tTLSv1.3\tTLS_AES_128_GCM_SHA256\t{result}\tHTTP/2.0\t-\t-\t{port}\t{latency}\t{result}\t{ct}\t{size}\t-\t-",
            ip = i % 254 + 1,
            port = 10000 + i % 50000,
        ).unwrap();
    }
}
