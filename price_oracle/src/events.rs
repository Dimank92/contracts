use soroban_sdk::{Address, Env, Symbol};

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

pub fn feeder_rotated(env: &Env, old_feeder: Address, new_feeder: Address) {
    env.events().publish(
        (Symbol::new(env, "feeder_rotated"),),
        (old_feeder, new_feeder),
    );
}

pub fn symbol_feeder_added(env: &Env, symbol: Symbol, feeder: Address) {
    env.events().publish(
        (Symbol::new(env, "symbol_feeder_added"), symbol),
        feeder,
    );
}

pub fn symbol_feeder_removed(env: &Env, symbol: Symbol, feeder: Address) {
    env.events().publish(
        (Symbol::new(env, "symbol_feeder_removed"), symbol),
        feeder,
    );
}

pub fn symbol_feeder_rotated(env: &Env, symbol: Symbol, old_feeder: Address, new_feeder: Address) {
    env.events().publish(
        (Symbol::new(env, "symbol_feeder_rotated"), symbol),
        (old_feeder, new_feeder),
    );
}

pub fn price_reported(env: &Env, feeder: Address, symbol: Symbol, price: i128) {
    env.events()
        .publish((Symbol::new(env, "price_reported"), feeder, symbol), price);
}

pub fn multisig_set(env: &Env, multisig: Address) {
    env.events()
        .publish((Symbol::new(env, "multisig_set"),), multisig);
}

pub fn symbol_precision_set(env: &Env, symbol: Symbol, precision: u32) {
    env.events().publish(
        (Symbol::new(env, "symbol_precision_set"), symbol),
        precision,
    );
}
