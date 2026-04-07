use std::io::Write;
use std::process::Command;

fn qc() -> Command {
    Command::new(env!("CARGO_BIN_EXE_qc"))
}

fn write_file(path: &std::path::Path, content: &str) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    f.write_all(content.as_bytes()).unwrap();
}

// ── Cloudflare Parser ──────────────────────────────────────────────

#[test]
fn cloudflare_import_valid_ndjson() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("cf.ndjson");
    let trace_path = dir.path().join("trace.csv");

    write_file(
        &log_path,
        &[
            r#"{"EdgeStartTimestamp":1711843200000000000,"ClientRequestURI":"/img/logo.png","EdgeResponseBytes":1024,"EdgeResponseStatus":200,"CacheCacheStatus":"hit","OriginResponseTime":5000000}"#,
            r#"{"EdgeStartTimestamp":1711843201000000000,"ClientRequestURI":"/api/data","EdgeResponseBytes":512,"EdgeResponseStatus":200,"CacheCacheStatus":"miss","OriginResponseTime":50000000,"EdgeResponseContentType":"application/json"}"#,
        ].join("\n"),
    );

    let out = qc()
        .args(["import", "-p", "cloudflare", "-i"])
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
    let msg = String::from_utf8_lossy(&out.stdout);
    assert!(msg.contains("Imported 2 events"), "got: {msg}");

    // Verify CSV is readable
    let csv_content = std::fs::read_to_string(&trace_path).unwrap();
    assert!(csv_content.contains("/img/logo.png"));
    assert!(csv_content.contains("/api/data"));
}

#[test]
fn cloudflare_import_skips_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("bad.ndjson");
    let trace_path = dir.path().join("trace.csv");

    write_file(
        &log_path,
        &[
            "not json",
            r#"{"EdgeStartTimestamp":1711843200000000000,"ClientRequestURI":"/ok","EdgeResponseBytes":100,"EdgeResponseStatus":200,"CacheCacheStatus":"hit","OriginResponseTime":0}"#,
            "{broken",
        ].join("\n"),
    );

    let out = qc()
        .args(["import", "-p", "cloudflare", "-i"])
        .arg(&log_path)
        .args(["-o"])
        .arg(&trace_path)
        .output()
        .unwrap();

    assert!(out.status.success());
    let msg = String::from_utf8_lossy(&out.stdout);
    assert!(msg.contains("Imported 1 events"), "got: {msg}");
}

// ── Fastly Parser ──────────────────────────────────────────────────

#[test]
fn fastly_import_valid_ndjson() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("fastly.ndjson");
    let trace_path = dir.path().join("trace.csv");

    write_file(
        &log_path,
        &[
            r#"{"timestamp":1711843200,"url":"GET /assets/style.css HTTP/1.1","status":200,"resp_body_bytes":2048,"cache_status":"HIT","time_elapsed":1500,"content_type":"text/css"}"#,
            r#"{"timestamp":1711843201,"url":"/api/v1/users","status":200,"resp_body_bytes":1024,"cache_status":"MISS","time_elapsed":45000,"content_type":"application/json"}"#,
            r#"{"timestamp":1711843202,"url":"DELETE /api/v1/session HTTP/2","status":204,"resp_body_bytes":0,"cache_status":"PASS","time_elapsed":2000}"#,
        ].join("\n"),
    );

    let out = qc()
        .args(["import", "-p", "fastly", "-i"])
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
    let msg = String::from_utf8_lossy(&out.stdout);
    assert!(msg.contains("Imported 3 events"), "got: {msg}");

    let csv_content = std::fs::read_to_string(&trace_path).unwrap();
    // Fastly request line "GET /path HTTP/1.1" should extract "/assets/style.css"
    assert!(csv_content.contains("/assets/style.css"));
    // Plain URL should pass through
    assert!(csv_content.contains("/api/v1/users"));
    // DELETE method should extract path
    assert!(csv_content.contains("/api/v1/session"));
}

#[test]
fn fastly_import_handles_empty() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("empty.ndjson");
    let trace_path = dir.path().join("trace.csv");

    write_file(&log_path, "");

    let out = qc()
        .args(["import", "-p", "fastly", "-i"])
        .arg(&log_path)
        .args(["-o"])
        .arg(&trace_path)
        .output()
        .unwrap();

    assert!(out.status.success());
    let msg = String::from_utf8_lossy(&out.stdout);
    assert!(msg.contains("Imported 0 events"), "got: {msg}");
}

// ── Unsupported provider ───────────────────────────────────────────

#[test]
fn import_rejects_unknown_provider() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("dummy.log");
    write_file(&log_path, "data");

    let out = qc()
        .args(["import", "-p", "akamai", "-i"])
        .arg(&log_path)
        .args(["-o", "/dev/null"])
        .output()
        .unwrap();

    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("unsupported provider") || err.contains("Supported"),
        "got: {err}"
    );
}
