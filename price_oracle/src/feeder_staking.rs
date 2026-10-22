//! Feeder bond accounting and the slash dispute state machine.
//!
//! A bond is `Active`, `Disputed` (a slice is held out while a slash is
//! under review) or `Removed` (the feeder left the active set). Bonds are
//! tracked as accounting entries here; token custody belongs to the
//! separate feeder-staking work. Slashed amounts go to the configured
//! `SlashDestination`: `Burn` only adds to the `Burned` total, `RewardPool`
//! adds to the `RewardPool` balance.

use soroban_sdk::{panic_with_error, Address, Bytes, Env};

use crate::{
    error::Error,
    events, storage, ttl,
    types::{Bond, BondStatus, DataKey, Dispute, Resolution, SlashDestination},
};

pub const MAX_EVIDENCE_LEN: u32 = 64;
/// Ledgers after `resolve_after` before anyone may finalize a dispute
/// nobody arbitrated; it then restores the bond.
pub const FINALIZE_GRACE_LEDGERS: u32 = ttl::DAY_IN_LEDGERS;
pub const DEFAULT_DISPUTE_WINDOW: u32 = ttl::DAY_IN_LEDGERS;

pub fn get_bond(env: &Env, feeder: &Address) -> Option<Bond> {
    env.storage()
        .persistent()
        .get(&DataKey::Bond(feeder.clone()))
}

fn save_bond(env: &Env, feeder: &Address, bond: &Bond) {
    ttl::set_persistent(env, &DataKey::Bond(feeder.clone()), bond);
}

pub fn is_disputed(env: &Env, feeder: &Address) -> bool {
    matches!(
        get_bond(env, feeder),
        Some(Bond {
            status: BondStatus::Disputed(_),
            ..
        })
    )
}

pub fn disputed_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::DisputedCount)
        .unwrap_or(0)
}

fn set_disputed_count(env: &Env, n: u32) {
    env.storage().instance().set(&DataKey::DisputedCount, &n);
}

pub fn dispute_window(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::DisputeWindow)
        .unwrap_or(DEFAULT_DISPUTE_WINDOW)
}

pub fn deposit(env: &Env, feeder: &Address, amount: i128) {
    if amount <= 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }
    let mut bond = get_bond(env, feeder).unwrap_or(Bond {
        amount: 0,
        status: BondStatus::Active,
    });
    bond.amount = bond
        .amount
        .checked_add(amount)
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidAmount));
    save_bond(env, feeder, &bond);
}

/// Opens a dispute holding `amount` of the feeder's bond. One dispute per
/// feeder at a time, and one per `(feeder, report_id)` ever.
pub fn open(env: &Env, feeder: &Address, report_id: u64, amount: i128) {
    if amount <= 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }
    let mut bond = get_bond(env, feeder).unwrap_or_else(|| panic_with_error!(env, Error::NoBond));
    if matches!(bond.status, BondStatus::Disputed(_)) {
        panic_with_error!(env, Error::DisputeAlreadyOpen);
    }
    let marker = DataKey::SlashedReport(feeder.clone(), report_id);
    if env.storage().persistent().has(&marker) {
        panic_with_error!(env, Error::ReportAlreadySlashed);
    }
    if bond.amount < amount {
        panic_with_error!(env, Error::InsufficientBond);
    }
    ttl::set_persistent(env, &marker, &true);
    let resolve_after = env.ledger().sequence() + dispute_window(env);
    bond.amount -= amount;
    bond.status = BondStatus::Disputed(Dispute {
        report_id,
        amount,
        resolve_after,
        evidence: None,
    });
    save_bond(env, feeder, &bond);
    set_disputed_count(env, disputed_count(env) + 1);
    events::dispute_opened(env, feeder.clone(), report_id, amount, resolve_after);
}

fn open_dispute(env: &Env, bond: &Bond) -> Dispute {
    match &bond.status {
        BondStatus::Disputed(d) => d.clone(),
        _ => panic_with_error!(env, Error::NoDispute),
    }
}

pub fn submit_evidence(env: &Env, feeder: &Address, evidence: Bytes) {
    if evidence.is_empty() || evidence.len() > MAX_EVIDENCE_LEN {
        panic_with_error!(env, Error::InvalidEvidence);
    }
    let mut bond =
        get_bond(env, feeder).unwrap_or_else(|| panic_with_error!(env, Error::NoDispute));
    let mut dispute = open_dispute(env, &bond);
    if env.ledger().sequence() >= dispute.resolve_after {
        panic_with_error!(env, Error::DisputeWindowClosed);
    }
    dispute.evidence = Some(evidence);
    let report_id = dispute.report_id;
    bond.status = BondStatus::Disputed(dispute);
    save_bond(env, feeder, &bond);
    events::evidence_submitted(env, feeder.clone(), report_id);
}

/// Settles the dispute. `earliest` is the first ledger it may run at.
pub fn resolve(env: &Env, feeder: &Address, resolution: Resolution, grace: u32) {
    let mut bond =
        get_bond(env, feeder).unwrap_or_else(|| panic_with_error!(env, Error::NoDispute));
    let dispute = open_dispute(env, &bond);
    if env.ledger().sequence() < dispute.resolve_after.saturating_add(grace) {
        panic_with_error!(env, Error::DisputeWindowOpen);
    }
    match resolution {
        Resolution::Restore => bond.amount += dispute.amount,
        Resolution::Slash => {
            let key = match slash_destination(env) {
                SlashDestination::Burn => DataKey::Burned,
                SlashDestination::RewardPool => DataKey::RewardPool,
            };
            let total: i128 = env.storage().instance().get(&key).unwrap_or(0);
            env.storage()
                .instance()
                .set(&key, &(total + dispute.amount));
        }
    }
    bond.status = if storage::is_feeder(env, feeder) {
        BondStatus::Active
    } else {
        BondStatus::Removed
    };
    save_bond(env, feeder, &bond);
    set_disputed_count(env, disputed_count(env) - 1);
    events::dispute_resolved(env, feeder.clone(), dispute.report_id, resolution);
}

pub fn slash_destination(env: &Env) -> SlashDestination {
    env.storage()
        .instance()
        .get(&DataKey::SlashDestination)
        .unwrap_or(SlashDestination::Burn)
}

/// Feeder joined the active set: a `Removed` bond becomes `Active` again.
pub fn on_feeder_added(env: &Env, feeder: &Address) {
    if let Some(mut bond) = get_bond(env, feeder) {
        if bond.status == BondStatus::Removed {
            bond.status = BondStatus::Active;
            save_bond(env, feeder, &bond);
        }
    }
}

/// Feeder left the active set. An open dispute is left untouched and
/// resolves to `Removed` instead of `Active`.
pub fn on_feeder_removed(env: &Env, feeder: &Address) {
    if let Some(mut bond) = get_bond(env, feeder) {
        if bond.status == BondStatus::Active {
            bond.status = BondStatus::Removed;
            save_bond(env, feeder, &bond);
        }
    }
}
