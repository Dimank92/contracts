#![no_std]

//! Zenith Vault — a per-(token, tag) escrow ledger.
//!
//! Built to close a gap discovered while testing options_market: that
//! contract holds every writer's collateral and every buyer's premium in
//! one undifferentiated token balance, with no accounting of which
//! balance is actually earmarked for which position. This contract is
//! that missing accounting layer — deposit/withdraw against a caller-
//! defined `tag` (e.g. a series_id), so "how much is earmarked for tag
//! 7" is always answerable instead of inferred from the token contract's
//! raw balance.
//!
//! One deployment holds any number of allowlisted tokens and serves any
//! number of integrator contracts. Tags are namespaced by their owner
//! (`Tag { owner, kind, id }`), so integrators can never collide, and
//! only a tag's owner may withdraw from or reassign it. The admin keeps
//! configuration powers only; moving escrow without the owner's signature
//! requires multisig approval (`*_via_multisig`).

use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, token, vec, Address, BytesN, Env,
    IntoVal, Symbol, Vec,
};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_adversaries;
#[cfg(test)]
mod test_fuzz;

pub mod error;
mod events;
mod migrations;
mod token_validation;
mod ttl;
pub mod types;

pub use error::Error;
pub use types::Tag;
use types::{DataKey, LegacyKey};

fn require_not_paused(env: &Env) {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

fn admin(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Admin).unwrap()
}

fn default_token(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Token).unwrap()
}

/// Cross-calls `multisig.is_approved(action_id)` via `invoke_contract`, so
/// this crate needs no multisig wasm to compile.
fn multisig_approved(env: &Env, multisig_contract: &Address, action_id: &BytesN<32>) -> bool {
    env.invoke_contract(
        multisig_contract,
        &Symbol::new(env, "is_approved"),
        vec![env, action_id.into_val(env)],
    )
}

fn require_multisig_approved(env: &Env, multisig_contract: &Address, action_id: BytesN<32>) {
    if !multisig_approved(env, multisig_contract, &action_id) {
        panic_with_error!(env, Error::Unauthorized);
    }
}

fn require_positive(env: &Env, amount: i128) {
    if amount <= 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }
}

fn escrow_of(env: &Env, token: &Address, tag: &Tag) -> i128 {
    ttl::get_persistent(env, &DataKey::Escrow(token.clone(), tag.clone())).unwrap_or(0)
}

fn set_escrow(env: &Env, token: &Address, tag: &Tag, amount: i128) {
    ttl::set_persistent(env, &DataKey::Escrow(token.clone(), tag.clone()), &amount);
}

/// Every token whose ledger total has ever been written (schema v2+).
/// Before v2 it isn't stored, and only the default token is known.
pub(crate) fn tokens(env: &Env) -> Vec<Address> {
    env.storage()
        .instance()
        .get(&DataKey::Tokens)
        .unwrap_or_else(|| vec![env, default_token(env)])
}

/// Sum of `TotalEscrowed` over `tokens`, for migration invariants.
pub(crate) fn ledger_totals(env: &Env) -> Vec<i128> {
    let mut totals = Vec::new(env);
    for token in tokens(env).iter() {
        totals.push_back(total_of(env, &token));
    }
    totals
}

fn total_of(env: &Env, token: &Address) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalEscrowed(token.clone()))
        .unwrap_or(0)
}

fn set_total(env: &Env, token: &Address, total: i128) {
    let mut index = tokens(env);
    if !index.contains(token) {
        index.push_back(token.clone());
        env.storage().instance().set(&DataKey::Tokens, &index);
    }
    env.storage()
        .instance()
        .set(&DataKey::TotalEscrowed(token.clone()), &total);
}

fn tag_owner(env: &Env, tag: &Tag) -> Option<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::TagOwner(tag.clone()))
}

fn legacy_owner(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&DataKey::LegacyOwner)
        .unwrap_or_else(|| admin(env))
}

fn legacy_tag(env: &Env, id: u64) -> Tag {
    Tag {
        owner: legacy_owner(env),
        kind: symbol_short!("legacy"),
        id,
    }
}

