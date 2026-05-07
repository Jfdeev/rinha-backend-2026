use serde_json::Value;

pub struct Payload {
    pub transaction_amount: f64,
    pub transaction_installments: f64,
    pub requested_at: String,
    pub customer_avg_amount: f64,
    pub customer_tx_count_24h: f64,
    pub merchant_is_known: bool,
    pub merchant_mcc: String,
    pub merchant_avg_amount: f64,
    pub is_online: bool,
    pub card_present: bool,
    pub km_from_home: f64,
    pub last_tx_timestamp: Option<String>,
    pub last_tx_km: Option<f64>,
}

pub fn parse(body: &[u8]) -> Option<Payload> {
    let v: Value = serde_json::from_slice(body).ok()?;

    let t = v.get("transaction")?;
    let c = v.get("customer")?;
    let m = v.get("merchant")?;
    let term = v.get("terminal")?;

    let merchant_id = m.get("id")?.as_str()?;
    let known_merchants = c.get("known_merchants")?.as_array()?;
    let merchant_is_known = known_merchants.iter().any(|x| x.as_str() == Some(merchant_id));

    let last = v.get("last_transaction").and_then(Value::as_object);

    Some(Payload {
        transaction_amount: t.get("amount")?.as_f64()?,
        transaction_installments: t.get("installments")?.as_f64()?,
        requested_at: t.get("requested_at")?.as_str()?.to_owned(),
        customer_avg_amount: c.get("avg_amount")?.as_f64()?,
        customer_tx_count_24h: c.get("tx_count_24h")?.as_f64()?,
        merchant_is_known,
        merchant_mcc: m.get("mcc")?.as_str()?.to_owned(),
        merchant_avg_amount: m.get("avg_amount")?.as_f64()?,
        is_online: term.get("is_online")?.as_bool()?,
        card_present: term.get("card_present")?.as_bool()?,
        km_from_home: term.get("km_from_home")?.as_f64()?,
        last_tx_timestamp: last
            .and_then(|o| o.get("timestamp"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        last_tx_km: last
            .and_then(|o| o.get("km_from_current"))
            .and_then(Value::as_f64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legit_null_last_tx() {
        let body = br#"{
            "id": "tx-1",
            "transaction": {"amount": 41.12, "installments": 2, "requested_at": "2026-03-11T18:45:53Z"},
            "customer": {"avg_amount": 82.24, "tx_count_24h": 3, "known_merchants": ["MERC-003","MERC-016"]},
            "merchant": {"id": "MERC-016", "mcc": "5411", "avg_amount": 60.25},
            "terminal": {"is_online": false, "card_present": true, "km_from_home": 29.23},
            "last_transaction": null
        }"#;
        let p = parse(body).unwrap();
        assert!((p.transaction_amount - 41.12).abs() < 1e-9);
        assert_eq!(p.transaction_installments as u32, 2);
        assert_eq!(p.requested_at, "2026-03-11T18:45:53Z");
        assert!((p.customer_avg_amount - 82.24).abs() < 1e-9);
        assert_eq!(p.customer_tx_count_24h as u32, 3);
        assert!(p.merchant_is_known);
        assert_eq!(p.merchant_mcc, "5411");
        assert!((p.merchant_avg_amount - 60.25).abs() < 1e-9);
        assert!(!p.is_online);
        assert!(p.card_present);
        assert!((p.km_from_home - 29.23).abs() < 1e-9);
        assert!(p.last_tx_timestamp.is_none());
        assert!(p.last_tx_km.is_none());
    }

    #[test]
    fn parses_with_last_tx() {
        let body = br#"{
            "id": "tx-2",
            "transaction": {"amount": 384.88, "installments": 3, "requested_at": "2026-03-11T20:23:35Z"},
            "customer": {"avg_amount": 769.76, "tx_count_24h": 3, "known_merchants": ["MERC-009","MERC-001","MERC-001"]},
            "merchant": {"id": "MERC-001", "mcc": "5912", "avg_amount": 298.95},
            "terminal": {"is_online": false, "card_present": true, "km_from_home": 13.7090520965},
            "last_transaction": {"timestamp": "2026-03-11T14:58:35Z", "km_from_current": 18.8626479774}
        }"#;
        let p = parse(body).unwrap();
        assert_eq!(p.last_tx_timestamp.as_deref(), Some("2026-03-11T14:58:35Z"));
        assert!((p.last_tx_km.unwrap() - 18.8626479774).abs() < 1e-9);
    }

    #[test]
    fn unknown_merchant() {
        let body = br#"{
            "id": "tx-3",
            "transaction": {"amount": 100.0, "installments": 1, "requested_at": "2026-03-14T05:15:12Z"},
            "customer": {"avg_amount": 81.28, "tx_count_24h": 20, "known_merchants": ["MERC-008","MERC-007"]},
            "merchant": {"id": "MERC-068", "mcc": "7802", "avg_amount": 54.86},
            "terminal": {"is_online": false, "card_present": true, "km_from_home": 952.27},
            "last_transaction": null
        }"#;
        let p = parse(body).unwrap();
        assert!(!p.merchant_is_known);
    }
}
