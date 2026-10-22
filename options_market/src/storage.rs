use soroban_sdk::{contracttype, Address, Env, Vec};

use crate::types::{MarketError, Position, PositionStatus};

/// Storage keys for the options market contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    /// Global admin address.
    Admin,
    /// Whether the contract is paused.
    Paused,
    /// Monotonic counter used to allocate position ids.
    NextPositionId,
    /// A single position record keyed by position id.
    Position(u64),
    /// List of position ids owned by a writer for a given series.
    WriterPositions(Address, u64),
    /// Aggregate recorded collateral for all live positions of a series.
    SeriesCollateral(u64),
    /// Series configuration / metadata keyed by series id.
    Series(u64),
    /// Whether a series is halted or in emergency state.
    SeriesHalted(u64),
}

/// Persist a position record.
pub fn write_position(env: &Env, position: &Position) {
    env.storage()
        .persistent()
        .set(&DataKey::Position(position.position_id), position);
}

/// Read a position record, returning an error if it does not exist.
pub fn read_position(env: &Env, position_id: u64) -> Result<Position, MarketError> {
    env.storage()
        .persistent()
        .get(&DataKey::Position(position_id))
        .ok_or(MarketError::PositionNotFound)
}

/// Read a position record if present.
pub fn maybe_read_position(env: &Env, position_id: u64) -> Option<Position> {
    env.storage()
        .persistent()
        .get(&DataKey::Position(position_id))
}

/// Remove a position record from storage.
pub fn remove_position(env: &Env, position_id: u64) {
    env.storage()
        .persistent()
        .remove(&DataKey::Position(position_id));
}

/// Allocate the next position id.
pub fn next_position_id(env: &Env) -> u64 {
    let id: u64 = env
        .storage()
        .instance()
        .get(&DataKey::NextPositionId)
        .unwrap_or(0);
    env.storage()
        .instance()
        .set(&DataKey::NextPositionId, &(id + 1));
    id
}

/// Register a position id under a writer for a series.
pub fn add_writer_position(env: &Env, writer: &Address, series_id: u64, position_id: u64) {
    let key = DataKey::WriterPositions(writer.clone(), series_id);
    let mut ids: Vec<u64> = env.storage().persistent().get(&key).unwrap_or(Vec::new(env));
    ids.push_back(position_id);
    env.storage().persistent().set(&key, &ids);
}

/// Remove a position id from a writer's list for a series.
pub fn remove_writer_position(env: &Env, writer: &Address, series_id: u64, position_id: u64) {
    let key = DataKey::WriterPositions(writer.clone(), series_id);
    let ids: Vec<u64> = env.storage().persistent().get(&key).unwrap_or(Vec::new(env));
    let mut updated: Vec<u64> = Vec::new(env);
    for id in ids.iter() {
        if id != position_id {
            updated.push_back(id);
        }
    }
    env.storage().persistent().set(&key, &updated);
}

/// List all position ids owned by a writer for a series.
pub fn writer_positions(env: &Env, writer: &Address, series_id: u64) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::WriterPositions(writer.clone(), series_id))
        .unwrap_or(Vec::new(env))
}

/// Read the aggregate recorded collateral for a series.
pub fn series_collateral(env: &Env, series_id: u64) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::SeriesCollateral(series_id))
        .unwrap_or(0)
}

/// Overwrite the aggregate recorded collateral for a series.
pub fn set_series_collateral(env: &Env, series_id: u64, amount: i128) {
    env.storage()
        .persistent()
        .set(&DataKey::SeriesCollateral(series_id), &amount);
}

