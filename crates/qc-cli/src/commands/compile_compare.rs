use std::io::Write;
use std::path::PathBuf;

use clap::Args;

#[derive(Args)]
pub struct CompileCompareArgs {
    /// PolicyIR JSON file
    #[arg(short, long)]
    pub policy: PathBuf,

    /// Scores file (PolicyFile JSON from `qc optimize`)
    #[arg(long)]
    pub scores: Option<PathBuf>,

    /// Output directory for all target configs
    #[arg(short, long, default_value = ".")]
    pub output_dir: PathBuf,
}

pub fn run(args: &CompileCompareArgs) -> anyhow::Result<()> {
    let targets = ["cloudflare", "cloudfront", "fastly"];
    let extensions = ["cloudflare.json", "cloudfront.json", "fastly.json"];

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    writeln!(out, "Cross-CDN Compilation")?;
    writeln!(out, "=====================")?;
    writeln!(out)?;

    std::fs::create_dir_all(&args.output_dir)?;
    let mut results = Vec::new();

    for (target, ext) in targets.iter().zip(extensions.iter()) {
        let output = args.output_dir.join(ext);
        let mut compile_args = vec![
            "compile".to_string(),
            "-p".to_string(),
            args.policy.to_string_lossy().to_string(),
            "-t".to_string(),
            target.to_string(),
            "-o".to_string(),
            output.to_string_lossy().to_string(),
            "--validate".to_string(),
        ];
        if let Some(ref scores) = args.scores {
            compile_args.push("--scores".to_string());
            compile_args.push(scores.to_string_lossy().to_string());
        }

        // Run compile internally
        let compile_result = super::compile::CompileArgs {
            policy: args.policy.clone(),
            target: target.to_string(),
            scores: args.scores.clone(),
            output: output.clone(),
            validate: true,
        };

        let status = super::compile::run(&compile_result);
        let (valid, issues) = match &status {
            Ok(()) => ("PASS".to_string(), 0),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("validation failed") {
                    let n = msg
                        .split("with ")
                        .nth(1)
                        .and_then(|s| s.split(' ').next())
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(1);
                    (format!("FAIL ({n} issues)"), n)
                } else {
                    ("ERROR".to_string(), 1)
                }
            }
        };

        results.push((target.to_string(), output.clone(), valid, issues));
    }

    writeln!(out)?;
    writeln!(out, "Summary")?;
    writeln!(out, "-------")?;
    writeln!(
        out,
        "{:<15} {:<40} {:>10}",
        "Target", "Output", "Validation"
    )?;
    writeln!(out, "{}", "-".repeat(68))?;
    for (target, output, valid, _) in &results {
        writeln!(out, "{:<15} {:<40} {:>10}", target, output.display(), valid)?;
    }

    let all_pass = results.iter().all(|(_, _, _, issues)| *issues == 0);
    writeln!(out)?;
    if all_pass {
        writeln!(out, "All targets: PASS")?;
    } else {
        writeln!(out, "Some targets have validation issues.")?;
    }

    Ok(())
}
