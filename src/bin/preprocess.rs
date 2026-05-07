/// Reads references.json.gz and writes a compact IVF binary:
///
///   4B  magic "RNHI"
///   4B  version u32 = 1
///   4B  n_centroids u32
///   4B  n_vectors u32
///   n_centroids × 14 × 4B  centroids (f32 LE)
///   n_centroids × 4B        bucket_starts (u32 LE)
///   n_centroids × 4B        bucket_counts (u32 LE)
///   n_vectors × 14 × 2B    vectors in bucket order (i16 LE)
///   n_vectors × 1B          labels in bucket order (0=legit, 1=fraud)
use flate2::read::GzDecoder;
use serde::Deserialize;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};

const N_CENTROIDS: usize = 2048;
const KMEANS_ITERS: usize = 40;
const BATCH_SIZE: usize = 100_000;

#[derive(Deserialize)]
struct RefEntry {
    vector: [f32; 14],
    label: String,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let in_path = args.get(1).map(String::as_str).unwrap_or("references.json.gz");
    let out_path = args.get(2).map(String::as_str).unwrap_or("data/refs.bin");

    // --- 1. Load reference data ---
    eprintln!("[preprocess] reading {in_path}");
    let gz = GzDecoder::new(BufReader::new(
        File::open(in_path).unwrap_or_else(|e| panic!("open {in_path}: {e}")),
    ));
    let entries: Vec<RefEntry> =
        serde_json::from_reader(gz).expect("parse references.json.gz");
    let n = entries.len();
    eprintln!("[preprocess] {n} entries");

    let vecs: Vec<[f32; 14]> = entries.iter().map(|e| e.vector).collect();
    let labels: Vec<u8> = entries
        .iter()
        .map(|e| if e.label == "fraud" { 1 } else { 0 })
        .collect();
    drop(entries);

    // --- 2. Build centroids via mini-batch k-means ---
    eprintln!("[preprocess] building {N_CENTROIDS} centroids ({KMEANS_ITERS} iterations)");
    let centroids = kmeans(&vecs, N_CENTROIDS, KMEANS_ITERS, BATCH_SIZE);

    // --- 3. Assign every vector to its nearest centroid ---
    eprintln!("[preprocess] assigning {n} vectors...");
    let assignments: Vec<usize> = vecs
        .iter()
        .map(|v| nearest_centroid(v, &centroids))
        .collect();

    // --- 4. Sort indices by centroid assignment ---
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_unstable_by_key(|&i| assignments[i as usize]);

    // --- 5. Build bucket metadata ---
    let mut bucket_counts = vec![0u32; N_CENTROIDS];
    for &a in &assignments {
        bucket_counts[a] += 1;
    }
    let mut bucket_starts = vec![0u32; N_CENTROIDS];
    let mut acc = 0u32;
    for b in 0..N_CENTROIDS {
        bucket_starts[b] = acc;
        acc += bucket_counts[b];
    }

    let min_bucket = *bucket_counts.iter().min().unwrap();
    let max_bucket = *bucket_counts.iter().max().unwrap();
    eprintln!("[preprocess] bucket sizes: min={min_bucket} max={max_bucket} avg={}", n / N_CENTROIDS);

    // --- 6. Write binary ---
    if let Some(parent) = std::path::Path::new(out_path).parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut out = BufWriter::new(
        File::create(out_path).unwrap_or_else(|e| panic!("create {out_path}: {e}")),
    );

    // Header
    out.write_all(b"RNHI").unwrap();
    out.write_all(&1u32.to_le_bytes()).unwrap();
    out.write_all(&(N_CENTROIDS as u32).to_le_bytes()).unwrap();
    out.write_all(&(n as u32).to_le_bytes()).unwrap();

    // Centroids (f32 LE)
    for c in &centroids {
        for &f in c.iter() {
            out.write_all(&f.to_le_bytes()).unwrap();
        }
    }

    // Bucket starts
    for &s in &bucket_starts {
        out.write_all(&s.to_le_bytes()).unwrap();
    }

    // Bucket counts
    for &c in &bucket_counts {
        out.write_all(&c.to_le_bytes()).unwrap();
    }

    // Vectors in bucket order (i16 LE)
    for &orig in &order {
        let v = &vecs[orig as usize];
        for &f in v.iter() {
            let iv: i16 = if f == -1.0 {
                i16::MIN
            } else {
                (f * 32767.0).round().clamp(0.0, 32767.0) as i16
            };
            out.write_all(&iv.to_le_bytes()).unwrap();
        }
    }

    // Labels in bucket order (u8)
    for &orig in &order {
        out.write_all(&[labels[orig as usize]]).unwrap();
    }

    out.flush().unwrap();

    let fraud_total = labels.iter().filter(|&&l| l == 1).count();
    // 16B header + nc*(56+4+4) + n*(28+1)
    let est_bytes = 16 + N_CENTROIDS * 64 + n * 29;
    eprintln!(
        "[preprocess] wrote {out_path} (~{} MB, {fraud_total} fraud / {} legit)",
        est_bytes / 1_048_576,
        n - fraud_total
    );
}

// ── Mini-batch k-means ────────────────────────────────────────────────────────

fn kmeans(
    vecs: &[[f32; 14]],
    k: usize,
    iters: usize,
    batch_size: usize,
) -> Vec<[f32; 14]> {
    let n = vecs.len();
    assert!(n >= k, "too few vectors for k-means");

    let mut rng = SimpleRng::new(12345);

    // Stride + jitter initialization: one vector per evenly-spaced segment
    let step = n / k;
    let mut centroids: Vec<[f32; 14]> = (0..k)
        .map(|i| {
            let base = i * step;
            let jitter = if step > 1 { rng.next_usize() % step } else { 0 };
            vecs[base + jitter]
        })
        .collect();

    // Per-centroid update count; start at 1 so first lr = 0.5 (not 1.0)
    let mut counts: Vec<u64> = vec![1; k];

    for iter in 0..iters {
        // Random mini-batch
        let batch: Vec<usize> = (0..batch_size)
            .map(|_| rng.next_usize() % n)
            .collect();

        // Assign each batch vector and update its centroid (incremental mean)
        for &idx in &batch {
            let v = &vecs[idx];
            let c = nearest_centroid(v, &centroids);
            counts[c] += 1;
            let lr = 1.0 / counts[c] as f32;
            for i in 0..14 {
                centroids[c][i] += lr * (v[i] - centroids[c][i]);
            }
        }

        if iter % 10 == 0 || iter + 1 == iters {
            eprintln!("[kmeans] iter {}/{iters}", iter + 1);
        }
    }

    centroids
}

fn nearest_centroid(v: &[f32; 14], centroids: &[[f32; 14]]) -> usize {
    let mut best = 0usize;
    let mut best_d = f32::MAX;
    for (i, c) in centroids.iter().enumerate() {
        let d = dist_f32(v, c);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

fn dist_f32(a: &[f32; 14], b: &[f32; 14]) -> f32 {
    let mut s = 0.0f32;
    for i in 0..14 {
        let d = a[i] - b[i];
        s += d * d;
    }
    s
}

// ── Minimal xorshift64 RNG ────────────────────────────────────────────────────

struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next_usize(&mut self) -> usize {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x as usize
    }
}
