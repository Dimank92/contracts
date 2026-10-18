use soroban_sdk::{contracterror, contracttype, symbol_short, vec, Address, Env, IntoVal, Symbol};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AdapterError {
    NoValidPrice = 1,
    ToleranceExceeded = 2,
    StalePrice = 3,
    HashMismatch = 4,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PriceQuote {
    pub price: i128,
    pub timestamp: u64,
    pub source: Symbol,
}

pub struct MultiSourceOracleAdapter;

impl MultiSourceOracleAdapter {
    pub fn resolve_price(
        env: &Env,
        primary_oracle: &Address,
        fallback_oracle: &Address,
        symbol: Symbol,
        max_staleness: u64,
        tolerance_bps: u32,
    ) -> Result<PriceQuote, AdapterError> {
        let now = env.ledger().timestamp();

        // 1. Query Primary (e.g. Zenith native feeder)
        let primary = Self::read_feed(env, primary_oracle, symbol.clone());
        let fallback = Self::read_feed(env, fallback_oracle, symbol);

        match (primary, fallback) {
            (Some(p), Some(f)) => {
                let p_fresh = now.saturating_sub(p.timestamp) <= max_staleness;
                let f_fresh = now.saturating_sub(f.timestamp) <= max_staleness;

                if p_fresh && f_fresh {
                    let diff = p.price.abs_diff(f.price);
                    let max_diff =
                        p.price.unsigned_abs().saturating_mul(tolerance_bps as u128) / 10_000;
                    if diff > max_diff {
                        return Err(AdapterError::ToleranceExceeded);
                    }
                    Ok(p)
                } else if p_fresh {
                    Ok(p)
                } else if f_fresh {
                    Ok(f)
                } else {
                    Err(AdapterError::StalePrice)
                }
            }
            (Some(p), None) if now.saturating_sub(p.timestamp) <= max_staleness => Ok(p),
            (None, Some(f)) if now.saturating_sub(f.timestamp) <= max_staleness => Ok(f),
            _ => Err(AdapterError::NoValidPrice),
        }
    }

    /// Reads `source.get_price(symbol)` (the price_oracle interface). A
    /// source that reverts, returns `None`, or returns a non-positive
    /// price yields `None` — never a zero or negative "price". The
    /// source's aggregate is already freshness-gated, so the quote is
    /// stamped with the current ledger time.
    fn read_feed(env: &Env, source: &Address, symbol: Symbol) -> Option<PriceQuote> {
        let price = env
            .try_invoke_contract::<Option<i128>, soroban_sdk::Error>(
                source,
                &Symbol::new(env, "get_price"),
                vec![env, symbol.into_val(env)],
            )
            .ok()?
            .ok()??;
        if price <= 0 {
            return None;
        }
        Some(PriceQuote {
            price,
            timestamp: env.ledger().timestamp(),
            source: symbol_short!("SEP40"),
        })
    }
}
