use soroban_sdk::{Address, Env, Symbol};

pub fn series_created(env: &Env, series_id: u64, strike_price: i128, expiry: u64, premium: i128) {
    env.events().publish(
        (Symbol::new(env, "series_created"),),
        (series_id, strike_price, expiry, premium),
    );
}

pub fn premium_updated(env: &Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
    env.events().publish(
        (Symbol::new(env, "premium_updated"), series_id),
        (new_premium, new_implied_vol),
    );
}

pub fn option_bought(
    env: &Env,
    buyer: Address,
    pos_id: u64,
    series_id: u64,
    contracts: i128,
    total_premium: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_bought"), buyer),
        (pos_id, series_id, contracts, total_premium),
    );
}

pub fn option_written(
    env: &Env,
    writer: Address,
    pos_id: u64,
    series_id: u64,
    contracts: i128,
    writer_premium: i128,
    required_collateral: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_written"), writer),
        (
            pos_id,
            series_id,
            contracts,
            writer_premium,
            required_collateral,
        ),
    );
}

pub fn option_exercised(
    env: &Env,
    owner: Address,
    position_id: u64,
    settlement_price: i128,
    payout: i128,
) {
    env.events().publish(
        (Symbol::new(env, "option_exercised"), owner),
        (position_id, settlement_price, payout),
    );
}

pub fn settlement_price_set(env: &Env, series_id: u64, price: i128) {
    env.events()
        .publish((Symbol::new(env, "settlement_price_set"), series_id), price);
}

pub fn collateral_reclaimed(env: &Env, writer: Address, position_id: u64, reclaim: i128) {
    env.events().publish(
        (Symbol::new(env, "collateral_reclaimed"), writer),
        (position_id, reclaim),
    );
}

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

pub fn series_cancelled(env: &Env, series_id: u64) {
    env.events()
        .publish((Symbol::new(env, "series_cancelled"),), series_id);
}

pub fn refund_claimed(env: &Env, owner: Address, position_id: u64, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "refund_claimed"), owner),
        (position_id, amount),
    );
}

pub fn fee_rate_updated(env: &Env, new_bps: i128) {
    env.events()
        .publish((Symbol::new(env, "fee_rate_updated"),), new_bps);
}

pub fn series_escrowed_to_vault(env: &Env, series_id: u64, amount: i128) {
    env.events().publish(
        (Symbol::new(env, "series_escrowed_to_vault"), series_id),
        amount,
    );
}

pub fn vault_updated(env: &Env, old_vault: Option<Address>, new_vault: Address) {
    env.events()
        .publish((Symbol::new(env, "vault_updated"),), (old_vault, new_vault));
}

pub fn admin_transfer_proposed(env: &Env, admin: Address, pending: Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transfer_proposed"),),
        (admin, pending),
    );
}

pub fn oracle_updated(env: &Env, old_oracle: Address, new_oracle: Address) {
    env.events().publish(
        (Symbol::new(env, "oracle_updated"),),
        (old_oracle, new_oracle),
    );
}

pub fn params_registry_updated(env: &Env, registry: Address) {
    env.events()
        .publish((Symbol::new(env, "params_registry_updated"),), registry);
}