/// Requires the recorded owner of `tag` to authorize. A tag that was
/// never deposited into has no owner yet, so nothing can be taken out of
/// it anyway — its (zero) balance check fails right after.
fn require_tag_owner(env: &Env, tag: &Tag) {
    if let Some(owner) = tag_owner(env, tag) {
        owner.require_auth();
    } else {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }
}

/// Records `tag`'s owner on its first use. Only a registered integrator
/// (or the legacy namespace owner) may create tags, and only inside its
/// own namespace — `tag.owner` must authorize creation, so nobody can
/// open a tag in another integrator's namespace.
fn ensure_tag_created(env: &Env, tag: &Tag, owner_already_authed: bool) {
    if tag_owner(env, tag).is_some() {
        return;
    }
    let registered: bool = env
        .storage()
        .instance()
        .get(&DataKey::Integrator(tag.owner.clone()))
        .unwrap_or(false);
    if !registered && tag.owner != legacy_owner(env) {
        panic_with_error!(env, Error::UnregisteredIntegrator);
    }
    if !owner_already_authed {
        tag.owner.require_auth();
    }
    env.storage()
        .persistent()
        .set(&DataKey::TagOwner(tag.clone()), &tag.owner);
}

fn shortfall_given(env: &Env, token: &Address, actual_balance: i128) -> i128 {
    total_of(env, token).saturating_sub(actual_balance).max(0)
}

fn do_deposit(env: &Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
    migrations::require_current(env);
    require_not_paused(env);
    from.require_auth();
    require_positive(env, amount);
    let allowed: bool = env
        .storage()
        .instance()
        .get(&DataKey::AllowedToken(token.clone()))
        .unwrap_or(false);
    if !allowed {
        panic_with_error!(env, Error::TokenNotAllowed);
    }
    ensure_tag_created(env, &tag, from == tag.owner);

    // Credit what actually arrived, not what was requested: a
    // fee-on-transfer (or otherwise non-standard) token would otherwise
    // leave the ledger claiming more than the vault holds.
    let this = env.current_contract_address();
    let token_client = token::Client::new(env, &token);
    let before = token_client.balance(&this);
    token_client.transfer(&from, &this, &amount);
    let received = token_client.balance(&this).checked_sub(before).unwrap();
    require_positive(env, received);

    let balance = escrow_of(env, &token, &tag).checked_add(received).unwrap();
    set_escrow(env, &token, &tag, balance);
    set_total(
        env,
        &token,
        total_of(env, &token).checked_add(received).unwrap(),
    );

    events::deposited(env, token, tag, from, received);
    received
}

/// Debits `tag` and pays `to`. Callers handle authorization.
fn do_withdraw(env: &Env, token: Address, tag: Tag, to: Address, amount: i128) {
    migrations::require_current(env);
    require_not_paused(env);
    require_positive(env, amount);

    let balance = escrow_of(env, &token, &tag);
    if balance < amount {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }

    let this = env.current_contract_address();
    let token_client = token::Client::new(env, &token);
    let actual_balance = token_client.balance(&this);
    let shortfall = shortfall_given(env, &token, actual_balance);
    if shortfall > 0 {
        events::shortfall_detected(env, token.clone(), shortfall);
    }
    if actual_balance < amount {
        panic_with_error!(env, Error::InsufficientVaultBalance);
    }

    set_escrow(env, &token, &tag, balance.checked_sub(amount).unwrap());
    set_total(
        env,
        &token,
        total_of(env, &token).checked_sub(amount).unwrap(),
    );
    token_client.transfer(&this, &to, &amount);

    events::withdrawn(env, token, tag, to, amount);
}

/// Pure ledger reassignment. Callers handle authorization.
fn do_transfer_tag(env: &Env, token: Address, from_tag: Tag, to_tag: Tag, amount: i128) {
    migrations::require_current(env);
    require_not_paused(env);
    require_positive(env, amount);

    let from_balance = escrow_of(env, &token, &from_tag);
    if from_balance < amount {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }
    set_escrow(
        env,
        &token,
        &from_tag,
        from_balance.checked_sub(amount).unwrap(),
    );
    let to_balance = escrow_of(env, &token, &to_tag).checked_add(amount).unwrap();
    set_escrow(env, &token, &to_tag, to_balance);

    // TotalEscrowed is unaffected — nothing entered or left the vault,
    // only which tag it's earmarked under changed.
    events::tag_transferred(env, token, from_tag, to_tag, amount);
}

