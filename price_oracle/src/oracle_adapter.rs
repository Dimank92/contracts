//! The boundary between raw price submissions and the price consumers
//! read. Every report enters through [`ingest`] and every aggregate
//! leaves through [`current_price`]; nothing else touches stored reports.
//!
//! # Ingest
//!
//! A [`FeederReport`] is accepted only if:
//!
//! 1. its symbol is known — has a declared precision, see
//!    [`symbol_precision`] (XLM, BTC, ETH and SOL are seeded at
//!    `initialize`) — else `UnknownSymbol`;
//! 2. its price is in `1..=MAX_PRICE` (after normalization to
//!    `PRICE_DECIMALS`) — else `InvalidPrice`;
//! 3. its price carries no more decimals than the symbol declares — else
//!    `PrecisionExceeded`;
//! 4. its timestamp is not in the future — else `FutureTimestamp` — and
//!    is neither already outside the freshness window nor older than the
//!    feeder's previous report for the symbol — else `StaleReport`.
//!
//! # Read path
//!
//! [`current_price`] returns an [`OraclePrice`] at `PRICE_DECIMALS` (the
//! `PRICE_PRECISION` options_market settles in): the median of every
//! currently authorized feeder's report no older than `max_staleness`,
//! provided at least `min_reports` of them are fresh. It never returns a
//! price older than the freshness window. The contract's `get_price`
//! is `current_price(..).ok().map(|p| p.price)` — exactly the
//! `Option<i128>` `options_market::set_settlement_price_from_oracle`
//! cross-calls, so both settlement paths see one price type and one
//! validation rule.
//!
//! [`MultiSourceOracleAdapter`] applies the same price validation to
//! prices read from *other* oracle contracts (primary/fallback).

use soroban_sdk::{contracterror, contracttype, Address, Env, IntoVal, Symbol};

use crate::error::Error;
use crate::math::{self, MathError, MAX_FEEDERS, MAX_PRICE, PRICE_DECIMALS};
use crate::types::DataKey;
use crate::{events, storage, ttl};

/// Symbols every deployment knows from `initialize`, each quoted with the
/// full `PRICE_DECIMALS` precision. The admin can add more (or narrow
/// these) with `set_symbol_precision`.
pub const DEFAULT_SYMBOLS: [&str; 4] = ["XLM", "BTC", "ETH", "SOL"];

/// One feeder's price for one symbol at one time, at `PRICE_DECIMALS`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeederReport {
    pub symbol: Symbol,
    pub feeder: Address,
    pub price: i128,
    /// Ledger timestamp (seconds) the price was observed at.
    pub timestamp: u64,
}

/// The aggregate for a symbol, as handed to consumers.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OraclePrice {
    pub symbol: Symbol,
    /// Median of the fresh reports, at `decimals` implied decimals.
    pub price: i128,
    /// Always `PRICE_DECIMALS`.
    pub decimals: u32,
    /// Timestamp of the *oldest* report that went into `price`, so a
    /// consumer can bound its age; never older than `max_staleness`.
    pub timestamp: u64,
    /// How many fresh reports the median was taken over.
    pub report_count: u32,
}

fn math_error(e: MathError) -> Error {
    match e {
        MathError::EmptySet => Error::NoReports,
        MathError::Overflow => Error::ArithmeticOverflow,
        MathError::InvalidDecimals => Error::InvalidDecimals,
        MathError::InvalidReference => Error::InvalidPrice,
    }
}

// ─── symbol registry ─────────────────────────────────────────────────────────

/// Declared precision of `symbol`, or `None` if it isn't a known symbol.
pub fn symbol_precision(env: &Env, symbol: &Symbol) -> Option<u32> {
    ttl::get_persistent(env, &DataKey::SymbolPrecision(symbol.clone()))
}

/// Declares `symbol` known with `precision` meaningful decimals
/// (`0..=PRICE_DECIMALS`). The caller must already have checked auth.
pub fn set_symbol_precision(env: &Env, symbol: &Symbol, precision: u32) -> Result<(), Error> {
    if precision > PRICE_DECIMALS {
        return Err(Error::InvalidDecimals);
    }
    ttl::set_persistent(env, &DataKey::SymbolPrecision(symbol.clone()), &precision);
    events::symbol_precision_set(env, symbol.clone(), precision);
    Ok(())
}

/// Seeds [`DEFAULT_SYMBOLS`] at full precision. Called once by `initialize`.
pub fn seed_default_symbols(env: &Env) {
    for name in DEFAULT_SYMBOLS {
        let symbol = Symbol::new(env, name);
        ttl::set_persistent(env, &DataKey::SymbolPrecision(symbol), &PRICE_DECIMALS);
    }
}

// ─── ingest ──────────────────────────────────────────────────────────────────

/// The feeder's latest stored report for `symbol`, as `(price, timestamp)`.
pub fn latest_report(env: &Env, symbol: &Symbol, feeder: &Address) -> Option<(i128, u64)> {
    ttl::get_persistent(env, &DataKey::PriceReport(symbol.clone(), feeder.clone()))
}

/// Builds a report from a price quoted with `decimals` implied decimals,
/// normalizing it to `PRICE_DECIMALS` (rounding down).
pub fn normalize(
    symbol: Symbol,
    feeder: Address,
    price: i128,
    decimals: u32,
    timestamp: u64,
) -> Result<FeederReport, Error> {
    let price = math::normalize_price(price, decimals).map_err(math_error)?;
    Ok(FeederReport {
        symbol,
        feeder,
        price,
        timestamp,
    })
}

