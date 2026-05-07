use std::fs::File;
use std::io::{BufReader, Read};

const MAGIC: &[u8; 4] = b"RNHI";
const K_PROBE: usize = 32;

pub struct Database {
    centroids: Box<[[f32; 14]]>,
    bucket_starts: Box<[u32]>,
    bucket_counts: Box<[u32]>,
    vectors: Box<[[i16; 14]]>,
    labels: Box<[u8]>,
}

impl Database {
    pub fn load(path: &str) -> Self {
        eprintln!("[data] loading {path}");
        let mut f = BufReader::with_capacity(
            1 << 20,
            File::open(path).unwrap_or_else(|e| panic!("open {path}: {e}")),
        );

        let mut magic = [0u8; 4];
        f.read_exact(&mut magic).expect("read magic");
        assert_eq!(&magic, MAGIC, "expected RNHI magic in refs.bin");

        let mut buf4 = [0u8; 4];
        f.read_exact(&mut buf4).expect("version");
        let _version = u32::from_le_bytes(buf4);

        f.read_exact(&mut buf4).expect("n_centroids");
        let nc = u32::from_le_bytes(buf4) as usize;

        f.read_exact(&mut buf4).expect("n_vectors");
        let n = u32::from_le_bytes(buf4) as usize;
        eprintln!("[data] {nc} centroids, {n} vectors");

        // Centroids (f32 LE, 14 × 4 bytes each)
        let mut centroids = Vec::with_capacity(nc);
        let mut cbuf = [0u8; 56];
        for _ in 0..nc {
            f.read_exact(&mut cbuf).expect("centroid");
            let mut c = [0f32; 14];
            for (j, cj) in c.iter_mut().enumerate() {
                *cj = f32::from_le_bytes([cbuf[j*4], cbuf[j*4+1], cbuf[j*4+2], cbuf[j*4+3]]);
            }
            centroids.push(c);
        }

        // Bucket starts
        let mut bucket_starts = vec![0u32; nc];
        for s in bucket_starts.iter_mut() {
            f.read_exact(&mut buf4).expect("bucket_start");
            *s = u32::from_le_bytes(buf4);
        }

        // Bucket counts
        let mut bucket_counts = vec![0u32; nc];
        for c in bucket_counts.iter_mut() {
            f.read_exact(&mut buf4).expect("bucket_count");
            *c = u32::from_le_bytes(buf4);
        }

        // Vectors (i16 LE, 14 × 2 bytes each, sorted by bucket)
        let mut vectors = Vec::with_capacity(n);
        let mut entry = [0u8; 28];
        for _ in 0..n {
            f.read_exact(&mut entry).expect("vector");
            let mut v = [0i16; 14];
            for (j, vj) in v.iter_mut().enumerate() {
                *vj = i16::from_le_bytes([entry[j*2], entry[j*2+1]]);
            }
            vectors.push(v);
        }

        // Labels (u8, same order as vectors)
        let mut labels = vec![0u8; n];
        f.read_exact(&mut labels).expect("labels");

        let fraud = labels.iter().filter(|&&l| l == 1).count();
        eprintln!("[data] ready — {fraud} fraud, {} legit", n - fraud);

        Database {
            centroids: centroids.into_boxed_slice(),
            bucket_starts: bucket_starts.into_boxed_slice(),
            bucket_counts: bucket_counts.into_boxed_slice(),
            vectors: vectors.into_boxed_slice(),
            labels: labels.into_boxed_slice(),
        }
    }

