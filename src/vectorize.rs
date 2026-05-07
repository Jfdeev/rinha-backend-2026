use crate::parse::Payload;

#[inline(always)]
fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

pub fn vectorize(p: &Payload) -> [f32; 14] {
    let mut v = [0.0f32; 14];

    // dim 0: amount / 10000
    v[0] = clamp01(p.transaction_amount as f32 / 10000.0);

    // dim 1: installments / 12
    v[1] = clamp01(p.transaction_installments as f32 / 12.0);

    // dim 2: (amount / customer_avg) / 10
    v[2] = if p.customer_avg_amount > 0.0 {
        clamp01((p.transaction_amount as f32 / p.customer_avg_amount as f32) / 10.0)
    } else {
        1.0
    };

    // dim 3: hour_of_day / 23
    let (year, month, day, hour, min, _) = parse_ts(p.requested_at.as_bytes());
    v[3] = hour as f32 / 23.0;

    // dim 4: day_of_week (Mon=0, Sun=6) / 6
    v[4] = weekday_mon0(year, month, day) as f32 / 6.0;

    // dims 5, 6: last_transaction
    match (p.last_tx_timestamp.as_deref(), p.last_tx_km) {
        (Some(ts_last), Some(km_last)) => {
            let cur_min = ts_to_minutes(year, month, day, hour, min);
            let (ly, lmo, ld, lh, lm, _) = parse_ts(ts_last.as_bytes());
            let last_min = ts_to_minutes(ly, lmo, ld, lh, lm);
            let diff = (cur_min - last_min).max(0) as f32;
            v[5] = clamp01(diff / 1440.0);
            v[6] = clamp01(km_last as f32 / 1000.0);
        }
        _ => {
            v[5] = -1.0;
            v[6] = -1.0;
        }
    }

    // dim 7: km_from_home / 1000
    v[7] = clamp01(p.km_from_home as f32 / 1000.0);

    // dim 8: tx_count_24h / 20
    v[8] = clamp01(p.customer_tx_count_24h as f32 / 20.0);

    // dim 9: is_online
    v[9] = if p.is_online { 1.0 } else { 0.0 };

    // dim 10: card_present
    v[10] = if p.card_present { 1.0 } else { 0.0 };

    // dim 11: unknown_merchant (1 = unknown)
    v[11] = if p.merchant_is_known { 0.0 } else { 1.0 };

    // dim 12: mcc_risk
    v[12] = mcc_risk(&p.merchant_mcc);

    // dim 13: merchant_avg_amount / 10000
    v[13] = clamp01(p.merchant_avg_amount as f32 / 10000.0);

    v
}

// Returns (year, month, day, hour, min, sec) from "YYYY-MM-DDTHH:MM:SSZ"
fn parse_ts(b: &[u8]) -> (u32, u32, u32, u32, u32, u32) {
    if b.len() < 19 {
        return (2026, 1, 1, 0, 0, 0);
    }
    let year = d4(b, 0);
    let month = d2(b, 5);
    let day = d2(b, 8);
    let hour = d2(b, 11);
    let min = d2(b, 14);
    let sec = d2(b, 17);
    (year, month, day, hour, min, sec)
}

#[inline(always)]
fn d2(b: &[u8], pos: usize) -> u32 {
    (b[pos] - b'0') as u32 * 10 + (b[pos + 1] - b'0') as u32
}

#[inline(always)]
fn d4(b: &[u8], pos: usize) -> u32 {
    (b[pos] - b'0') as u32 * 1000
        + (b[pos + 1] - b'0') as u32 * 100
        + (b[pos + 2] - b'0') as u32 * 10
        + (b[pos + 3] - b'0') as u32
}