/// Increase the aggregate recorded collateral for a series by `delta`.
///
/// This is the single write site for series collateral accounting: initial
/// locking, top-ups, unwinds and settlement all funnel through here so the
/// `vault` tag ledger balance stays equal to the sum of live positions'
/// recorded collateral.
pub fn add_series_collateral(env: &Env, series_id: u64, delta: i128) -> Result<i128, MarketError> {
    let current = series_collateral(env, series_id);
    let updated = current
        .checked_add(delta)
        .ok_or(MarketError::MathOverflow)?;
    if updated < 0 {
        return Err(MarketError::InsufficientCollateral);
    }
    set_series_collateral(env, series_id, updated);
    Ok(updated)
}

/// Update a position's recorded collateral on top-up.
///
/// Applies the delta to the position record and mirrors it into the series
/// aggregate so the vault tag ledger invariant is preserved. Returns the
/// position's new recorded collateral.
pub fn top_up_position_collateral(
    env: &Env,
    position_id: u64,
    amount: i128,
) -> Result<i128, MarketError> {
    if amount <= 0 {
        return Err(MarketError::InvalidAmount);
    }
    let mut position = read_position(env, position_id)?;
    if position.status != PositionStatus::Active {
        return Err(MarketError::PositionNotActive);
    }
    let new_collateral = position
        .collateral
        .checked_add(amount)
        .ok_or(MarketError::MathOverflow)?;
    position.collateral = new_collateral;
    write_position(env, &position);
    add_series_collateral(env, position.series_id, amount)?;
    Ok(new_collateral)
}

/// Mark a position as unwound and release its recorded collateral from the
/// series aggregate.
///
/// The caller is responsible for computing the net collateral actually
/// returned to the writer (collateral minus the buy-back premium); this helper
/// only removes the position's full recorded collateral from the series
/// aggregate and marks the record unwound so it is no longer counted as live.
/// Returns the position's recorded collateral that was released.
pub fn unwind_position_record(env: &Env, position_id: u64) -> Result<i128, MarketError> {
    let mut position = read_position(env, position_id)?;
    if position.status != PositionStatus::Active {
        return Err(MarketError::PositionNotActive);
    }
    let released = position.collateral;
    position.status = PositionStatus::Unwound;
    position.collateral = 0;
    write_position(env, &position);
    add_series_collateral(env, position.series_id, -released)?;
    remove_writer_position(env, &position.writer, position.series_id, position_id);
    Ok(released)
}

/// Recompute the aggregate recorded collateral for a series by summing the
/// recorded collateral of all live positions.
///
/// Used by invariant assertions and as a repair path; the normal flow keeps
/// the aggregate up to date incrementally via `add_series_collateral`.
pub fn recompute_series_collateral(env: &Env, series_id: u64) -> Result<i128, MarketError> {
    let mut total: i128 = 0;
    let count: u64 = env
        .storage()
        .instance()
        .get(&DataKey::NextPositionId)
        .unwrap_or(0);
    let mut id: u64 = 0;
    while id < count {
        if let Some(position) = maybe_read_position(env, id) {
            if position.series_id == series_id && position.status == PositionStatus::Active {
                total = total
                    .checked_add(position.collateral)
                    .ok_or(MarketError::MathOverflow)?;
            }
        }
        id += 1;
    }
    Ok(total)
}

/// Assert that the series aggregate matches the sum of live positions'
/// collateral. Returns the aggregate on success.
pub fn assert_series_collateral_invariant(
    env: &Env,
    series_id: u64,
) -> Result<i128, MarketError> {
    let recorded = series_collateral(env, series_id);
    let computed = recompute_series_collateral(env, series_id)?;
    if recorded != computed {
        return Err(MarketError::CollateralInvariantViolated);
    }
    Ok(recorded)
}

/// Whether a series is halted or in emergency state.
pub fn is_series_halted(env: &Env, series_id: u64) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::SeriesHalted(series_id))
        .unwrap_or(false)
}

/// Set the halted / emergency flag for a series.
pub fn set_series_halted(env: &Env, series_id: u64, halted: bool) {
    env.storage()
        .persistent()
        .set(&DataKey::SeriesHalted(series_id), &halted);
}