use soroban_sdk::{Address, Env, Symbol};

pub fn deposit_queued(env: &Env, user: Address, epoch: u32, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "deposit_queued"), user, epoch), amount);
}

pub fn withdraw_queued(env: &Env, user: Address, epoch: u32, shares: i128) {
    env.events()
        .publish((Symbol::new(env, "withdraw_queued"), user, epoch), shares);
}

pub fn claimed(env: &Env, user: Address, shares: i128, assets: i128) {
    env.events()
        .publish((Symbol::new(env, "claimed"), user), (shares, assets));
}

pub fn option_written(
    env: &Env,
    series_id: u64,
    position_id: u64,
    collateral: i128,
    premium: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_written"), series_id, position_id),
        (collateral, premium),
    );
}

pub fn collateral_reclaimed(env: &Env, position_id: u64, collateral: i128, returned: i128) {
    env.events().publish(
        (Symbol::new(env, "collateral_reclaimed"), position_id),
        (collateral, returned),
    );
}

pub fn epoch_processed(env: &Env, epoch: u32, total_assets: i128, total_shares: i128) {
    env.events().publish(
        (Symbol::new(env, "epoch_processed"), epoch),
        (total_assets, total_shares),
    );
}

pub fn config_updated(env: &Env) {
    env.events()
        .publish((Symbol::new(env, "config_updated"),), ());
}

pub fn cap_reached(env: &Env, epoch: u32, total: i128) {
    env.events()
        .publish((Symbol::new(env, "cap_reached"), epoch), total);
}

pub fn paused(env: &Env, paused: bool) {
    env.events().publish((Symbol::new(env, "paused"),), paused);
}

pub fn fees_collected(
    env: &Env,
    epoch: u32,
    treasury: Address,
    performance: i128,
    management: i128,
) {
    env.events().publish(
        (Symbol::new(env, "fees_collected"), epoch, treasury),
        (performance, management),
    );
}