/// Checks `report` against every ingest rule (see the module docs)
/// without storing anything.
pub fn validate(env: &Env, report: &FeederReport, max_staleness: u64) -> Result<(), Error> {
    let precision = symbol_precision(env, &report.symbol).ok_or(Error::UnknownSymbol)?;
    if report.price <= 0 || report.price > MAX_PRICE {
        return Err(Error::InvalidPrice);
    }
    if !math::check_precision(report.price, precision).map_err(math_error)? {
        return Err(Error::PrecisionExceeded);
    }
    let now = env.ledger().timestamp();
    if report.timestamp > now {
        return Err(Error::FutureTimestamp);
    }
    if now - report.timestamp > max_staleness {
        return Err(Error::StaleReport);
    }
    if let Some((_, previous)) = latest_report(env, &report.symbol, &report.feeder) {
        if report.timestamp < previous {
            return Err(Error::StaleReport);
        }
    }
    Ok(())
}

/// Validates and stores `report` as its feeder's latest for the symbol.
/// The caller must already have checked the feeder's auth and membership.
pub fn ingest(env: &Env, report: FeederReport, max_staleness: u64) -> Result<(), Error> {
    validate(env, &report, max_staleness)?;
    ttl::set_persistent(
        env,
        &DataKey::PriceReport(report.symbol.clone(), report.feeder.clone()),
        &(report.price, report.timestamp),
    );
    events::price_reported(env, report.feeder, report.symbol, report.price);
    Ok(())
}

// ─── read path ───────────────────────────────────────────────────────────────

/// The current aggregate for `symbol` (see the module docs).
///
/// Errors: `NoReports` if no current feeder ever reported it,
/// `StalePrice` if every report is older than `max_staleness`, and
/// `BelowQuorum` if fewer than `min_reports` are fresh.
pub fn current_price(
    env: &Env,
    symbol: &Symbol,
    max_staleness: u64,
    min_reports: u32,
) -> Result<OraclePrice, Error> {
    let now = env.ledger().timestamp();
    let mut buffer = [0i128; MAX_FEEDERS as usize];
    let mut fresh = 0usize;
    let mut seen = false;
    let mut oldest = u64::MAX;
    for feeder in storage::feeder_list(env).iter() {
        let Some((price, reported_at)) = latest_report(env, symbol, &feeder) else {
            continue;
        };
        seen = true;
        // A timestamp ahead of `now` can't pass ingest; count it stale
        // rather than trusting it if it ever appears.
        let fresh_enough = now
            .checked_sub(reported_at)
            .is_some_and(|age| age <= max_staleness);
        if fresh_enough && fresh < buffer.len() {
            buffer[fresh] = price;
            fresh += 1;
            oldest = oldest.min(reported_at);
        }
    }
    if !seen {
        return Err(Error::NoReports);
    }
    if fresh == 0 {
        return Err(Error::StalePrice);
    }
    if (fresh as u32) < min_reports {
        return Err(Error::BelowQuorum);
    }
    let price = math::median(&mut buffer[..fresh]).map_err(math_error)?;
    Ok(OraclePrice {
        symbol: symbol.clone(),
        price,
        decimals: PRICE_DECIMALS,
        timestamp: oldest,
        report_count: fresh as u32,
    })
}

// ─── multi-source reads from other oracle contracts ──────────────────────────

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

/// Reads `get_price(symbol) -> Option<i128>` from two oracle contracts
/// and reconciles them. Each source is held to the same price rule as
/// [`ingest`] (`1..=MAX_PRICE`); a source that reverts, returns `None`,
/// returns the wrong type, or returns an invalid price counts as having
/// no price, never as zero.
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
        let fresh = |q: &PriceQuote| {
            now.checked_sub(q.timestamp)
                .is_some_and(|age| age <= max_staleness)
        };

        let primary = Self::read_feed(env, primary_oracle, symbol.clone());
        let fallback = Self::read_feed(env, fallback_oracle, symbol);

        match (primary, fallback) {
            (Some(p), Some(f)) => match (fresh(&p), fresh(&f)) {
                (true, true) => match math::within_deviation(f.price, p.price, tolerance_bps) {
                    Ok(true) => Ok(p),
                    _ => Err(AdapterError::ToleranceExceeded),
                },
                (true, false) => Ok(p),
                (false, true) => Ok(f),
                (false, false) => Err(AdapterError::StalePrice),
            },
            (Some(p), None) if fresh(&p) => Ok(p),
            (None, Some(f)) if fresh(&f) => Ok(f),
            (Some(_), None) | (None, Some(_)) => Err(AdapterError::StalePrice),
            (None, None) => Err(AdapterError::NoValidPrice),
        }
    }

    /// One source's price, or `None` if it has no valid one. A price
    /// oracle's `get_price` already applies its own staleness window and
    /// returns no timestamp, so the quote is stamped with the read time.
    fn read_feed(env: &Env, source: &Address, symbol: Symbol) -> Option<PriceQuote> {
        let result = env.try_invoke_contract::<Option<i128>, soroban_sdk::Error>(
            source,
            &Symbol::new(env, "get_price"),
            soroban_sdk::vec![env, symbol.into_val(env)],
        );
        let price = match result {
            Ok(Ok(Some(price))) => price,
            _ => return None,
        };
        if price <= 0 || price > MAX_PRICE {
            return None;
        }
        Some(PriceQuote {
            price,
            timestamp: env.ledger().timestamp(),
            source: Symbol::new(env, "SEP40"),
        })
    }
}