fn do_sweep(env: &Env, token: Address, to: Address) -> i128 {
    migrations::require_current(env);
    let this = env.current_contract_address();
    let token_client = token::Client::new(env, &token);
    let actual_balance = token_client.balance(&this);

    // A clawback (or any token-level balance decrease) can leave the
    // balance BELOW the ledger. That's a shortfall, not untagged funds —
    // report it as NoUntaggedFunds instead of an arithmetic trap.
    let untagged = actual_balance.saturating_sub(total_of(env, &token));
    if untagged <= 0 {
        panic_with_error!(env, Error::NoUntaggedFunds);
    }

    token_client.transfer(&this, &to, &untagged);
    events::swept_untagged(env, token, to, untagged);
    untagged
}

fn set_token_allowed_inner(env: &Env, token: Address, allowed: bool) {
    env.storage()
        .instance()
        .set(&DataKey::AllowedToken(token.clone()), &allowed);
    events::token_allowed(env, token, allowed);
}

fn set_integrator_inner(env: &Env, integrator: Address, allowed: bool) {
    env.storage()
        .instance()
        .set(&DataKey::Integrator(integrator.clone()), &allowed);
    events::integrator_set(env, integrator, allowed);
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    pub fn initialize(env: Env, admin: Address, token: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage().instance().set(&DataKey::LegacyOwner, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Tokens, &vec![&env, token.clone()]);
        set_token_allowed_inner(&env, token, true);
        zenith_common::migrations::init_schema(&env, migrations::CURRENT_SCHEMA_VERSION);
    }

    /// Hands off control to a new address. Requires the CURRENT admin's
    /// signature, not the incoming one. The admin configures the vault
    /// (token allowlist, integrator registry, pause, sweep) but owns no
    /// integrator's tags, so this never hands over escrowed funds.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the current admin's own signature — same pattern as
    /// options_market's and price_oracle's transfer_admin_via_multisig.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        new_admin: Address,
    ) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Emergency stop: blocks deposit, withdraw, and transfer_tag. Both
    /// sides, unlike options_market's pause (which only blocks new
    /// exposure, not winding existing positions down) — a vault holding
    /// real funds should be freezable outright if something's gone wrong.
    pub fn pause(env: Env) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        ttl::extend_instance(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    /// Permissionless alternative to pause(), same rationale and pattern
    /// as options_market's and price_oracle's pause_via_multisig.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    // ─── token allowlist / integrator registry ─────────────────────────

    /// Adds or removes `token` from the allowlist. Removing a token only
    /// blocks NEW deposits of it: existing balances stay withdrawable,
    /// transferable, and sweepable.
    pub fn set_token_allowed(env: Env, token: Address, allowed: bool) {
        admin(&env).require_auth();
        set_token_allowed_inner(&env, token, allowed);
    }

    pub fn set_token_allowed_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        token: Address,
        allowed: bool,
    ) {
        require_multisig_approved(&env, &multisig_contract, action_id);
        set_token_allowed_inner(&env, token, allowed);
    }

    pub fn is_token_allowed(env: Env, token: Address) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::AllowedToken(token))
            .unwrap_or(false)
    }

    /// Registers (or deregisters) an integrator allowed to create tags in
    /// its own namespace. Deregistering blocks new tags only; tags it
    /// already owns keep working.
    pub fn set_integrator(env: Env, integrator: Address, allowed: bool) {
        admin(&env).require_auth();
        set_integrator_inner(&env, integrator, allowed);
    }

    pub fn set_integrator_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        integrator: Address,
        allowed: bool,
    ) {
        require_multisig_approved(&env, &multisig_contract, action_id);
        set_integrator_inner(&env, integrator, allowed);
    }

    pub fn is_integrator(env: Env, integrator: Address) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Integrator(integrator))
            .unwrap_or(false)
    }

    // ─── escrow ────────────────────────────────────────────────────────

    /// Pulls `amount` of `token` from `from` into the vault, crediting
    /// `(token, tag)`. `from` must authorize the transfer. The first
    /// deposit into a tag records `tag.owner` as its owner; that requires
    /// `tag.owner` to be a registered integrator and to authorize (which
    /// it already has when it is also `from`). End users can therefore
    /// deposit into a contract-owned tag only with that contract's auth.
    ///
    /// Credits the measured balance delta, not `amount`, and returns it:
    /// with a fee-on-transfer token the tag is credited what actually
    /// arrived. Costs two extra token balance reads per deposit.
    pub fn deposit(env: Env, from: Address, token: Address, tag: Tag, amount: i128) -> i128 {
        do_deposit(&env, from, token, tag, amount)
    }

    /// Pays `amount` of `(token, tag)`'s escrow to `to`. Requires the
    /// tag owner's signature — in the intended integration that is the
    /// calling contract itself, so the contract-to-contract call
    /// satisfies require_auth() through the call itself.
    ///
    /// Panics with InsufficientEscrowBalance if `tag` doesn't have that
    /// much earmarked, regardless of other tags' deposits. Emits
    /// `shortfall_detected` if the vault's actual balance of `token` is
    /// below its ledger (e.g. after a clawback), and panics with
    /// InsufficientVaultBalance instead of a token error if it can't
    /// cover this payout.
    pub fn withdraw(env: Env, token: Address, tag: Tag, to: Address, amount: i128) {
        require_tag_owner(&env, &tag);
        do_withdraw(&env, token, tag, to, amount);
    }

    /// Emergency payout without the tag owner: requires multisig approval
    /// (e.g. the owning contract is being replaced and can't sign). Same
    /// InsufficientEscrowBalance cap applies — approval changes who can
    /// call this, not how much any tag is entitled to.
    pub fn withdraw_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        token: Address,
        tag: Tag,
        to: Address,
        amount: i128,
    ) {
        require_multisig_approved(&env, &multisig_contract, action_id);
        do_withdraw(&env, token, tag, to, amount);
    }

    /// Moves `amount` of `token` escrow from `from_tag` to `to_tag` with
    /// no token movement — e.g. options_market's roll_position. Requires
    /// `from_tag`'s owner; if `to_tag` belongs to (or would be created
    /// for) a different owner, that owner must authorize too.
    pub fn transfer_tag(env: Env, token: Address, from_tag: Tag, to_tag: Tag, amount: i128) {
        require_tag_owner(&env, &from_tag);
        let from_owner = tag_owner(&env, &from_tag).unwrap();
        match tag_owner(&env, &to_tag) {
            Some(to_owner) => {
                if to_owner != from_owner {
                    to_owner.require_auth();
                }
            }
            None => ensure_tag_created(&env, &to_tag, to_tag.owner == from_owner),
        }
        do_transfer_tag(&env, token, from_tag, to_tag, amount);
    }

    /// Emergency alternative to transfer_tag, gated on multisig approval.
    /// A `to_tag` that doesn't exist yet is recorded as owned by
    /// `to_tag.owner`, so funds only ever land in their namespace owner's
    /// control.
    pub fn transfer_tag_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        token: Address,
        from_tag: Tag,
        to_tag: Tag,
        amount: i128,
    ) {
        require_multisig_approved(&env, &multisig_contract, action_id);
        do_transfer_tag(&env, token, from_tag, to_tag, amount);
    }

    pub fn balance_of(env: Env, token: Address, tag: Tag) -> i128 {
        escrow_of(&env, &token, &tag)
    }

    pub fn get_total_escrowed(env: Env, token: Address) -> i128 {
        total_of(&env, &token)
    }

    /// `max(0, TotalEscrowed(token) − actual balance)`: how much of the
    /// ledger the vault can no longer back, e.g. after a SAC clawback.
    pub fn get_shortfall(env: Env, token: Address) -> i128 {
        let actual = token::Client::new(&env, &token).balance(&env.current_contract_address());
        shortfall_given(&env, &token, actual)
    }

    pub fn get_tag_owner(env: Env, tag: Tag) -> Option<Address> {
        tag_owner(&env, &tag)
    }

    pub fn get_admin(env: Env) -> Address {
        admin(&env)
    }

    /// The default token the `_legacy` wrappers operate on.
    /// Tokens the ledger has a total for (see `DataKey::Tokens`).
    pub fn get_tokens(env: Env) -> Vec<Address> {
        tokens(&env)
    }

    pub fn get_token(env: Env) -> Address {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    /// Recovers tokens that landed on the vault's own address OUTSIDE
    /// deposit() — e.g. a direct token transfer sent straight to this
    /// contract's address by mistake, rather than through deposit(),
    /// which is the only path that actually credits a tag's ledger. Those
    /// tokens sit in the vault's real balance but aren't accounted for
    /// under any tag, so they'd otherwise be stuck forever: no tag's
    /// withdraw could ever reach them (withdraw is capped at that tag's
    /// OWN escrowed balance), and get_total_escrowed's ledger sum would
    /// permanently under-report the vault's actual token balance.
    pub fn sweep_untagged(env: Env, token: Address, to: Address) -> i128 {
        ttl::extend_instance(&env);
        admin(&env).require_auth();
        do_sweep(&env, token, to)
    }

    /// Permissionless alternative to sweep_untagged, gated on multisig
    /// approval.
    pub fn sweep_untagged_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: BytesN<32>,
        token: Address,
        to: Address,
    ) -> i128 {
        ttl::extend_instance(&env);
        if !multisig_approved(&env, &multisig_contract, &action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        do_sweep(&env, token, to)
    }

    // ─── legacy u64-tag wrappers (default token) ───────────────────────

    /// The namespaced tag a legacy `u64` id maps to:
    /// `(legacy owner, "legacy", id)`, where the legacy owner is the
    /// admin at initialize time.
    pub fn legacy_tag(env: Env, id: u64) -> Tag {
        legacy_tag(&env, id)
    }

    pub fn deposit_legacy(env: Env, from: Address, id: u64, amount: i128) -> i128 {
        let tag = legacy_tag(&env, id);
        do_deposit(&env, from, default_token(&env), tag, amount)
    }

    pub fn withdraw_legacy(env: Env, id: u64, to: Address, amount: i128) {
        let tag = legacy_tag(&env, id);
        require_tag_owner(&env, &tag);
        do_withdraw(&env, default_token(&env), tag, to, amount);
    }

    pub fn transfer_tag_legacy(env: Env, from_id: u64, to_id: u64, amount: i128) {
        let from_tag = legacy_tag(&env, from_id);
        let to_tag = legacy_tag(&env, to_id);
        Self::transfer_tag(env.clone(), default_token(&env), from_tag, to_tag, amount);
    }

    pub fn balance_of_legacy(env: Env, id: u64) -> i128 {
        escrow_of(&env, &default_token(&env), &legacy_tag(&env, id))
    }

    /// Moves pre-upgrade `Escrow(u64)` entries (single-token, bare-u64
    /// tags) to `Escrow(default token, legacy_tag(id))`, owned by the
    /// current admin. The old `TotalEscrowed` moves on the first call.
    /// Idempotent: ids with no legacy entry are skipped.
    pub fn migrate_legacy(env: Env, ids: Vec<u64>) {
        let admin = admin(&env);
        admin.require_auth();
        if !env.storage().instance().has(&DataKey::LegacyOwner) {
            env.storage().instance().set(&DataKey::LegacyOwner, &admin);
        }
        let token = default_token(&env);
        if !env
            .storage()
            .instance()
            .has(&DataKey::AllowedToken(token.clone()))
        {
            set_token_allowed_inner(&env, token.clone(), true);
        }

        if let Some(old_total) = env
            .storage()
            .instance()
            .get::<_, i128>(&LegacyKey::TotalEscrowed)
        {
            set_total(
                &env,
                &token,
                total_of(&env, &token).checked_add(old_total).unwrap(),
            );
            env.storage().instance().remove(&LegacyKey::TotalEscrowed);
        }

        for id in ids.iter() {
            let key = LegacyKey::Escrow(id);
            let Some(amount) = env.storage().persistent().get::<_, i128>(&key) else {
                continue;
            };
            env.storage().persistent().remove(&key);
            let tag = legacy_tag(&env, id);
            if tag_owner(&env, &tag).is_none() {
                env.storage()
                    .persistent()
                    .set(&DataKey::TagOwner(tag.clone()), &tag.owner);
            }
            let balance = escrow_of(&env, &token, &tag).checked_add(amount).unwrap();
            set_escrow(&env, &token, &tag, balance);
            events::legacy_migrated(&env, id, amount);
        }
    }
}