// Tomohiko Sakamoto algorithm — returns 0=Mon, 1=Tue, ..., 6=Sun
fn weekday_mon0(year: u32, month: u32, day: u32) -> u32 {
    static T: [u32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    let dow = (y + y / 4 - y / 100 + y / 400 + T[month as usize - 1] + day) % 7;
    // Sakamoto: 0=Sun → we want 0=Mon
    (dow + 6) % 7
}

// Minutes since a fixed epoch (order-preserving, sign-safe)
fn ts_to_minutes(year: u32, month: u32, day: u32, hour: u32, min: u32) -> i64 {
    let days = civil_to_days(year as i64, month as i64, day as i64);
    days * 1440 + (hour as i64) * 60 + min as i64
}

// Days since 1970-01-01 (Howard Hinnant's civil_from_days inverse)
fn civil_to_days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn mcc_risk(mcc: &str) -> f32 {
    match mcc {
        "5411" => 0.15,
        "5812" => 0.30,
        "5912" => 0.20,
        "5944" => 0.45,
        "7801" => 0.80,
        "7802" => 0.75,
        "7995" => 0.85,
        "4511" => 0.35,
        "5311" => 0.25,
        "5999" => 0.50,
        _ => 0.50,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.0005
    }

    // Legit example from REGRAS_DE_DETECCAO.md
    // Expected: [0.0041, 0.1667, 0.05, 0.7826, 0.3333, -1, -1, 0.0292, 0.15, 0, 1, 0, 0.15, 0.006]
    #[test]
    fn legit_example_null_last_tx() {
        let body = br#"{
            "id": "tx-1329056812",
            "transaction": {"amount": 41.12, "installments": 2, "requested_at": "2026-03-11T18:45:53Z"},
            "customer": {"avg_amount": 82.24, "tx_count_24h": 3, "known_merchants": ["MERC-003","MERC-016"]},
            "merchant": {"id": "MERC-016", "mcc": "5411", "avg_amount": 60.25},
            "terminal": {"is_online": false, "card_present": true, "km_from_home": 29.23},
            "last_transaction": null
        }"#;
        let p = parse(body).unwrap();
        let vec = vectorize(&p);

        assert!(approx_eq(vec[0], 0.0041), "dim0 amount: {}", vec[0]);
        assert!(approx_eq(vec[1], 0.1667), "dim1 installments: {}", vec[1]);
        assert!(approx_eq(vec[2], 0.05), "dim2 amount_vs_avg: {}", vec[2]);
        assert!(approx_eq(vec[3], 0.7826), "dim3 hour: {}", vec[3]);
        assert!(approx_eq(vec[4], 0.3333), "dim4 weekday: {}", vec[4]);
        assert_eq!(vec[5], -1.0, "dim5 sentinel");
        assert_eq!(vec[6], -1.0, "dim6 sentinel");
        assert!(approx_eq(vec[7], 0.0292), "dim7 km_home: {}", vec[7]);
        assert!(approx_eq(vec[8], 0.15), "dim8 tx_count: {}", vec[8]);
        assert_eq!(vec[9], 0.0, "dim9 is_online");
        assert_eq!(vec[10], 1.0, "dim10 card_present");
        assert_eq!(vec[11], 0.0, "dim11 known_merchant");
        assert!(approx_eq(vec[12], 0.15), "dim12 mcc_risk: {}", vec[12]);
        assert!(approx_eq(vec[13], 0.006), "dim13 merch_avg: {}", vec[13]);
    }

    // Fraud example from REGRAS_DE_DETECCAO.md
    // Expected: [0.9506, 0.8333, 1.0, 0.2174, 0.8333, -1, -1, 0.9523, 1.0, 0, 1, 1, 0.75, 0.0055]
    #[test]
    fn fraud_example_null_last_tx() {
        let body = br#"{
            "id": "tx-3330991687",
            "transaction": {"amount": 9505.97, "installments": 10, "requested_at": "2026-03-14T05:15:12Z"},
            "customer": {"avg_amount": 81.28, "tx_count_24h": 20, "known_merchants": ["MERC-008","MERC-007","MERC-005"]},
            "merchant": {"id": "MERC-068", "mcc": "7802", "avg_amount": 54.86},
            "terminal": {"is_online": false, "card_present": true, "km_from_home": 952.27},
            "last_transaction": null
        }"#;
        let p = parse(body).unwrap();
        let vec = vectorize(&p);

        assert!(approx_eq(vec[0], 0.9506), "dim0: {}", vec[0]);
        assert!(approx_eq(vec[1], 0.8333), "dim1: {}", vec[1]);
        assert_eq!(vec[2], 1.0, "dim2 clamped");
        assert!(approx_eq(vec[3], 0.2174), "dim3: {}", vec[3]);
        assert!(approx_eq(vec[4], 0.8333), "dim4: {}", vec[4]);
        assert_eq!(vec[5], -1.0);
        assert_eq!(vec[6], -1.0);
        assert!(approx_eq(vec[7], 0.9523), "dim7: {}", vec[7]);
        assert_eq!(vec[8], 1.0, "dim8 clamped");
        assert_eq!(vec[9], 0.0);
        assert_eq!(vec[10], 1.0);
        assert_eq!(vec[11], 1.0, "dim11 unknown");
        assert!(approx_eq(vec[12], 0.75), "dim12: {}", vec[12]);
        assert!(approx_eq(vec[13], 0.0055), "dim13: {}", vec[13]);
    }

    // Example with last_transaction present — smoke test data
    #[test]
    fn with_last_tx_minutes() {
        let body = br#"{
            "id": "tx-smoke",
            "transaction": {"amount": 384.88, "installments": 3, "requested_at": "2026-03-11T20:23:35Z"},
            "customer": {"avg_amount": 769.76, "tx_count_24h": 3, "known_merchants": ["MERC-009","MERC-001","MERC-001"]},
            "merchant": {"id": "MERC-001", "mcc": "5912", "avg_amount": 298.95},
            "terminal": {"is_online": false, "card_present": true, "km_from_home": 13.7090520965},
            "last_transaction": {"timestamp": "2026-03-11T14:58:35Z", "km_from_current": 18.8626479774}
        }"#;
        let p = parse(body).unwrap();
        let vec = vectorize(&p);

        // 20:23 - 14:58 = 5h25m = 325 minutes; 325/1440 = 0.2257
        assert!(approx_eq(vec[5], 0.2257), "dim5 minutes_since: {}", vec[5]);
        // 18.8626 / 1000 = 0.01886
        assert!(approx_eq(vec[6], 0.01886), "dim6 km_last: {}", vec[6]);
    }
}
