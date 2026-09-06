//! Run with: cargo run --release -p ds-engine --example fingerprint_bench
//! Creates only its own temporary fixture; never opens the user's downloads.
use anyhow::{ensure, Result};
use ds_engine::safe_fs::{fingerprint, fingerprint_matches};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Write,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

fn main() -> Result<()> {
    ensure!(
        !cfg!(debug_assertions),
        "Use --release for comparable timings"
    );
    let path =
        std::env::temp_dir().join(format!("ds-fingerprint-bench-{}.bin", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = File::create_new(&path)?;
        let block: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
        let mut legacy = Sha256::new();
        let mut full = blake3::Hasher::new();
        for _ in 0..256 {
            file.write_all(&block)?;
            legacy.update(&block);
            full.update(&block);
        }
        file.sync_all()?;
        drop(file);
        let legacy = format!("{:x}", legacy.finalize());
        let full = format!("blake3:{}", full.finalize().to_hex());
        let cancel = CancellationToken::new();
        ensure!(
            fingerprint_matches(&path, &full, &cancel)?,
            "Warm-cache full hash mismatch"
        );
        let expected = fingerprint(&path, &cancel)?;
        let mut sha_ms = Vec::new();
        let mut blake_ms = Vec::new();
        let mut sampled_ms = Vec::new();
        for round in 0..9 {
            let started = Instant::now();
            if round % 3 == 0 {
                ensure!(
                    fingerprint_matches(&path, &legacy, &cancel)?,
                    "SHA-256 mismatch"
                );
                sha_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            } else if round % 3 == 1 {
                ensure!(
                    fingerprint_matches(&path, &full, &cancel)?,
                    "Full BLAKE3 mismatch"
                );
                blake_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            } else {
                ensure!(
                    fingerprint(&path, &cancel)? == expected,
                    "Sampled BLAKE3 mismatch"
                );
                sampled_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            }
        }
        sha_ms.sort_by(f64::total_cmp);
        blake_ms.sort_by(f64::total_cmp);
        sampled_ms.sort_by(f64::total_cmp);
        println!(
            "{}",
            serde_json::json!({"fixture_mib":256,"build":"release","cache":"warm OS file cache","samples_per_algorithm":3,"sha256_full_median_ms":sha_ms[1],"blake3_full_median_ms":blake_ms[1],"blake3_sampled_median_ms":sampled_ms[1],"sampled_bytes":5 * 64 * 1024,"note":"Full and sampled fingerprints provide different verification coverage; cached timings do not predict cold-disk latency."})
        );
        Ok(())
    })();
    let _ = fs::remove_file(&path);
    result
}
