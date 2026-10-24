use crate::config::SymbolCfg;

/// A price report: `price` is an integer scaled by `10^decimals`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub symbol: String,
    pub price: i128,
    pub decimals: u32,
    pub interval_id: u64,
}

impl Report {
    pub fn build(sym: &SymbolCfg, interval_id: u64) -> Result<Report, String> {
        Ok(Report {
            symbol: sym.name.clone(),
            price: scale(&sym.price, sym.decimals)?,
            decimals: sym.decimals,
            interval_id,
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        // Price as a string: i128 does not fit a JSON number.
        serde_json::json!({
            "symbol": self.symbol,
            "price": self.price.to_string(),
            "decimals": self.decimals,
            "interval_id": self.interval_id,
        })
    }
}

/// Parses a decimal string into an integer scaled by `10^decimals`.
/// Rejects non-positive values and more fractional digits than `decimals`
/// (silent rounding would hide a misconfigured symbol).
pub fn scale(price: &str, decimals: u32) -> Result<i128, String> {
    let (int, frac) = price.split_once('.').unwrap_or((price, ""));
    if int.is_empty() && frac.is_empty()
        || !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit())
    {
        return Err(format!("invalid price {price:?}"));
    }
    if frac.len() as u32 > decimals {
        return Err(format!("price {price:?} has more than {decimals} decimals"));
    }
    let digits = format!(
        "{int}{frac}{}",
        "0".repeat((decimals as usize) - frac.len())
    );
    let v: i128 = digits
        .parse()
        .map_err(|_| format!("price {price:?} out of range"))?;
    if v <= 0 {
        return Err("price must be positive".into());
    }
    Ok(v)
}
