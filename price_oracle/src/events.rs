use soroban_sdk::{Address, Env, Symbol};

use crate::types::Resolution;

pub fn admin_transferred(env: &Env, old_admin: Address, new_admin: Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transferred"),),
        (old_admin, new_admin),
    );
}

pub fn paused(env: &Env) {
    env.events().publish((Symbol::new(env, "paused"),), ());
}

pub fn unpaused(env: &Env) {
    env.events().publish((Symbol::new(env, "unpaused"),), ());
}

pub fn max_staleness_updated(env: &Env, new_staleness: u64) {
    env.events()
        .publish((Symbol::new(env, "max_staleness_updated"),), new_staleness);
}

pub fn min_reports_updated(env: &Env, new_min_reports: u32) {
    env.events()
        .publish((Symbol::new(env, "min_reports_updated"),), new_min_reports);
}

pub fn feeder_added(env: &Env, feeder: Address) {
    env.events()
        .publish((Symbol::new(env, "feeder_added"),), feeder);
}

pub fn feeder_removed(env: &Env, feeder: Address) {
    env.events()
        .publish((Symbol::new(env, "feeder_removed"),), feeder);
}

pub fn price_reported(env: &Env, feeder: Address, symbol: Symbol, price: i128) {
    env.events()
        .publish((Symbol::new(env, "price_reported"), feeder, symbol), price);
}

pub fn dispute_opened(
    env: &Env,
    feeder: Address,
    report_id: u64,
    amount: i128,
    resolve_after: u32,
) {
    env.events().publish(
        (Symbol::new(env, "dispute_opened"), feeder, report_id),
        (amount, resolve_after),
    );
}

pub fn evidence_submitted(env: &Env, feeder: Address, report_id: u64) {
    env.events()
        .publish((Symbol::new(env, "evidence_submitted"), feeder), report_id);
}

pub fn dispute_resolved(env: &Env, feeder: Address, report_id: u64, resolution: Resolution) {
    env.events().publish(
        (Symbol::new(env, "dispute_resolved"), feeder, report_id),
        resolution,
    );
}