    pub fn knn_fraud_count(&self, query: &[f32; 14]) -> u8 {
        let nc = self.centroids.len();

        // Find K_PROBE nearest centroids by linear scan
        let mut top_probe = [(f32::MAX, 0usize); K_PROBE];
        for (i, c) in self.centroids.iter().enumerate() {
            let d = dist_f32(query, c);
            if d < top_probe[K_PROBE - 1].0 {
                top_probe[K_PROBE - 1] = (d, i);
                let mut k = K_PROBE - 1;
                while k > 0 && top_probe[k].0 < top_probe[k - 1].0 {
                    top_probe.swap(k, k - 1);
                    k -= 1;
                }
            }
        }

        // Scan vectors in those K_PROBE buckets, track top-5 by i16 distance
        let q = quantize(query);
        let mut top = [(i64::MAX, 0u8); 5];

        for &(_, bi) in &top_probe {
            if bi >= nc {
                continue;
            }
            let start = self.bucket_starts[bi] as usize;
            let count = self.bucket_counts[bi] as usize;
            for i in start..start + count {
                let d = dist2(&q, unsafe { self.vectors.get_unchecked(i) });
                if d < top[4].0 {
                    top[4] = (d, unsafe { *self.labels.get_unchecked(i) });
                    let mut k = 4;
                    while k > 0 && top[k].0 < top[k - 1].0 {
                        top.swap(k, k - 1);
                        k -= 1;
                    }
                }
            }
        }

        top.iter().filter(|&&(_, l)| l == 1).count() as u8
    }
}

fn quantize(v: &[f32; 14]) -> [i16; 14] {
    let mut q = [0i16; 14];
    for (i, &f) in v.iter().enumerate() {
        q[i] = if f == -1.0 {
            i16::MIN
        } else {
            (f * 32767.0).round().clamp(0.0, 32767.0) as i16
        };
    }
    q
}

fn dist_f32(a: &[f32; 14], b: &[f32; 14]) -> f32 {
    let mut s = 0.0f32;
    for i in 0..14 {
        let d = a[i] - b[i];
        s += d * d;
    }
    s
}

#[inline(always)]
fn dist2(a: &[i16; 14], b: &[i16; 14]) -> i64 {
    #[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
    unsafe {
        return dist2_avx2(a, b);
    }
    #[allow(unreachable_code)]
    {
        let mut s = 0i64;
        for i in 0..14 {
            let d = a[i] as i64 - b[i] as i64;
            s += d * d;
        }
        s
    }
}

#[cfg(all(target_arch = "x86_64", target_feature = "avx2"))]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn dist2_avx2(a: &[i16; 14], b: &[i16; 14]) -> i64 {
    use std::arch::x86_64::*;

    // Mask: keep first 14 i16 lanes, zero the last 2 (which read past the
    // logical end of [i16; 14] into adjacent memory).
    // Each i16 lane = 0xFFFF if kept, 0x0000 if zeroed.
    const MASK: [i16; 16] = [
        -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0,
    ];
    let mask = _mm256_loadu_si256(MASK.as_ptr() as *const __m256i);

    let av = _mm256_and_si256(_mm256_loadu_si256(a.as_ptr() as *const __m256i), mask);
    let bv = _mm256_and_si256(_mm256_loadu_si256(b.as_ptr() as *const __m256i), mask);

    let diff = _mm256_sub_epi16(av, bv);
    // 16 × i16 → 8 × i32, each = (d_2i)^2 + (d_2i+1)^2
    let sq = _mm256_madd_epi16(diff, diff);

    // Widen 8 × i32 → 4+4 × i64 to avoid overflow on horizontal sum
    let lo = _mm256_cvtepi32_epi64(_mm256_castsi256_si128(sq));
    let hi = _mm256_cvtepi32_epi64(_mm256_extracti128_si256::<1>(sq));
    let sum64 = _mm256_add_epi64(lo, hi);

    // Horizontal sum of 4 × i64
    let sum_hi = _mm256_extracti128_si256::<1>(sum64);
    let sum_lo = _mm256_castsi256_si128(sum64);
    let s2 = _mm_add_epi64(sum_lo, sum_hi);
    let s1 = _mm_add_epi64(s2, _mm_unpackhi_epi64(s2, s2));
    _mm_cvtsi128_si64(s1)
}
